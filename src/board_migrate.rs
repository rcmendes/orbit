//! `orbit migrate`: converts a pre-ADR-0019 Board into a standalone Board
//! folder. Previews by default; `--apply` performs the conversion in a single
//! Git commit per Board.

use std::path::{Path, PathBuf};

use serde_json::{Value, json};

use crate::board::{self, Board, Config, Discovery, OrbitBoard};
use crate::cli::BoardMigrateArgs as MigrateArgs;
use crate::error::{OrbitError, Result, code};
use crate::output::CommandOutput;
use crate::task::{self, RemoteLink, Task};

const MIGRATE_MESSAGE: &str = "orbit: migrate board";

#[derive(Debug, Default)]
struct ContentStats {
    tasks_scanned: usize,
    github_links: usize,
    repo_labels: usize,
    remote_links: usize,
    areas: Vec<String>,
    config_changes: Vec<String>,
}

/// Where the Board folder should live after migration.
#[derive(Debug, Clone, PartialEq)]
enum Layout {
    /// Already at the target; only content conversion may be needed.
    InPlace,
    /// Move/convert into `<anchor>/.orbit/board`.
    ToOrbitBoard { orbit: PathBuf, target: PathBuf },
}

pub fn cmd_migrate(args: &MigrateArgs, dir: Option<&Path>) -> Result<CommandOutput> {
    let discovery = Board::discovery(dir);
    if let Some(error) = discovery.error {
        return Err(error);
    }
    let Some(root) = discovery.root.clone() else {
        return Ok(CommandOutput::new(
            json!({ "migrated": false, "reason": "no Board found; run `orbit init` to create one" }),
            "no Orbit Board found; run `orbit init` to create one\n".to_string(),
        ));
    };
    let mut board = Board::at_root(root, discovery.warnings.clone())?;
    board.orbit_dir = discovery.orbit_dir.clone();

    let needs_layout = board.needs_migration(Some(&discovery));
    let layout = plan_layout(&board, &discovery)?;
    let stats = plan_content(&board)?;
    let content_needed = stats.github_links > 0
        || stats.repo_labels > 0
        || !stats.config_changes.is_empty()
        || stats.areas_needed();

    if !needs_layout && !content_needed {
        return Ok(CommandOutput::new(
            json!({
                "migrated": false,
                "board": board.root,
                "reason": "the Board already uses the standalone layout",
            }),
            format!("the Board at {} is already migrated\n", board.root.display()),
        ));
    }

    let report = json!({
        "board": board.root,
        "from": board.root,
        "to": layout_target(&layout, &board),
        "layout": if needs_layout { "convert" } else { "content" },
        "needs_layout": needs_layout,
        "tasks": stats.tasks_scanned,
        "github_links": stats.github_links,
        "repo_labels": stats.repo_labels,
        "remote_projects": stats.remote_projects(),
        "areas": stats.areas,
        "config_changes": stats.config_changes,
        "applied": args.apply,
        "commit": Value::Null,
    });
    let human = format!(
        "orbit migrate {} the Board at {}\n  layout: {}\n  tasks: {} scanned, {} github link(s), {} repo label(s)\n",
        if args.apply { "applied to" } else { "preview for" },
        board.root.display(),
        layout_target(&layout, &board).display(),
        stats.tasks_scanned,
        stats.github_links,
        stats.repo_labels,
    );

    if !args.apply {
        let mut output = CommandOutput::new(report, human);
        output.warnings = board.warnings.clone();
        return Ok(output);
    }

    let commit = apply(&board, &discovery, &layout, needs_layout, content_needed)?;
    let mut data = report;
    data["applied"] = json!(true);
    let commit_hash = commit.clone();
    data["commit"] = commit.map(|commit| json!(commit)).unwrap_or(Value::Null);
    let mut output = CommandOutput::new(
        data,
        format!("{human}  committed as {}\n", commit_hash.unwrap_or_default()),
    );
    output.warnings = board.warnings.clone();
    Ok(output)
}

impl ContentStats {
    fn areas_needed(&self) -> bool {
        !self.areas.is_empty()
    }
    fn remote_projects(&self) -> usize {
        self.config_changes
            .iter()
            .filter(|change| change.starts_with("remote.projects"))
            .count()
    }
}

fn layout_target(layout: &Layout, board: &Board) -> PathBuf {
    match layout {
        Layout::InPlace => board.root.clone(),
        Layout::ToOrbitBoard { target, .. } => target.clone(),
    }
}

/// Decides the post-migration Board folder location.
fn plan_layout(board: &Board, discovery: &Discovery) -> Result<Layout> {
    let root = &board.root;
    // Already anchored: `.orbit/board` or a visible folder reached through an
    // Orbit-folder pointer. A legacy Board sitting directly in `.orbit` has
    // `orbit_dir == root` and is not anchored.
    let anchored = root
        .parent()
        .and_then(|parent| parent.file_name())
        .and_then(|name| name.to_str())
        == Some(".orbit")
        || board.orbit_dir.as_ref().is_some_and(|orbit| orbit != root);
    if anchored {
        return Ok(Layout::InPlace);
    }
    // A Board sitting directly in an Orbit folder moves to `.orbit/board`.
    if root.file_name().and_then(|name| name.to_str()) == Some(".orbit") {
        return Ok(Layout::ToOrbitBoard {
            orbit: root.clone(),
            target: root.join("board"),
        });
    }
    // Legacy `<workspace>/board` or `<workspace>/.board` folders.
    if root
        .parent()
        .and_then(|parent| parent.file_name())
        .and_then(|name| name.to_str())
        == Some(".board")
    {
        let workspace = root.parent().and_then(Path::parent).unwrap_or(root);
        return Ok(Layout::ToOrbitBoard {
            orbit: workspace.join(".orbit"),
            target: workspace.join(".orbit").join("board"),
        });
    }
    if root.file_name().and_then(|name| name.to_str()) == Some("board")
        && root.parent().is_some()
        && !crate::git::is_git_repo(root.parent().unwrap())
    {
        let workspace = root.parent().unwrap();
        return Ok(Layout::ToOrbitBoard {
            orbit: workspace.join(".orbit"),
            target: workspace.join(".orbit").join("board"),
        });
    }
    // Worktree-backed Boards (including the legacy `.git/orbit/board`)
    // become `<main-checkout>/.orbit/board`.
    if root.join(".git").is_file() || matches!(discovery.source, board::DiscoverySource::LegacyLocal) {
        if let Some(target) = repo_board_target(root)? {
            return Ok(Layout::ToOrbitBoard {
                orbit: target.parent().unwrap().parent().unwrap().to_path_buf(),
                target,
            });
        }
    }
    // A standalone Board outside any anchor stays in place; only its
    // configuration and Task files are converted.
    Ok(Layout::InPlace)
}

/// `<main-checkout>/.orbit/board` for the repository that owns `root`.
fn repo_board_target(root: &Path) -> Result<Option<PathBuf>> {
    let context = Board::repo_context_at(root)?;
    let Some(context) = context else {
        return Ok(None);
    };
    Ok(Some(context.main_checkout.join(".orbit").join("board")))
}

/// Scans the Board for content that `orbit migrate` rewrites.
fn plan_content(board: &Board) -> Result<ContentStats> {
    let mut stats = ContentStats::default();
    let config = Config::load(&board.root)?;
    if config.board.storage.is_some() {
        stats.config_changes.push("board.storage".to_string());
    }
    if config.github != crate::github::GitHubConfig::default() {
        stats.config_changes.push("github→remote".to_string());
    }


    let tasks_dir = board.root.join("tasks");
    let mut areas = std::collections::BTreeSet::new();
    if let Ok(entries) = std::fs::read_dir(&tasks_dir) {
        for entry in entries.flatten() {
            let path = entry.path();
            if path.extension().and_then(|e| e.to_str()) != Some("md") {
                continue;
            }
            let Some(id) = path.file_stem().and_then(|s| s.to_str()) else {
                continue;
            };
            if id.starts_with('.') {
                continue;
            }
            let content = std::fs::read_to_string(&path)
                .map_err(|e| OrbitError::io(format!("read {}", path.display()), e))?;
            let task = task::parse_task(id.to_string(), &content)?;
            stats.tasks_scanned += 1;
            if task.front.github.is_some() {
                stats.github_links += 1;
            }
            if task.front.remote.is_some() {
                stats.remote_links += 1;
            }
            if task
                .front
                .labels
                .iter()
                .any(|label| label.starts_with("repo:"))
            {
                stats.repo_labels += 1;
            }
            if let Some(area) = task.front.area {
                areas.insert(area.as_str().to_string());
            }
        }
    }
    if config.board.areas.is_empty() && !areas.is_empty() {
        stats.areas = areas.into_iter().collect();
    }
    Ok(stats)
}

/// Applies the migration. Returns the migration commit hash when one was
/// created.
fn apply(
    board: &Board,
    discovery: &Discovery,
    layout: &Layout,
    needs_layout: bool,
    content_needed: bool,
) -> Result<Option<String>> {
    if !needs_layout {
        // Content-only migration in place.
        if !content_needed {
            return Ok(None);
        }
        let _lock = board.lock_exclusive()?;
        migrate_content(&board.root)?;
        if git_has_changes(&board.root)? {
            board.commit(MIGRATE_MESSAGE, true)?;
        }
        return Ok(None);
    }

    match layout {
        Layout::InPlace => unreachable!("needs_layout implies a layout target"),
        Layout::ToOrbitBoard { orbit, target } => {
            migrate_to_orbit_board(board, discovery, orbit, target, content_needed)
        }
    }
}

fn migrate_to_orbit_board(
    board: &Board,
    _discovery: &Discovery,
    orbit: &Path,
    target: &Path,
    content_needed: bool,
) -> Result<Option<String>> {
    let _lock = board.lock_exclusive()?;
    let source = board.root.clone();

    if source.join(".git").is_file() {
        // Worktree-backed Board: build a standalone repository at the target
        // preserving the `orbit` branch history, then convert in one commit.
        return migrate_worktree(board, orbit, target, content_needed);
    }

    // Standalone repository: move the folder, then convert content in place.
    if let Some(parent) = target.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|e| OrbitError::io(format!("create {}", parent.display()), e))?;
    }
    if source != *target {
        if target.exists() {
            return Err(OrbitError::config(format!(
                "migration target {} already exists",
                target.display()
            )));
        }
        std::fs::rename(&source, target)
            .map_err(|e| OrbitError::io(format!("move {} to {}", source.display(), target.display()), e))?;
    }
    let moved = Board::at_root(target.to_path_buf(), Vec::new())?;
    if content_needed {
        migrate_content(target)?;
    }
    if git_has_changes(target)? {
        let _ = moved;
        crate::git::commit(target, MIGRATE_MESSAGE)?;
    }
    let _ = orbit;
    Ok(None)
}

/// Converts a worktree-backed Board into a standalone repository at
/// `<orbit>/board`, preserving branch history, in one migration commit.
fn migrate_worktree(
    board: &Board,
    orbit: &Path,
    target: &Path,
    content_needed: bool,
) -> Result<Option<String>> {
    let source = board.root.clone();
    let context = Board::repo_context_at(&source)?
        .ok_or_else(|| OrbitError::config("cannot resolve the repository owning the Board worktree"))?;
    let branch = board.config.board.branch.clone();

    // Stage a standalone clone of the Board branch next to the Orbit folder.
    let stage = orbit
        .parent()
        .unwrap_or_else(|| Path::new("."))
        .join(format!(".orbit-migrate-{}", std::process::id()));
    if stage.exists() {
        std::fs::remove_dir_all(&stage)
            .map_err(|e| OrbitError::io(format!("remove stale {}", stage.display()), e))?;
    }
    std::fs::create_dir_all(&stage)
        .map_err(|e| OrbitError::io(format!("create {}", stage.display()), e))?;
    crate::git::init_repo(&stage)?;
    git_out(&stage, &["fetch", context.main_checkout.to_str().unwrap_or_default(), &branch])?;
    git_out(&stage, &["checkout", "-b", &branch, "FETCH_HEAD"])?;

    // Content conversion + the single migration commit inside the stage.
    if content_needed {
        migrate_content(&stage)?;
    }
    // The layout conversion itself is recorded by removing `storage` and
    // rewriting the Board into its own repository; commit once so the
    // standalone history carries the conversion.
    let _ = crate::git::commit(&stage, MIGRATE_MESSAGE)?;

    // Remove the old worktree, then place the standalone Board folder.
    crate::git::worktree_remove(&context.main_checkout, &source)?;
    if let Some(parent) = target.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|e| OrbitError::io(format!("create {}", parent.display()), e))?;
    }
    if target.exists() {
        return Err(OrbitError::config(format!(
            "migration target {} already exists",
            target.display()
        )));
    }
    std::fs::rename(&stage, target).map_err(|e| {
        OrbitError::io(
            format!("move {} to {}", stage.display(), target.display()),
            e,
        )
    })?;

    // The Orbit folder keeps `cache/` beside the Board folder; hide the whole
    // Orbit folder from the code repository through `.git/info/exclude`.
    let cache = orbit.join("cache");
    std::fs::create_dir_all(&cache)
        .map_err(|e| OrbitError::io(format!("create {}", cache.display()), e))?;
    let _ = board::ensure_orbit_excluded(orbit, None);

    // The code repository no longer owns Board history: drop its `orbit`
    // branch when it still points at the migrated tip.
    if crate::git::branch_exists(&context.main_checkout, &branch)? {
        let _ = crate::git::delete_branch(&context.main_checkout, &branch);
    }
    Ok(None)
}

fn git_has_changes(root: &Path) -> Result<bool> {
    Ok(!crate::git::status_porcelain(root)?.trim().is_empty())
}

fn git_out(cwd: &Path, args: &[&str]) -> Result<String> {
    crate::git::git_output(cwd, args).map_err(|error| {
        OrbitError::new(code::GIT_ERROR, format!("git {} failed: {error}", args.join(" ")))
    })
}

/// Rewrites `config.toml` and Task files in place:
/// `github:` → `remote:`, `repo:*` labels → `repo`, `[github]` → `[remote]`,
/// `board.storage` removed, `board.areas` seeded from Area values in use.
fn migrate_content(root: &Path) -> Result<ContentStats> {
    let mut stats = ContentStats::default();
    let mut config = Config::load(root)?;
    let mut config_changed = false;
    if config.board.storage.take().is_some() {
        config_changed = true;
        stats.config_changes.push("board.storage".to_string());
    }

    // `[github]` → `[remote]` project entries plus the adapter settings.
    if config.github != crate::github::GitHubConfig::default() {
        let github = std::mem::take(&mut config.github);
        config.remote.auto_push |= github.auto_push;
        for repo in &github.repositories {
            config.remote.projects.push(crate::remote::RemoteProject {
                tracker: "github".to_string(),
                project: repo.clone(),
            });
            stats.config_changes.push(format!("remote.projects += {repo}"));
        }
        let mut adapter = github.clone();
        adapter.repositories.clear();
        if adapter != crate::github::GitHubConfig::default() {
            config.remote.github = Some(adapter);
        }
        config_changed = true;
        stats.config_changes.push("github→remote".to_string());
    }

    // Seed `board.areas` from the Area values Tasks actually use.
    let tasks_dir = root.join("tasks");
    let mut areas = std::collections::BTreeSet::new();
    if let Ok(entries) = std::fs::read_dir(&tasks_dir) {
        for entry in entries.flatten() {
            let path = entry.path();
            if path.extension().and_then(|e| e.to_str()) != Some("md") {
                continue;
            }
            let Some(id) = path.file_stem().and_then(|s| s.to_str()) else {
                continue;
            };
            if id.starts_with('.') {
                continue;
            }
            let content = std::fs::read_to_string(&path)
                .map_err(|e| OrbitError::io(format!("read {}", path.display()), e))?;
            let mut task = task::parse_task(id.to_string(), &content)?;
            stats.tasks_scanned += 1;
            let before = task::render_task(&task)?;
            let mut changed = false;

            // `github:` frontmatter → provider-neutral `remote:`.
            if let Some(github) = task.front.github.take() {
                if task.front.remote.is_none() {
                    task.front.remote = Some(RemoteLink {
                        tracker: "github".to_string(),
                        project: github.repo.clone(),
                        id: github.number.to_string(),
                        url: Some(format!(
                            "https://github.com/{}/issues/{}",
                            github.repo, github.number
                        )),
                        active: github.active,
                        baseline: github
                            .last_sync
                            .as_ref()
                            .and_then(|baseline| serde_yaml_ng::to_value(baseline).ok()),
                    });
                }
                stats.github_links += 1;
                changed = true;
            }
            // `repo:*` labels → the `repo` field.
            let repo_label = task
                .front
                .labels
                .iter()
                .find_map(|label| label.strip_prefix("repo:"))
                .map(|name| name.to_string());
            if repo_label.is_some() {
                if task.front.repo.is_none()
                    && let Some(name) = &repo_label
                {
                    task.front.repo = Some(name.clone());
                }
                task.front
                    .labels
                    .retain(|label| !label.starts_with("repo:"));
                stats.repo_labels += 1;
                changed = true;
            }
            if let Some(area) = task.front.area {
                areas.insert(area.as_str().to_string());
            }

            if changed {
                let rendered = task::render_task(&task)?;
                if rendered != before {
                    crate::atomic::write_atomic(&path, rendered.as_bytes(), true)?;
                }
            }
        }
    }
    if config.board.areas.is_empty() && !areas.is_empty() {
        config.board.areas = areas.iter().cloned().collect();
        config_changed = true;
        stats.areas = areas.iter().cloned().collect();
        stats.config_changes.push("board.areas".to_string());
    }
    if config_changed {
        config.validate()?;
        let toml = config.to_toml()?;
        crate::atomic::write_atomic(&root.join("config.toml"), toml.as_bytes(), true)?;
    }
    Ok(stats)
}

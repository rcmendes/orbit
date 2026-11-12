//! `orbit migrate`: converts a pre-ADR-0019 Board into a standalone Board
//! folder. Previews by default; `--apply` performs the conversion in a single
//! Git commit per Board.

use std::path::{Path, PathBuf};

use serde_json::{Value, json};

use crate::board::{self, Board, Config, Discovery};
use crate::cli::BoardMigrateArgs as MigrateArgs;
use crate::error::{OrbitError, Result, code};
use crate::git::CommitOutcome;
use crate::output::CommandOutput;
use crate::task::{self, RemoteLink};

const MIGRATE_MESSAGE: &str = "orbit: migrate board";

#[derive(Debug, Default)]
struct ContentStats {
    tasks_scanned: usize,
    github_links: usize,
    repo_labels: usize,
    remote_links: usize,
    /// Remote Projects the Board will have after migration: existing
    /// `[remote.projects]` entries plus the `[github]` repositories that are
    /// not already configured.
    remote_projects: usize,
    areas: Vec<String>,
    config_changes: Vec<String>,
}

/// Where the Board folder should live after migration.
#[derive(Debug, Clone, PartialEq)]
enum Layout {
    /// Already at the target; only content conversion may be needed.
    InPlace,
    /// Convert into a standalone repository at `target`, anchored by the
    /// Orbit folder `orbit` (`.orbit/cache` lives beside the Board folder).
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
            format!(
                "the Board at {} is already migrated\n",
                board.root.display()
            ),
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
        "remote_projects": stats.remote_projects,
        "areas": stats.areas,
        "config_changes": stats.config_changes,
        "applied": args.apply,
        "commit": Value::Null,
    });
    let human = format!(
        "orbit migrate {} the Board at {}\n  layout: {}\n  tasks: {} scanned, {} github link(s), {} repo label(s)\n",
        if args.apply {
            "applied to"
        } else {
            "preview for"
        },
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

    let commit = apply(&board, &discovery, &layout, content_needed)?;
    let mut data = report;
    data["applied"] = json!(true);
    data["commit"] = commit.map(|commit| json!(commit)).unwrap_or(Value::Null);
    let mut human = human;
    if let Some(hash) = &data["commit"].as_str() {
        human.push_str(&format!("  committed as {hash}\n"));
    }
    let mut output = CommandOutput::new(data, human);
    output.warnings = board.warnings.clone();
    Ok(output)
}

impl ContentStats {
    fn areas_needed(&self) -> bool {
        !self.areas.is_empty()
    }
}

fn layout_target(layout: &Layout, board: &Board) -> PathBuf {
    match layout {
        Layout::InPlace => board.root.clone(),
        Layout::ToOrbitBoard { target, .. } => target.clone(),
    }
}

fn orbit_dir_of(board: &Board, discovery: &Discovery) -> Option<PathBuf> {
    board
        .orbit_dir
        .clone()
        .or_else(|| discovery.orbit_dir.clone())
}

fn dir_name(path: &Path) -> Option<&str> {
    path.file_name().and_then(|name| name.to_str())
}

fn parent_name(path: &Path) -> Option<&str> {
    path.parent()
        .and_then(|parent| parent.file_name())
        .and_then(|name| name.to_str())
}

/// `<common>/orbit/board` — the retired in-Git-directory Board store. The
/// path shape identifies it even when discovery reached the Board through an
/// explicit `--dir` rather than the LegacyLocal source.
fn is_legacy_local_board(root: &Path) -> bool {
    root.file_name().and_then(|name| name.to_str()) == Some(board::BOARD_DIR)
        && root
            .parent()
            .and_then(|parent| parent.file_name())
            .and_then(|name| name.to_str())
            == Some("orbit")
        && root
            .parent()
            .and_then(Path::parent)
            .is_some_and(|common| common.join("HEAD").is_file())
}

/// Resolves the repository context for a Board inside a Git directory. The
/// Board path has no work tree of its own, so fall back to the caller's
/// repository and finally to the common dir's parent working tree.
fn repo_context_for_legacy(root: &Path) -> Result<Option<crate::board::RepoContext>> {
    if let Some(context) = Board::repo_context_at(root)? {
        return Ok(Some(context));
    }
    if let Some(context) = Board::repo_context() {
        return Ok(Some(context));
    }
    if let Some(common) = root.parent().and_then(Path::parent)
        && let Some(worktree) = common.parent()
        && let Some(toplevel) = crate::git::toplevel_containing(worktree)
    {
        return Board::repo_context_at(&toplevel);
    }
    Ok(None)
}

/// Decides the post-migration Board folder location.
fn plan_layout(board: &Board, discovery: &Discovery) -> Result<Layout> {
    let root = &board.root;
    let orbit_dir = orbit_dir_of(board, discovery);
    let worktree = root.join(".git").is_file();
    let legacy_local = matches!(discovery.source, board::DiscoverySource::LegacyLocal)
        || is_legacy_local_board(root);
    // Anchored means the Board already sits where an Orbit folder points at
    // it: `.orbit/board` hidden, or a visible folder named by the `board`
    // pointer file.
    let anchored = parent_name(root) == Some(board::ORBIT_DIR)
        || orbit_dir.as_ref().is_some_and(|orbit| *orbit != *root);

    if worktree || legacy_local {
        // A worktree-backed Board becomes a standalone repository. When it
        // already sits at its final folder the conversion happens in place;
        // an `.orbit`-shaped worktree moves to `.orbit/board`; anything else
        // (the legacy `.git/orbit/board`) moves to `<main>/.orbit/board`.
        if legacy_local {
            let context = repo_context_for_legacy(root)?.ok_or_else(|| {
                OrbitError::config("cannot resolve the repository owning the legacy Board")
            })?;
            let orbit = context.main_checkout.join(board::ORBIT_DIR);
            return Ok(Layout::ToOrbitBoard {
                target: orbit.join(board::BOARD_DIR),
                orbit,
            });
        }
        if dir_name(root) == Some(board::ORBIT_DIR) {
            return Ok(Layout::ToOrbitBoard {
                orbit: root.clone(),
                target: root.join(board::BOARD_DIR),
            });
        }
        if anchored {
            let orbit = orbit_dir
                .clone()
                .or_else(|| root.parent().map(Path::to_path_buf))
                .ok_or_else(|| {
                    OrbitError::config("cannot resolve the Orbit folder for the Board")
                })?;
            return Ok(Layout::ToOrbitBoard {
                orbit,
                target: root.clone(),
            });
        }
        if let Some(target) = repo_board_target(root)? {
            return Ok(Layout::ToOrbitBoard {
                orbit: target.parent().unwrap_or(root).to_path_buf(),
                target,
            });
        }
        return Ok(Layout::InPlace);
    }

    if anchored {
        return Ok(Layout::InPlace);
    }
    // A standalone Board sitting directly in an Orbit folder moves to
    // `.orbit/board`; the folder itself stays as the Orbit folder.
    if dir_name(root) == Some(board::ORBIT_DIR) {
        return Ok(Layout::ToOrbitBoard {
            orbit: root.clone(),
            target: root.join(board::BOARD_DIR),
        });
    }
    // Legacy `<workspace>/.board` and `<workspace>/board` folders.
    if parent_name(root) == Some(board::LEGACY_BOARD_DIR) {
        let workspace = root.parent().and_then(Path::parent).unwrap_or(root);
        return Ok(Layout::ToOrbitBoard {
            orbit: workspace.join(board::ORBIT_DIR),
            target: workspace.join(board::ORBIT_DIR).join(board::BOARD_DIR),
        });
    }
    if dir_name(root) == Some(board::BOARD_DIR)
        && let Some(workspace) = root.parent()
        && !crate::git::is_git_repo(workspace)
    {
        return Ok(Layout::ToOrbitBoard {
            orbit: workspace.join(board::ORBIT_DIR),
            target: workspace.join(board::ORBIT_DIR).join(board::BOARD_DIR),
        });
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
    // Remote Projects after migration: what `[remote.projects]` already
    // configures plus each `[github]` repository not already present.
    let mut projects: std::collections::BTreeSet<(String, String)> = config
        .remote
        .projects
        .iter()
        .map(|project| (project.tracker.clone(), project.project.clone()))
        .collect();
    if config.github != crate::github::GitHubConfig::default() {
        for repo in &config.github.repositories {
            if projects.insert(("github".to_string(), repo.clone())) {
                stats
                    .config_changes
                    .push(format!("remote.projects += {repo}"));
            }
        }
        stats.config_changes.push("github→remote".to_string());
    }
    stats.remote_projects = projects.len();

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
            if let Some(area) = &task.front.area {
                areas.insert(area.clone());
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
    content_needed: bool,
) -> Result<Option<String>> {
    match layout {
        Layout::InPlace => {
            let _lock = board.lock_exclusive()?;
            ensure_board_repo(&board.root, &board.config.board.branch)?;
            if content_needed {
                migrate_content(&board.root)?;
            }
            let commit = commit_migration(&board.root)?;
            hide_in_working_tree(board, discovery)?;
            Ok(commit)
        }
        Layout::ToOrbitBoard { orbit, target } => {
            migrate_to_orbit_board(board, discovery, orbit, target, content_needed)
        }
    }
}

/// A migrated Board must be a standalone repository on its configured
/// branch. Legacy Board folders that were plain directories — the
/// `.git/orbit/board` store, hand-made boards — get `git init` and the
/// branch pointer before the migration commit.
fn ensure_board_repo(root: &Path, branch: &str) -> Result<()> {
    if root.join(".git").exists() {
        return Ok(());
    }
    crate::git::init_repo(root)?;
    crate::git::set_head(root, branch)?;
    Ok(())
}

/// Keeps a Board that stays inside a Git working tree hidden through
/// `.git/info/exclude`, never `.gitignore` (ADR 0019).
fn hide_in_working_tree(board: &Board, discovery: &Discovery) -> Result<()> {
    match orbit_dir_of(board, discovery) {
        Some(orbit) => {
            let visible =
                (board.root.parent() != Some(orbit.as_path())).then_some(board.root.as_path());
            let _ = board::ensure_orbit_excluded(&orbit, visible);
        }
        None => {
            let _ = Board::ensure_excluded(&board.root);
        }
    }
    Ok(())
}

/// Commits pending Board changes as the single migration commit and reports
/// the new HEAD.
fn commit_migration(root: &Path) -> Result<Option<String>> {
    if !git_has_changes(root)? {
        return Ok(None);
    }
    match crate::git::commit(root, MIGRATE_MESSAGE)? {
        CommitOutcome::Committed => Ok(Some(crate::git::head_commit(root)?)),
        _ => Ok(None),
    }
}

fn migrate_to_orbit_board(
    board: &Board,
    discovery: &Discovery,
    orbit: &Path,
    target: &Path,
    content_needed: bool,
) -> Result<Option<String>> {
    let _lock = board.lock_exclusive()?;
    let source = board.root.clone();

    if source.join(".git").is_file() {
        // Worktree-backed Board: build a standalone repository preserving the
        // `orbit` branch history, then convert in one commit.
        return migrate_worktree(board, orbit, target, content_needed);
    }

    if source == *target {
        ensure_board_repo(target, &board.config.board.branch)?;
        if content_needed {
            migrate_content(target)?;
        }
        let commit = commit_migration(target)?;
        hide_in_working_tree(board, discovery)?;
        return Ok(commit);
    }

    // Standalone repository: move the folder, ensure it is a repository,
    // then convert content in place.
    move_board_folder(&source, target)?;
    ensure_board_repo(target, &board.config.board.branch)?;
    if content_needed {
        migrate_content(target)?;
    }
    let commit = commit_migration(target)?;
    finish_orbit_folder(orbit, target)?;
    Ok(commit)
}

/// Moves `source` to `target`. When the target sits inside the source — a
/// standalone Board that *is* the `.orbit` folder becoming `.orbit/board` —
/// the folder is staged through a sibling first. A `cache/` directory inside
/// the Orbit folder belongs to the Orbit folder, not the Board, and stays.
fn move_board_folder(source: &Path, target: &Path) -> Result<()> {
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

    if !target.starts_with(source) {
        std::fs::rename(source, target).map_err(|e| {
            OrbitError::io(
                format!("move {} to {}", source.display(), target.display()),
                e,
            )
        })?;
        return Ok(());
    }

    // `<orbit>` → `<orbit>/board`: keep the Orbit folder, move the Board
    // repository into it.
    let temp = source.with_file_name(format!(
        "{}.migrate-{}",
        source
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_else(|| ".orbit".to_string()),
        std::process::id()
    ));
    if temp.exists() {
        std::fs::remove_dir_all(&temp)
            .map_err(|e| OrbitError::io(format!("remove stale {}", temp.display()), e))?;
    }
    std::fs::rename(source, &temp).map_err(|e| {
        OrbitError::io(
            format!("move {} to {}", source.display(), temp.display()),
            e,
        )
    })?;
    std::fs::create_dir_all(source)
        .map_err(|e| OrbitError::io(format!("create {}", source.display()), e))?;
    // A `cache/` inside the moved folder belongs to the Orbit folder, not
    // the Board repository.
    let moved_cache = temp.join(board::CACHE_DIR);
    if moved_cache.is_dir() {
        std::fs::rename(&moved_cache, source.join(board::CACHE_DIR)).map_err(|e| {
            OrbitError::io(
                format!(
                    "move {} to {}",
                    moved_cache.display(),
                    source.join(board::CACHE_DIR).display()
                ),
                e,
            )
        })?;
    }
    std::fs::rename(&temp, target).map_err(|e| {
        OrbitError::io(
            format!("move {} to {}", temp.display(), target.display()),
            e,
        )
    })?;
    Ok(())
}

/// Creates `<orbit>/cache` beside the Board folder and hides the Orbit
/// folder — plus a visible Board folder — inside a Git working tree through
/// `.git/info/exclude` (never `.gitignore`).
fn finish_orbit_folder(orbit: &Path, target: &Path) -> Result<()> {
    let cache = orbit.join(board::CACHE_DIR);
    std::fs::create_dir_all(&cache)
        .map_err(|e| OrbitError::io(format!("create {}", cache.display()), e))?;
    let visible = (target.parent() != Some(orbit)).then_some(target);
    let _ = board::ensure_orbit_excluded(orbit, visible);
    Ok(())
}

/// Converts a worktree-backed Board into a standalone repository at `target`,
/// preserving branch history, in one migration commit.
fn migrate_worktree(
    board: &Board,
    orbit: &Path,
    target: &Path,
    content_needed: bool,
) -> Result<Option<String>> {
    let source = board.root.clone();
    let context = Board::repo_context_at(&source)?.ok_or_else(|| {
        OrbitError::config("cannot resolve the repository owning the Board worktree")
    })?;
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
    git_out(
        &stage,
        &[
            "fetch",
            context.main_checkout.to_str().unwrap_or_default(),
            &branch,
        ],
    )?;
    git_out(&stage, &["checkout", "-b", &branch, "FETCH_HEAD"])?;

    // Content conversion + the single migration commit inside the stage.
    if content_needed {
        migrate_content(&stage)?;
    }
    let commit = commit_migration(&stage)?;

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

    finish_orbit_folder(orbit, target)?;

    // The code repository no longer owns Board history: drop its `orbit`
    // branch when it still points at the migrated tip.
    if crate::git::branch_exists(&context.main_checkout, &branch)? {
        let _ = crate::git::delete_branch(&context.main_checkout, &branch);
    }
    Ok(commit)
}

fn git_has_changes(root: &Path) -> Result<bool> {
    Ok(!crate::git::status_porcelain(root)?.trim().is_empty())
}

fn git_out(cwd: &Path, args: &[&str]) -> Result<String> {
    crate::git::git_output(cwd, args).map_err(|error| {
        OrbitError::new(
            code::GIT_ERROR,
            format!("git {} failed: {error}", args.join(" ")),
        )
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
            let project = crate::remote::RemoteProject {
                tracker: "github".to_string(),
                project: repo.clone(),
            };
            if !config.remote.projects.contains(&project) {
                config.remote.projects.push(project);
                stats
                    .config_changes
                    .push(format!("remote.projects += {repo}"));
            }
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
            if let Some(area) = &task.front.area {
                areas.insert(area.clone());
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

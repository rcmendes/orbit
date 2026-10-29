//! Workspace diagnostics used by `orbit doctor`.
//!
//! The checks in this module are read-only. They inspect the Workspace folder,
//! the rebuildable repository and Issue caches, Linked Task provenance, local
//! Board remnants, and managed agent setup. Safe repairs remain in `doctor
//! --fix`; semantic conflicts are only reported with a next action.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use chrono::{DateTime, Duration, Utc};
use serde_json::json;

use crate::board::Board;
use crate::github::status::status_from_issue;
use crate::graph::{Graph, Issue, LoadMode};
use crate::task::Status;

const CACHE_STALE_AFTER_HOURS: i64 = 24;

pub(crate) struct WorkspaceDiagnosis {
    pub root: Option<PathBuf>,
    pub board: Option<PathBuf>,
    pub organization: Option<String>,
    pub repositories: usize,
    pub included_repositories: usize,
    pub last_refreshed_at: Option<DateTime<Utc>>,
    pub issues: Vec<Issue>,
    pub warnings: Vec<String>,
}

impl WorkspaceDiagnosis {
    pub(crate) fn json(&self) -> serde_json::Value {
        match &self.root {
            None => serde_json::Value::Null,
            Some(root) => json!({
                "root": root,
                "board": self.board,
                "organization": self.organization,
                "repositories": self.repositories,
                "included_repositories": self.included_repositories,
                "last_refreshed_at": self.last_refreshed_at,
                "board_path": self.board.as_ref().map(|path| path.join("config.toml")),
                "repository_cache": crate::workspace::catalog_path(root),
                "issue_cache": crate::issue_catalog::cache_path(root),
            }),
        }
    }
}

/// Diagnose one Workspace. `board` is present when discovery resolved the
/// Workspace Board; the layout checks still run when only `.orbit` remnants
/// exist so `doctor` can explain how to finish initialization or migration.
pub(crate) fn diagnose(workspace: &Path, board: Option<&Board>) -> WorkspaceDiagnosis {
    let mut issues = Vec::new();
    let mut warnings = Vec::new();
    let catalog = diagnose_layout_and_repositories(workspace, &mut issues, &mut warnings);
    let issue_catalog =
        diagnose_issue_cache(workspace, catalog.as_ref(), &mut issues, &mut warnings);
    diagnose_migration(workspace, &mut issues, &mut warnings);

    if let Some(board) = board {
        diagnose_links(
            board,
            issue_catalog.as_ref(),
            catalog.as_ref(),
            &mut issues,
            &mut warnings,
        );
        diagnose_agent_setup(
            workspace,
            board,
            catalog.as_ref(),
            &mut issues,
            &mut warnings,
        );
    }

    WorkspaceDiagnosis {
        root: Some(workspace.to_path_buf()),
        board: board.map(|board| board.root.clone()),
        organization: catalog
            .as_ref()
            .and_then(|catalog| catalog.organization.clone()),
        repositories: catalog
            .as_ref()
            .map(|catalog| catalog.repositories.len())
            .unwrap_or(0),
        included_repositories: catalog
            .as_ref()
            .map(|catalog| {
                catalog
                    .repositories
                    .values()
                    .filter(|entry| entry.included)
                    .count()
            })
            .unwrap_or(0),
        last_refreshed_at: catalog
            .as_ref()
            .and_then(|catalog| catalog.last_refreshed_at),
        issues,
        warnings,
    }
}

fn diagnose_layout_and_repositories(
    workspace: &Path,
    issues: &mut Vec<Issue>,
    warnings: &mut Vec<String>,
) -> Option<crate::workspace::RepositoryCatalog> {
    let orbit = workspace.join(".orbit");
    let board = orbit.join("board");
    if board.exists() && !board.is_dir() {
        issues.push(
            Issue::new(
                "WORKSPACE_LAYOUT",
                None,
                format!(
                    "Workspace Board path {} is not a directory",
                    board.display()
                ),
            )
            .with_cause("the Workspace layout is malformed")
            .with_next_action(format!(
                "move {} aside, then run `orbit init {}`",
                board.display(),
                workspace.display()
            )),
        );
    } else if !board.exists() {
        issues.push(
            Issue::new(
                "WORKSPACE_LAYOUT",
                None,
                format!(
                    "Workspace root {} has .orbit state but no Workspace Board",
                    workspace.display()
                ),
            )
            .with_cause("initialization or migration did not finish creating .orbit/board")
            .with_next_action(format!("run `orbit init {}`", workspace.display())),
        );
    }
    let cache = orbit.join("cache");
    if cache.exists() && !cache.is_dir() {
        issues.push(
            Issue::new(
                "WORKSPACE_LAYOUT",
                None,
                format!(
                    "Workspace cache path {} is not a directory",
                    cache.display()
                ),
            )
            .with_cause("the rebuildable cache location is occupied by a file")
            .with_next_action(format!(
                "move {} aside, then run `orbit workspace refresh`",
                cache.display()
            )),
        );
    }

    let catalog_path = crate::workspace::catalog_path(workspace);
    let catalog = match crate::workspace::catalog_for_diagnostics(workspace) {
        Ok(catalog) => catalog,
        Err(error) => {
            issues.push(
                Issue::new(
                    "WORKSPACE_CACHE_CORRUPT",
                    None,
                    format!(
                        "could not read the Workspace repository catalog: {}",
                        error.message
                    ),
                )
                .with_cause(
                    "the repository cache is missing required structure or contains invalid JSON",
                )
                .with_next_action(format!(
                    "inspect or move aside {}, then run `orbit workspace refresh`",
                    catalog_path.display()
                )),
            );
            return None;
        }
    };

    if !catalog_path.exists() || catalog.last_refreshed_at.is_none() {
        warnings.push(format!(
            "{}: Workspace repositories have not been discovered yet; run `orbit workspace refresh`",
            workspace.display()
        ));
    } else if is_stale(catalog.last_refreshed_at) {
        issues.push(
            Issue::warning(
                "WORKSPACE_CATALOG_STALE",
                "Workspace repository discovery is older than 24 hours",
            )
            .with_repository(workspace.display().to_string())
            .with_cause("discovery runs only on explicit refresh")
            .with_next_action(
                "run `orbit workspace refresh` to reconcile new or changed child repositories",
            ),
        );
    }

    if let Some(organization) = catalog.organization.as_deref()
        && organization.trim().is_empty()
    {
        issues.push(
            Issue::new(
                "WORKSPACE_ORGANIZATION",
                None,
                "Workspace organization is empty",
            )
            .with_cause("the repository catalog has no usable organization")
            .with_next_action("run `orbit github connect` from an organization-owned child repository, then refresh"),
        );
    }
    if catalog.repositories.is_empty() && catalog.last_refreshed_at.is_some() {
        issues.push(
            Issue::warning(
                "WORKSPACE_REPOSITORIES_EMPTY",
                "Workspace discovery found no included organization repositories",
            )
            .with_repository(workspace.display().to_string())
            .with_cause("no direct child Git repository has an eligible GitHub origin")
            .with_next_action("clone the organization repositories under the Workspace, then run `orbit workspace refresh`"),
        );
    }

    for entry in catalog.repositories.values() {
        let path = workspace.join(&entry.path);
        let identity = entry.repository.clone();
        match entry.status {
            crate::workspace::RepositoryStatus::Included => {
                if !entry.discovered || !path.exists() {
                    issues.push(
                        Issue::new(
                            "WORKSPACE_REPOSITORY_MISSING",
                            None,
                            format!("included repository {} is missing at {}", identity, path.display()),
                        )
                        .with_repository(identity)
                        .with_cause("the child directory was moved or deleted after discovery")
                        .with_next_action("run `orbit workspace refresh`; the repository will become stale and its cache retained"),
                    );
                } else if !crate::workspace::is_direct_git_repository(&path) {
                    issues.push(
                        Issue::new(
                            "WORKSPACE_REPOSITORY_INACCESSIBLE",
                            None,
                            format!("included repository {} is no longer a direct Git repository", identity),
                        )
                        .with_repository(identity)
                        .with_cause("the child path no longer has a readable Git working tree")
                        .with_next_action("restore the repository or run `orbit workspace refresh` to mark it stale"),
                    );
                }
            }
            crate::workspace::RepositoryStatus::Stale => issues.push(
                Issue::warning(
                    "WORKSPACE_REPOSITORY_STALE",
                    format!("repository {identity} is stale; its last cache was retained"),
                )
                .with_repository(identity)
                .with_cause(
                    entry
                        .error
                        .clone()
                        .unwrap_or_else(|| "the last GitHub access check failed".to_string()),
                )
                .with_next_action("check GitHub access and run `orbit workspace refresh`; detach explicitly if the repository should be removed"),
            ),
            crate::workspace::RepositoryStatus::Inaccessible => issues.push(
                Issue::new(
                    "WORKSPACE_REPOSITORY_INACCESSIBLE",
                    None,
                    format!("repository {identity} is inaccessible"),
                )
                .with_repository(identity)
                .with_cause(
                    entry
                        .error
                        .clone()
                        .unwrap_or_else(|| "GitHub access failed".to_string()),
                )
                .with_next_action("check `gh auth status`, repository permissions, and the configured organization, then run `orbit workspace refresh`"),
            ),
            crate::workspace::RepositoryStatus::Removed => issues.push(
                Issue::warning(
                    "WORKSPACE_REPOSITORY_REMOVED",
                    format!("repository {identity} is no longer discovered; its cache is retained"),
                )
                .with_repository(identity)
                .with_cause(
                    entry
                        .error
                        .clone()
                        .unwrap_or_else(|| "the repository directory or Git origin is gone".to_string()),
                )
                .with_next_action("run `orbit workspace detach OWNER/REPO` to preview a permanent detach, or restore the repository and refresh"),
            ),
            crate::workspace::RepositoryStatus::UnsupportedOwner => issues.push(
                Issue::warning(
                    "WORKSPACE_REPOSITORY_UNSUPPORTED",
                    format!("repository {identity} is outside the single Workspace organization"),
                )
                .with_repository(identity)
                .with_cause(
                    entry
                        .error
                        .clone()
                        .unwrap_or_else(|| "the repository owner is not accepted in full mode".to_string()),
                )
                .with_next_action("use one organization per full-mode Workspace, or exclude the repository"),
            ),
            crate::workspace::RepositoryStatus::Excluded => {}
        }
    }

    for directory in &catalog.non_git_directories {
        // A directory that used to be a repository is already reported as
        // WORKSPACE_REPOSITORY_STALE, which is the actionable finding. A plain
        // folder that was never a repository is informational: Workspace
        // discovery only includes direct child Git repositories, and ordinary
        // folders such as docs/ or notes/ must not make the Workspace unhealthy.
        if catalog.repositories.values().any(|entry| {
            entry.status == crate::workspace::RepositoryStatus::Stale && entry.path == *directory
        }) {
            continue;
        }
        warnings.push(format!(
            "child directory `{directory}` is not a Git repository; Workspace discovery ignores it"
        ));
    }
    for directory in &catalog.missing_origins {
        issues.push(
            Issue::warning(
                "WORKSPACE_MISSING_ORIGIN",
                format!("child Git repository `{directory}` has no readable `origin` remote"),
            )
            .with_repository(directory)
            .with_cause("the repository cannot be identified without a GitHub origin")
            .with_next_action("add a GitHub `origin` remote, then run `orbit workspace refresh`"),
        );
    }
    for origin in &catalog.unsupported_origins {
        issues.push(
            Issue::warning(
                "WORKSPACE_UNSUPPORTED_ORIGIN",
                format!(
                    "child repository `{}` has an unsupported origin",
                    origin.path
                ),
            )
            .with_repository(origin.path.clone())
            .with_issue(origin.origin.clone())
            .with_cause(origin.reason.clone())
            .with_next_action(
                "use a github.com HTTPS or SSH origin, then run `orbit workspace refresh`",
            ),
        );
    }

    let config_organization = board_organization(workspace, &board, issues);
    if let (Some(config), Some(catalog)) = (
        config_organization.as_deref(),
        catalog.organization.as_deref(),
    ) && !config.eq_ignore_ascii_case(catalog)
    {
        issues.push(
            Issue::warning(
                "WORKSPACE_CONFIG_CONFLICT",
                format!("repository catalog organization `{catalog}` differs from Board organization `{config}`"),
            )
            .with_repository(workspace.display().to_string())
            .with_cause("the Workspace configuration changed after the last discovery")
            .with_next_action("run `orbit workspace refresh` to rebuild discovery for the configured organization"),
        );
    }
    if config_organization.is_none() {
        warnings.push(format!(
            "{}: Workspace has no GitHub organization configured; run `orbit github connect` from an organization-owned child repository before refreshing",
            workspace.display()
        ));
    }

    Some(catalog)
}

fn board_organization(workspace: &Path, board: &Path, issues: &mut Vec<Issue>) -> Option<String> {
    if !board.is_dir() {
        return None;
    }
    match Board::at_root(board.to_path_buf(), Vec::new()) {
        Ok(board) => board.config.github.organization.clone(),
        Err(error) => {
            issues.push(
                Issue::new(
                    "WORKSPACE_LAYOUT",
                    None,
                    format!("Workspace Board at {} cannot be loaded: {}", board.display(), error.message),
                )
                .with_repository(workspace.display().to_string())
                .with_cause("the Board configuration is missing or malformed")
                .with_next_action("repair or restore the Board configuration, or run `orbit doctor` from a healthy checkout"),
            );
            None
        }
    }
}

fn diagnose_issue_cache(
    workspace: &Path,
    repositories: Option<&crate::workspace::RepositoryCatalog>,
    issues: &mut Vec<Issue>,
    warnings: &mut Vec<String>,
) -> Option<crate::issue_catalog::IssueCatalog> {
    let path = crate::issue_catalog::cache_path(workspace);
    let catalog = match crate::issue_catalog::catalog_for_diagnostics(workspace) {
        Ok(catalog) => catalog,
        Err(error) => {
            issues.push(
                Issue::new(
                    "WORKSPACE_CACHE_CORRUPT",
                    None,
                    format!("could not read the Remote Issue catalog: {}", error.message),
                )
                .with_cause("the Remote Issue cache contains invalid structure or JSON")
                .with_next_action(format!(
                    "inspect or move aside {}, then run `orbit workspace issues refresh`",
                    path.display()
                )),
            );
            return None;
        }
    };
    let included = repositories
        .map(|catalog| {
            catalog
                .repositories
                .values()
                .filter(|entry| entry.included)
                .map(|entry| entry.repository.clone())
                .collect::<std::collections::BTreeSet<_>>()
        })
        .unwrap_or_default();
    if !path.exists() {
        warnings.push(format!(
            "{}: Remote Issue catalog has not been refreshed yet; run `orbit workspace issues refresh`",
            workspace.display()
        ));
        return Some(catalog);
    }
    for repository in &included {
        let Some(cached) = catalog.repositories.get(repository) else {
            warnings.push(format!(
                "{repository}: Remote Issues have not been cached; run `orbit workspace issues refresh`"
            ));
            continue;
        };
        if let Some(error) = &cached.error {
            let stale = cached.last_refreshed_at.is_some();
            issues.push(
                if stale {
                    Issue::warning(
                        "WORKSPACE_ISSUES_STALE",
                        format!("Remote Issue cache for {repository} is stale after a failed refresh"),
                    )
                } else {
                    Issue::new(
                        "WORKSPACE_ISSUES_UNAVAILABLE",
                        None,
                        format!("Remote Issue cache for {repository} is unavailable"),
                    )
                }
                .with_repository(repository.clone())
                .with_cause(error.clone())
                .with_next_action("check GitHub access, then rerun `orbit workspace issues refresh`; the previous snapshot is retained"),
            );
        } else if cached.last_refreshed_at.is_none() {
            warnings.push(format!(
                "{repository}: Remote Issues have no successful refresh; run `orbit workspace issues refresh`"
            ));
        } else if is_stale(cached.last_refreshed_at) {
            issues.push(
                Issue::warning(
                    "WORKSPACE_ISSUES_STALE",
                    format!("Remote Issue cache for {repository} is older than 24 hours"),
                )
                .with_repository(repository.clone())
                .with_cause("the catalog refreshes only on explicit command")
                .with_next_action("run `orbit workspace issues refresh`"),
            );
        }
    }
    Some(catalog)
}

fn diagnose_migration(workspace: &Path, issues: &mut Vec<Issue>, warnings: &mut Vec<String>) {
    let orbit = workspace.join(".orbit");
    let state = orbit.join("workspace-migration-state.json");
    let stage = orbit.join(".workspace-board-migration-stage");
    if state.exists() {
        issues.push(
            Issue::new(
                "WORKSPACE_MIGRATION_INCOMPLETE",
                None,
                "an interrupted Workspace Board migration has a saved state file",
            )
            .with_repository(workspace.display().to_string())
            .with_cause(format!("{} exists", state.display()))
            .with_next_action("review the saved plan with `orbit workspace migrate --resume --dry-run`, then resume or roll back"),
        );
    }
    if stage.exists() && !state.exists() {
        issues.push(
            Issue::warning(
                "WORKSPACE_MIGRATION_STAGE",
                "a Workspace migration staging Board exists without a saved plan",
            )
            .with_repository(workspace.display().to_string())
            .with_cause(format!("{} exists", stage.display()))
            .with_next_action("inspect the staging Board and remove it only after confirming no migration is pending"),
        );
    }
    for source in local_boards(workspace) {
        warnings.push(format!(
            "local Board at {} remains beside the Workspace Board; run `orbit workspace migrate --dry-run` to review any unmigrated Work, or keep it as the documented backup",
            source.display()
        ));
    }
}

fn local_boards(workspace: &Path) -> Vec<PathBuf> {
    let mut found = Vec::new();
    for candidate in [
        workspace.join(".board"),
        workspace.join("board"),
        workspace.join(".orbit"),
    ] {
        if Board::is_board(&candidate) {
            found.push(dunce::canonicalize(&candidate).unwrap_or(candidate));
        }
    }
    if let Ok(children) = std::fs::read_dir(workspace) {
        for child in children.flatten() {
            let path = child.path();
            if !path.is_dir() || child.file_name() == ".orbit" {
                continue;
            }
            if path.join(".git").exists() {
                let local = path.join(".orbit");
                if Board::is_board(&local) {
                    found.push(dunce::canonicalize(&local).unwrap_or(local));
                }
                if let Ok(common) = crate::git::common_dir_at(&path) {
                    let legacy = common.join("orbit/board");
                    if Board::is_board(&legacy) {
                        found.push(dunce::canonicalize(&legacy).unwrap_or(legacy));
                    }
                }
            }
        }
    }
    found.sort();
    found.dedup();
    found
}

fn diagnose_links(
    board: &Board,
    issue_catalog: Option<&crate::issue_catalog::IssueCatalog>,
    repositories: Option<&crate::workspace::RepositoryCatalog>,
    issues: &mut Vec<Issue>,
    warnings: &mut Vec<String>,
) {
    let graph = match Graph::load(board, LoadMode::Lenient) {
        Ok((graph, _)) => graph,
        Err(error) => {
            issues.push(
                Issue::new(
                    "WORKSPACE_LINK_DIAGNOSTICS",
                    None,
                    format!("could not inspect Linked Tasks: {}", error.message),
                )
                .with_cause("the Board graph is not loadable")
                .with_next_action("fix the reported Task parsing issues first"),
            );
            return;
        }
    };
    let included = repositories
        .map(|catalog| {
            catalog
                .repositories
                .values()
                .filter(|entry| entry.included)
                .map(|entry| entry.repository.to_ascii_lowercase())
                .collect::<std::collections::BTreeSet<_>>()
        })
        .unwrap_or_default();
    let mut links: BTreeMap<String, Vec<(String, bool)>> = BTreeMap::new();
    for task in graph.tasks.values() {
        if let Some(link) = task.front.github.as_ref() {
            links
                .entry(format!(
                    "{}#{}",
                    link.repo.to_ascii_lowercase(),
                    link.number
                ))
                .or_default()
                .push((task.id.clone(), link.active));
        }
    }
    for (identity, holders) in &links {
        if holders.len() > 1 {
            let tasks = holders
                .iter()
                .map(|(task, active)| format!("{task}{}", if *active { "" } else { " (inactive)" }))
                .collect::<Vec<_>>()
                .join(", ");
            let active = holders.iter().any(|(_, active)| *active);
            issues.push(
                if active {
                    Issue::new(
                        "GITHUB_DUPLICATE_LINK",
                        None,
                        format!("GitHub identity {identity} is linked to multiple Tasks: {tasks}"),
                    )
                } else {
                    Issue::warning(
                        "GITHUB_DUPLICATE_LINK",
                        format!(
                            "GitHub provenance {identity} is retained by multiple Tasks: {tasks}"
                        ),
                    )
                }
                .with_repository(identity.split('#').next().unwrap_or(identity))
                .with_issue(identity.clone())
                .with_cause(
                    "Link uniqueness is by repository and Issue number; titles are never used",
                )
                .with_next_action(
                    "detach or reattach one Task explicitly; doctor will not merge or delete work",
                ),
            );
        }
    }

    for task in graph.tasks.values() {
        let Some(link) = task.front.github.as_ref().filter(|link| link.active) else {
            continue;
        };
        let identity = format!("{}#{}", link.repo.to_ascii_lowercase(), link.number);
        if !included.is_empty() && !included.contains(&link.repo.to_ascii_lowercase()) {
            issues.push(
                Issue::warning(
                    "GITHUB_ORPHAN_LINK",
                    format!(
                        "{} links to {identity}, but its repository is not currently included",
                        task.id
                    ),
                )
                .with_task(task.id.clone())
                .with_repository(link.repo.clone())
                .with_issue(identity.clone())
                .with_cause("the repository was detached or excluded after the Task was linked")
                .with_next_action("reattach the repository or detach the Linked Task explicitly"),
            );
            continue;
        }
        let Some(catalog) = issue_catalog else {
            continue;
        };
        let Some(cached) = catalog
            .repositories
            .get(&link.repo.to_ascii_lowercase())
            .and_then(|repository| {
                repository
                    .issues
                    .iter()
                    .find(|issue| issue.number == link.number)
            })
        else {
            issues.push(
                Issue::warning(
                    "GITHUB_ORPHAN_LINK",
                    format!("{} links to {identity}, which is absent from the Remote Issue cache", task.id),
                )
                .with_task(task.id.clone())
                .with_repository(link.repo.clone())
                .with_issue(identity.clone())
                .with_cause("the cache is stale, the Issue was removed, or the repository is detached")
                .with_next_action("run `orbit workspace issues refresh`, then inspect the Identity with `orbit workspace issues inspect`"),
            );
            continue;
        };
        let Some(baseline) = link.last_sync.as_ref() else {
            warnings.push(format!(
                "{} links to {identity} without a sync baseline; `orbit github sync` will need an explicit preview",
                task.id
            ));
            continue;
        };
        let mut conflicts = Vec::new();
        if task.front.title != baseline.title && cached.title != baseline.title {
            conflicts.push("title");
        }
        let local_labels = normalized_labels(&task.front.labels);
        let cached_labels = normalized_labels(&cached.labels);
        if local_labels != normalized_labels(&baseline.labels)
            && cached_labels != normalized_labels(&baseline.labels)
        {
            conflicts.push("labels");
        }
        let cached_status = status_from_issue(cached.state, &cached.labels);
        let local_status_changed = baseline
            .status
            .is_some_and(|status| status != task.front.status);
        let remote_status_changed = baseline
            .issue_state
            .is_some_and(|state| state != cached.state);
        if local_status_changed && remote_status_changed {
            conflicts.push("status");
        }
        if !conflicts.is_empty() {
            issues.push(
                Issue::warning(
                    "GITHUB_SYNC_CONFLICT",
                    format!("{} has field-level sync conflicts for {identity}", task.id),
                )
                .with_task(task.id.clone())
                .with_repository(link.repo.clone())
                .with_issue(identity.clone())
                .with_cause(format!(
                    "both Orbit and the cached GitHub Issue changed: {}",
                    conflicts.join(", ")
                ))
                .with_next_action("run `orbit github sync <task>` to choose `--resolve FIELD=orbit|github`; doctor never overwrites either side"),
            );
        }
        if task.front.status == Status::Todo && cached_status == Status::Done {
            warnings.push(format!(
                "{} is todo but cached Remote Issue {identity} is closed; sync will need an explicit status decision",
                task.id
            ));
        }
    }
}

fn normalized_labels(labels: &[String]) -> Vec<String> {
    let mut labels = labels
        .iter()
        .map(|label| label.to_ascii_lowercase())
        .collect::<Vec<_>>();
    labels.sort();
    labels.dedup();
    labels
}

fn diagnose_agent_setup(
    workspace: &Path,
    board: &Board,
    repositories: Option<&crate::workspace::RepositoryCatalog>,
    issues: &mut Vec<Issue>,
    warnings: &mut Vec<String>,
) {
    let expected = board
        .config
        .workspace
        .agent_harnesses
        .as_ref()
        .map(|names| {
            names
                .iter()
                .filter_map(|name| crate::setup::SetupClient::from_name(name))
                .collect::<Vec<_>>()
        });
    let configured_repositories = repositories
        .map(|catalog| {
            catalog
                .repositories
                .values()
                .filter(|entry| entry.included && entry.discovered)
                .map(|entry| (entry.repository.clone(), workspace.join(&entry.path)))
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    let expects_workspace = expected.as_ref().is_some_and(|clients| !clients.is_empty())
        || configured_repositories.iter().any(|(_, path)| {
            crate::setup::configured_clients(path)
                .iter()
                .any(|client| client.is_harness())
        });
    if expects_workspace {
        match crate::setup::workspace_guidance(workspace, true, false) {
            Ok(output) => {
                record_setup_preview("Workspace root", workspace, &output, issues, warnings)
            }
            Err(error) => issues.push(
                Issue::new(
                    "WORKSPACE_AGENT_CONFIG",
                    None,
                    format!(
                        "could not inspect Workspace agent guidance: {}",
                        error.message
                    ),
                )
                .with_repository(workspace.display().to_string())
                .with_cause("a managed Workspace instruction file is malformed")
                .with_next_action("run `orbit workspace setup --print` to preview the repair"),
            ),
        }
    }

    for (repository, path) in &configured_repositories {
        if !path.is_dir() {
            continue;
        }
        let clients = match &expected {
            Some(expected) => expected.clone(),
            None => crate::setup::configured_clients(path)
                .into_iter()
                .filter(|client| client.is_harness())
                .collect(),
        };
        if clients.is_empty() {
            continue;
        }
        check_repository_setup(repository, path, clients, issues);
        let matt = crate::setup::detects_mattpocock_skills(path);
        if matt {
            check_matt_setup(repository, path, issues, warnings);
        }
    }
    if crate::setup::detects_mattpocock_skills(workspace) {
        check_matt_setup("Workspace root", workspace, issues, warnings);
    }
}

fn check_repository_setup(
    repository: &str,
    path: &Path,
    clients: Vec<crate::setup::SetupClient>,
    issues: &mut Vec<Issue>,
) {
    let mut changed = std::collections::BTreeSet::new();
    let mut skipped = std::collections::BTreeSet::new();
    for client in clients {
        // Compare against the command mode already recorded for this harness.
        // Falling back to the portable command makes a missing entry detectable
        // without flagging a legitimate absolute Orbit binary as drift.
        let mcp_bin = crate::setup::configured_mcp_command(path, client)
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from("orbit"));
        let preview = crate::setup::install(
            path,
            crate::setup::InstallOptions {
                scope: crate::setup::SetupScope::Project,
                clients: vec![client],
                hooks: false,
                hook_bin: None,
                mcp_bin: Some(mcp_bin),
                print: true,
                remove: false,
            },
        );
        match preview {
            Ok(output) => {
                let files = output
                    .data
                    .get("files")
                    .and_then(serde_json::Value::as_array)
                    .cloned()
                    .unwrap_or_default();
                for file in files {
                    if let Some(action) = file["action"].as_str()
                        && let Some(file_path) = file["path"].as_str()
                    {
                        match action {
                            "created" | "updated" => {
                                changed.insert(file_path.to_string());
                            }
                            "skipped" => {
                                skipped.insert(file_path.to_string());
                            }
                            _ => {}
                        }
                    }
                }
            }
            Err(error) => {
                issues.push(
                    Issue::new(
                        "WORKSPACE_AGENT_CONFIG",
                        None,
                        format!(
                            "could not inspect agent setup in {repository}: {}",
                            error.message
                        ),
                    )
                    .with_repository(repository.to_string())
                    .with_cause("an agent configuration file is malformed")
                    .with_next_action(
                        "repair or move the malformed file, then run `orbit workspace setup --print`",
                    ),
                );
                return;
            }
        }
    }
    if !changed.is_empty() {
        let changed = changed.into_iter().collect::<Vec<_>>();
        issues.push(
            Issue::warning(
                "WORKSPACE_AGENT_DRIFT",
                format!(
                    "managed agent setup in {repository} is missing or stale: {}",
                    changed.join(", ")
                ),
            )
            .with_repository(repository.to_string())
            .with_cause(
                "managed instructions or MCP configuration no longer match the selected harnesses",
            )
            .with_next_action(
                "run `orbit workspace setup --print`, then apply the selected harness integrations",
            ),
        );
    }
    if !skipped.is_empty() {
        let skipped = skipped.into_iter().collect::<Vec<_>>();
        issues.push(
            Issue::new(
                "WORKSPACE_AGENT_CONFLICT",
                None,
                format!(
                    "{} keeps unmanaged agent content; Orbit did not overwrite it",
                    skipped.join(", ")
                ),
            )
            .with_repository(repository.to_string())
            .with_cause("the file exists without an Orbit-managed section")
            .with_next_action("review the custom file, then run `orbit workspace setup --print` and choose a safe merge"),
        );
    }
    for config in crate::doctor::absolute_board_configs(path) {
        issues.push(
            Issue::warning(
                "ABSOLUTE_BOARD_PATH",
                format!(
                    "{} records an absolute Workspace or Board path",
                    config.display()
                ),
            )
            .with_repository(repository.to_string())
            .with_cause("managed MCP configuration must resolve the shared Workspace Board by discovery")
            .with_next_action("run `orbit workspace setup --print`, then re-run setup to rewrite the entry path-free"),
        );
    }
}

fn check_matt_setup(
    repository: &str,
    path: &Path,
    issues: &mut Vec<Issue>,
    warnings: &mut Vec<String>,
) {
    let preview = crate::setup::install(
        path,
        crate::setup::InstallOptions {
            scope: crate::setup::SetupScope::Project,
            clients: vec![crate::setup::SetupClient::Mattpocock],
            hooks: false,
            hook_bin: None,
            mcp_bin: None,
            print: true,
            remove: false,
        },
    );
    match preview {
        Ok(output) => {
            let files = output
                .data
                .get("files")
                .and_then(serde_json::Value::as_array)
                .cloned()
                .unwrap_or_default();
            let changed = files
                .iter()
                .filter(|file| matches!(file["action"].as_str(), Some("created" | "updated")))
                .filter_map(|file| file["path"].as_str().map(str::to_string))
                .collect::<Vec<_>>();
            if !changed.is_empty() {
                warnings.push(format!(
                    "{repository}: Matt Pocock skills are present but Orbit tracker guidance is missing or stale: {}; run `orbit workspace setup`",
                    changed.join(", ")
                ));
            }
            if let Some(conflict) = files.iter().find(|file| file["action"] == "skipped") {
                issues.push(
                    Issue::warning(
                        "MATTPOCOCK_SETUP_DRIFT",
                        format!(
                            "{repository} keeps custom tracker guidance at {}",
                            conflict["path"].as_str().unwrap_or("docs/agents")
                        ),
                    )
                    .with_repository(repository.to_string())
                    .with_cause("the Matt skills integration is detected but its Orbit guidance is custom")
                    .with_next_action("review the custom guidance; Orbit will not overwrite it"),
                );
            }
        }
        Err(error) => warnings.push(format!(
            "{repository}: Matt Pocock skills were detected but Orbit could not inspect their tracker guidance: {}",
            error.message
        )),
    }
}

fn record_setup_preview(
    repository: &str,
    path: &Path,
    output: &crate::output::CommandOutput,
    issues: &mut Vec<Issue>,
    warnings: &mut Vec<String>,
) {
    let files = output
        .data
        .get("files")
        .and_then(serde_json::Value::as_array)
        .cloned()
        .unwrap_or_default();
    let changed = files
        .iter()
        .filter(|file| matches!(file["action"].as_str(), Some("created" | "updated")))
        .filter_map(|file| file["path"].as_str().map(str::to_string))
        .collect::<Vec<_>>();
    if !changed.is_empty() {
        issues.push(
            Issue::warning(
                "WORKSPACE_AGENT_DRIFT",
                format!(
                    "managed agent guidance at {repository} is missing or stale: {}",
                    changed.join(", ")
                ),
            )
            .with_repository(repository.to_string())
            .with_cause("Workspace instructions do not match the selected harnesses")
            .with_next_action(
                "run `orbit workspace setup --print`, then apply the managed guidance",
            ),
        );
    }
    warnings.extend(
        output
            .warnings
            .iter()
            .map(|warning| format!("{}: {warning}", path.display())),
    );
}

fn is_stale(timestamp: Option<DateTime<Utc>>) -> bool {
    timestamp
        .is_none_or(|timestamp| Utc::now() - timestamp > Duration::hours(CACHE_STALE_AFTER_HOURS))
}

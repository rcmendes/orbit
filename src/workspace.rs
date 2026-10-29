use std::collections::{BTreeMap, BTreeSet};
use std::io::IsTerminal;
use std::path::{Path, PathBuf};
use std::process::Command;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::json;

use crate::board::Board;
use crate::cli::{WorkspaceArgs, WorkspaceCommand, WorkspaceSetupArgs};
use crate::error::{OrbitError, Result, code};
use crate::github::client::client_from_env;

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct WorkspaceConfig {
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub include: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub exclude: Vec<String>,
    /// Repositories detached locally without changing the user's include filter.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub detached: Vec<String>,
    /// Harness selection reused by later `workspace setup` reconciliations.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agent_harnesses: Option<Vec<String>>,
}

impl WorkspaceConfig {
    pub fn is_empty(&self) -> bool {
        self.include.is_empty()
            && self.exclude.is_empty()
            && self.detached.is_empty()
            && self.agent_harnesses.is_none()
    }

    pub fn validate(&self) -> Result<()> {
        for repository in self
            .include
            .iter()
            .chain(&self.exclude)
            .chain(&self.detached)
        {
            crate::github::GitHubConfig::normalize_repo(repository)?;
        }
        let included = self
            .include
            .iter()
            .map(|repo| repo.to_ascii_lowercase())
            .collect::<BTreeSet<_>>();
        if self
            .exclude
            .iter()
            .any(|repo| included.contains(&repo.to_ascii_lowercase()))
        {
            return Err(OrbitError::config(
                "workspace.include and workspace.exclude cannot contain the same repository",
            ));
        }
        let mut detached = BTreeSet::new();
        if self
            .detached
            .iter()
            .any(|repo| !detached.insert(repo.to_ascii_lowercase()))
        {
            return Err(OrbitError::config(
                "workspace.detached must not contain duplicate repositories",
            ));
        }
        if let Some(harnesses) = &self.agent_harnesses {
            let mut seen = BTreeSet::new();
            for harness in harnesses {
                if crate::setup::SetupClient::harness_from_name(harness).is_none() {
                    return Err(OrbitError::config(format!(
                        "unsupported Workspace agent harness `{harness}`"
                    )));
                }
                if !seen.insert(harness) {
                    return Err(OrbitError::config(format!(
                        "duplicate Workspace agent harness `{harness}`"
                    )));
                }
            }
        }
        Ok(())
    }

    fn is_included(&self, repository: &str) -> bool {
        self.include
            .iter()
            .any(|item| item.eq_ignore_ascii_case(repository))
    }

    fn has_include_filter(&self) -> bool {
        !self.include.is_empty()
    }

    fn is_excluded(&self, repository: &str) -> bool {
        self.exclude
            .iter()
            .any(|item| item.eq_ignore_ascii_case(repository))
            || self
                .detached
                .iter()
                .any(|item| item.eq_ignore_ascii_case(repository))
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub(crate) struct RepositoryCatalog {
    #[serde(default)]
    pub(crate) organization: Option<String>,
    #[serde(default)]
    pub(crate) last_refreshed_at: Option<DateTime<Utc>>,
    #[serde(default)]
    pub(crate) repositories: BTreeMap<String, RepositoryEntry>,
    #[serde(default)]
    pub(crate) non_git_directories: Vec<String>,
    #[serde(default)]
    pub(crate) missing_origins: Vec<String>,
    #[serde(default)]
    pub(crate) unsupported_origins: Vec<UnsupportedOrigin>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct RepositoryEntry {
    pub(crate) repository: String,
    pub(crate) path: String,
    pub(crate) discovered: bool,
    pub(crate) status: RepositoryStatus,
    pub(crate) included: bool,
    #[serde(default)]
    pub(crate) cached: bool,
    pub(crate) origin: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) error: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) last_checked_at: Option<DateTime<Utc>>,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub(crate) enum RepositoryStatus {
    Included,
    Excluded,
    Inaccessible,
    Stale,
    Removed,
    UnsupportedOwner,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct UnsupportedOrigin {
    pub(crate) path: String,
    pub(crate) origin: String,
    pub(crate) reason: String,
}

pub fn run(board: &Board, args: WorkspaceArgs) -> Result<crate::output::CommandOutput> {
    let workspace = workspace_root(board)?;
    match args.command {
        WorkspaceCommand::Migrate(_) => Err(OrbitError::invalid_argument(
            "`workspace migrate` does not run against an already-resolved Board",
        )),
        WorkspaceCommand::Status => status(&workspace),
        WorkspaceCommand::Refresh => refresh(board, &workspace),
        WorkspaceCommand::Include(args) => set_override(board, &args.repository, true),
        WorkspaceCommand::Exclude(args) => set_override(board, &args.repository, false),
        WorkspaceCommand::Detach(args) => {
            crate::issue_catalog::detach_repository(board, &workspace, args)
        }
        WorkspaceCommand::Reattach(args) => {
            crate::issue_catalog::reattach_repository(board, &workspace, args)
        }
        WorkspaceCommand::Setup(args) => setup(board, &workspace, args),
        WorkspaceCommand::Issues(args) => crate::issue_catalog::run(board, args.command),
    }
}

pub(crate) fn workspace_root(board: &Board) -> Result<PathBuf> {
    workspace_root_if_board(board).ok_or_else(|| {
        OrbitError::invalid_argument(
            "`orbit workspace` requires a Workspace Board at `<workspace>/.orbit/board`",
        )
    })
}

pub(crate) fn workspace_root_if_board(board: &Board) -> Option<PathBuf> {
    if !board.is_workspace_board() {
        return None;
    }
    let workspace = board.root.parent()?.parent()?;
    Some(dunce::canonicalize(workspace).unwrap_or_else(|_| workspace.to_path_buf()))
}

/// Finds the nearest Workspace folder that contains Workspace-only state. A
/// regular Board at a child repository's `.orbit` is skipped so `doctor` can
/// still identify the enclosing Workspace across the Git boundary.
pub(crate) fn candidate_workspace_root(start: &Path) -> Option<PathBuf> {
    let start = dunce::canonicalize(start).unwrap_or_else(|_| start.to_path_buf());
    for ancestor in start.ancestors() {
        let orbit = ancestor.join(".orbit");
        if orbit.join("board").is_dir()
            || orbit.join("cache").exists()
            || orbit.join("workspace-migration-state.json").exists()
            || orbit.join(".workspace-board-migration-stage").exists()
        {
            return Some(ancestor.to_path_buf());
        }
        if orbit.is_dir()
            && !Board::is_board(&orbit)
            && crate::git::toplevel_containing(ancestor).is_none()
        {
            return Some(ancestor.to_path_buf());
        }
    }
    None
}

fn setup(
    board: &Board,
    workspace: &Path,
    args: WorkspaceSetupArgs,
) -> Result<crate::output::CommandOutput> {
    let catalog = load_catalog(&catalog_path(workspace))?;
    if catalog.last_refreshed_at.is_none() {
        return Err(OrbitError::invalid_argument(
            "Workspace repositories have not been discovered; run `orbit workspace refresh` before setup",
        ));
    }
    let repositories = catalog
        .repositories
        .values()
        .filter(|entry| entry.discovered && (args.remove || entry.included))
        .filter_map(|entry| {
            let relative = Path::new(&entry.path);
            if relative.components().count() != 1
                || relative
                    .components()
                    .any(|part| !matches!(part, std::path::Component::Normal(_)))
            {
                return None;
            }
            let root = workspace.join(relative);
            is_direct_git_repository(&root).then(|| (entry.repository.clone(), root))
        })
        .collect::<Vec<_>>();

    let detected = detected_workspace_harnesses(&repositories);
    let selected = if args.remove {
        args.agents
            .as_deref()
            .map(|selector| parse_workspace_harnesses(selector, &detected, args.print))
            .transpose()?
            .unwrap_or_else(all_workspace_harnesses)
    } else if let Some(selector) = args.agents.as_deref() {
        parse_workspace_harnesses(selector, &detected, args.print)?
    } else if let Some(saved) = &board.config.workspace.agent_harnesses {
        harnesses_from_names(saved)?
    } else if args.print {
        detected.clone()
    } else if std::io::stdin().is_terminal() {
        crate::setup::prompt_workspace_harnesses(&detected)?
    } else {
        detected.clone()
    };

    let mut file_results = Vec::new();
    let mut warnings = Vec::new();
    let workspace_mattpocock = crate::setup::detects_mattpocock_skills(workspace);
    let workspace_files = crate::setup::workspace_guidance(workspace, args.print, args.remove)?;
    collect_setup_output(None, workspace_files, &mut file_results, &mut warnings);
    if workspace_mattpocock || args.remove {
        let mattpocock_files = crate::setup::install(
            workspace,
            crate::setup::InstallOptions {
                scope: crate::setup::SetupScope::Project,
                clients: vec![crate::setup::SetupClient::Mattpocock],
                hooks: false,
                hook_bin: None,
                mcp_bin: None,
                print: args.print,
                remove: args.remove,
            },
        )?;
        collect_setup_output(None, mattpocock_files, &mut file_results, &mut warnings);
    }

    let mcp_bin = if !args.remove && !args.no_mcp && selected.iter().any(|client| client.uses_mcp())
    {
        Some(crate::setup::configured_mcp_bin(
            args.portable_mcp,
            args.mcp_bin.as_deref(),
        )?)
    } else {
        None
    };
    let hook_bin =
        if !args.remove && args.hooks && selected.contains(&crate::setup::SetupClient::Claude) {
            Some(crate::setup::configured_hook_bin(args.mcp_bin.as_deref())?)
        } else {
            None
        };

    let mut repository_results = Vec::new();
    for (repository, root) in &repositories {
        let mattpocock = crate::setup::detects_mattpocock_skills(root);
        let mut clients = if args.remove {
            all_workspace_harnesses()
        } else {
            selected.clone()
        };
        if mattpocock || args.remove {
            clients.push(crate::setup::SetupClient::Mattpocock);
        }
        clients.sort_by_key(|client| client.name());
        clients.dedup();
        if clients.is_empty() {
            repository_results.push(json!({
                "repository": repository,
                "path": root,
                "agents": [],
                "mattpocock_skills": false,
                "files": [],
            }));
            continue;
        }
        let installed = crate::setup::install(
            root,
            crate::setup::InstallOptions {
                scope: crate::setup::SetupScope::Project,
                clients: clients.clone(),
                hooks: args.hooks,
                hook_bin: hook_bin.clone(),
                mcp_bin: mcp_bin.clone(),
                print: args.print,
                remove: args.remove,
            },
        )?;
        let files = collect_setup_output(
            Some(repository),
            installed,
            &mut file_results,
            &mut warnings,
        );
        repository_results.push(json!({
            "repository": repository,
            "path": root,
            "agents": selected.iter().map(|client| client.name()).collect::<Vec<_>>(),
            "mattpocock_skills": mattpocock,
            "files": files,
        }));
    }

    if !args.print {
        board.save_config(
            if args.remove {
                "orbit: remove Workspace agent integrations"
            } else {
                "orbit: configure Workspace agent integrations"
            },
            |config| {
                config.workspace.agent_harnesses = if args.remove {
                    None
                } else {
                    Some(
                        selected
                            .iter()
                            .map(|client| client.name().to_string())
                            .collect(),
                    )
                };
                Ok(())
            },
        )?;
    }

    let selected_names = selected
        .iter()
        .map(|client| client.name())
        .collect::<Vec<_>>();
    let mut human = if args.print {
        "Workspace agent setup preview (nothing was written).\n".to_string()
    } else if args.remove {
        "Removed Orbit-managed Workspace agent integrations.\n".to_string()
    } else {
        "Workspace agent integrations reconciled.\n".to_string()
    };
    human.push_str(&format!("Harnesses: {}\n", selected_names.join(", ")));
    for repository in &repository_results {
        human.push_str(&format!(
            "{}: {} files planned\n",
            repository["repository"].as_str().unwrap_or("unknown"),
            repository["files"].as_array().map_or(0, Vec::len)
        ));
    }
    for file in &file_results {
        human.push_str(&format!(
            "{} {}\n",
            file["action"].as_str().unwrap_or("unknown"),
            file["path"].as_str().unwrap_or("")
        ));
    }
    let mut output = crate::output::CommandOutput::new(
        json!({
            "workspace": workspace,
            "preview": args.print,
            "removed": args.remove,
            "agents": selected_names,
            "mattpocock_skills": workspace_mattpocock,
            "repositories": repository_results,
            "files": file_results,
        }),
        human,
    );
    warnings.sort();
    warnings.dedup();
    output.warnings = warnings;
    Ok(output)
}

fn collect_setup_output(
    repository: Option<&str>,
    output: crate::output::CommandOutput,
    combined: &mut Vec<serde_json::Value>,
    warnings: &mut Vec<String>,
) -> Vec<serde_json::Value> {
    let mut files = Vec::new();
    if let Some(entries) = output.data["files"].as_array() {
        for entry in entries {
            let mut file = entry.clone();
            file["repository"] = json!(repository);
            files.push(file.clone());
            combined.push(file);
        }
    }
    warnings.extend(output.warnings);
    files
}

fn all_workspace_harnesses() -> Vec<crate::setup::SetupClient> {
    crate::setup::SetupClient::harnesses()
}

fn detected_workspace_harnesses(
    repositories: &[(String, PathBuf)],
) -> Vec<crate::setup::SetupClient> {
    let mut detected = Vec::new();
    for (_, root) in repositories {
        for client in crate::setup::detect_harnesses(root) {
            if !detected.contains(&client) {
                detected.push(client);
            }
        }
    }
    detected
}

fn harnesses_from_names(names: &[String]) -> Result<Vec<crate::setup::SetupClient>> {
    names
        .iter()
        .map(|name| {
            crate::setup::SetupClient::harness_from_name(name).ok_or_else(|| {
                OrbitError::config(format!("unsupported Workspace agent harness `{name}`"))
            })
        })
        .collect()
}

fn parse_workspace_harnesses(
    selector: &str,
    detected: &[crate::setup::SetupClient],
    preview: bool,
) -> Result<Vec<crate::setup::SetupClient>> {
    match selector.trim() {
        "auto" => Ok(detected.to_vec()),
        "all" => Ok(all_workspace_harnesses()),
        "none" => Ok(Vec::new()),
        "ask" if preview => Ok(detected.to_vec()),
        "ask" => crate::setup::prompt_workspace_harnesses(detected),
        "" => Err(OrbitError::invalid_argument(
            "`--agents` must be auto, all, none, ask, or a comma-separated harness list",
        )),
        names => {
            let mut clients = Vec::new();
            for name in names.split(|character: char| character == ',' || character.is_whitespace())
            {
                if name.is_empty() {
                    continue;
                }
                let client = if name == "mattpocock" {
                    return Err(OrbitError::invalid_argument(
                        "Matt Pocock skills are detected and configured separately; do not include `mattpocock` in `--agents`",
                    ));
                } else {
                    crate::setup::SetupClient::harness_from_name(name).ok_or_else(|| {
                        OrbitError::invalid_argument(format!(
                            "unknown harness `{name}`; expected claude, cursor, codex, or opencode"
                        ))
                    })?
                };
                if !clients.contains(&client) {
                    clients.push(client);
                }
            }
            Ok(clients)
        }
    }
}

pub(crate) fn included_repositories(workspace: &Path) -> Result<Vec<String>> {
    let catalog = load_catalog(&catalog_path(workspace))?;
    Ok(catalog
        .repositories
        .values()
        .filter(|entry| entry.included && entry.status != RepositoryStatus::Removed)
        .map(|entry| entry.repository.clone())
        .collect())
}

pub(crate) fn catalog_path(workspace: &Path) -> PathBuf {
    workspace
        .join(".orbit")
        .join("cache")
        .join("repositories.json")
}

pub(crate) fn catalog_for_diagnostics(workspace: &Path) -> Result<RepositoryCatalog> {
    load_catalog(&catalog_path(workspace))
}

/// Return repositories available for issue creation from the Workspace catalog.
/// `None` means that this Board is not a Workspace Board.
pub(crate) fn export_repositories(board: &Board) -> Result<Option<Vec<String>>> {
    if !board.is_workspace_board() {
        return Ok(None);
    }
    let workspace = workspace_root(board)?;
    let catalog = load_catalog(&catalog_path(&workspace))?;
    Ok(Some(
        catalog
            .repositories
            .values()
            .filter(|entry| entry.included)
            .map(|entry| entry.repository.clone())
            .collect(),
    ))
}

fn load_catalog(path: &Path) -> Result<RepositoryCatalog> {
    match std::fs::read(path) {
        Ok(bytes) => serde_json::from_slice(&bytes)
            .map_err(|error| OrbitError::config(format!("invalid {}: {error}", path.display()))),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            Ok(RepositoryCatalog::default())
        }
        Err(error) => Err(OrbitError::io(format!("read {}", path.display()), error)),
    }
}

fn save_catalog(path: &Path, catalog: &RepositoryCatalog) -> Result<()> {
    let parent = path
        .parent()
        .ok_or_else(|| OrbitError::config("Workspace cache has no parent directory"))?;
    std::fs::create_dir_all(parent)
        .map_err(|error| OrbitError::io(format!("create {}", parent.display()), error))?;
    let contents = serde_json::to_vec_pretty(catalog)
        .map_err(|error| OrbitError::config(format!("serialize Workspace catalog: {error}")))?;
    crate::atomic::write_atomic(path, &contents, true)
}

fn status(workspace: &Path) -> Result<crate::output::CommandOutput> {
    let catalog = load_catalog(&catalog_path(workspace))?;
    let repositories = catalog.repositories.values().collect::<Vec<_>>();
    let data = json!({
        "workspace": workspace,
        "organization": catalog.organization,
        "last_refreshed_at": catalog.last_refreshed_at,
        "repositories": repositories,
        "non_git_directories": catalog.non_git_directories,
        "missing_origins": catalog.missing_origins,
        "unsupported_origins": catalog.unsupported_origins,
    });
    let mut lines = vec![format!("Workspace: {}", workspace.display())];
    lines.push(format!(
        "Organization: {}",
        catalog.organization.as_deref().unwrap_or("not configured")
    ));
    lines.push(format!(
        "Last refresh: {}",
        catalog
            .last_refreshed_at
            .map(|time| time.to_rfc3339())
            .unwrap_or_else(|| "never (run `orbit workspace refresh`)".to_string())
    ));
    if repositories.is_empty() {
        lines.push("Repositories: none discovered".to_string());
    } else {
        lines.push("Repositories:".to_string());
        lines.extend(repositories.iter().map(|entry| {
            let error = entry
                .error
                .as_deref()
                .map(|message| format!(" — {message}"))
                .unwrap_or_default();
            format!(
                "  {} [{}] {}{}",
                entry.repository,
                format_status(entry.status),
                entry.path,
                error
            )
        }));
    }
    if !catalog.non_git_directories.is_empty() {
        lines.push(format!(
            "Non-Git directories: {}",
            catalog.non_git_directories.join(", ")
        ));
    }
    if !catalog.missing_origins.is_empty() {
        lines.push(format!(
            "Missing origin: {}",
            catalog.missing_origins.join(", ")
        ));
    }
    for origin in &catalog.unsupported_origins {
        lines.push(format!(
            "Unsupported origin in {}: {} ({})",
            origin.path, origin.origin, origin.reason
        ));
    }
    Ok(crate::output::CommandOutput::new(data, lines.join("\n")))
}

pub(crate) fn refresh(board: &Board, workspace: &Path) -> Result<crate::output::CommandOutput> {
    if board.config.github.mode.is_basic() {
        return Err(OrbitError::invalid_argument(
            "Workspace repository discovery requires GitHub full mode and one configured organization",
        ));
    }
    let organization = board
        .config
        .github
        .organization
        .as_deref()
        .filter(|org| !org.trim().is_empty())
        .ok_or_else(|| {
            OrbitError::new(
                code::GITHUB_NOT_CONFIGURED,
                "Workspace repository discovery needs `[github].organization`; configure it with `orbit github connect` from an organization-owned child repository",
            )
        })?
        .to_string();
    let client = client_from_env()?;
    let path = catalog_path(workspace);
    let mut catalog = load_catalog(&path)?;
    let previous = catalog.repositories.clone();
    let mut current = BTreeMap::new();
    let mut non_git_directories = Vec::new();
    let mut missing_origins = Vec::new();
    let mut unsupported_origins = Vec::new();
    let mut unavailable_paths = BTreeMap::new();
    let entries = std::fs::read_dir(workspace)
        .map_err(|error| OrbitError::io(format!("scan {}", workspace.display()), error))?;
    let mut children = Vec::new();
    for entry in entries {
        let entry = entry.map_err(|error| {
            OrbitError::io(
                format!("read child entry in {}", workspace.display()),
                error,
            )
        })?;
        if entry
            .file_type()
            .map_err(|error| OrbitError::io(format!("inspect {}", entry.path().display()), error))?
            .is_dir()
        {
            children.push(entry);
        }
    }
    children.sort_by_key(|entry| entry.file_name());

    for child in children {
        let child_name = child.file_name().to_string_lossy().into_owned();
        if child_name == ".orbit" {
            continue;
        }
        let child_path = child.path();
        if !is_direct_git_repository(&child_path) {
            unavailable_paths.insert(
                child_name.clone(),
                "the previously discovered repository is no longer a Git repository".to_string(),
            );
            non_git_directories.push(child_name);
            continue;
        }
        let origin = match git_output(&child_path, &["remote", "get-url", "origin"]) {
            Ok(origin) if !origin.is_empty() => origin,
            _ => {
                unavailable_paths.insert(
                    child_name.clone(),
                    "the Git repository has no readable `origin` remote".to_string(),
                );
                missing_origins.push(child_name);
                continue;
            }
        };
        let repository = match parse_github_origin(&origin) {
            Ok(repository) => repository.to_ascii_lowercase(),
            Err(reason) => {
                unavailable_paths.insert(child_name.clone(), reason.clone());
                unsupported_origins.push(UnsupportedOrigin {
                    path: child_name,
                    origin,
                    reason,
                });
                continue;
            }
        };
        let previous_entry = previous.get(&repository);
        let (status, included, error, checked_at) = if !repository
            .split_once('/')
            .is_some_and(|(owner, _)| owner.eq_ignore_ascii_case(&organization))
        {
            (
                RepositoryStatus::UnsupportedOwner,
                false,
                Some(format!(
                    "owner is outside configured Workspace organization `{organization}`"
                )),
                None,
            )
        } else if board.config.workspace.is_excluded(&repository)
            || (board.config.workspace.has_include_filter()
                && !board.config.workspace.is_included(&repository))
        {
            (RepositoryStatus::Excluded, false, None, None)
        } else {
            match client.repository_info(&repository) {
                Ok(info) if info.owner_type.eq_ignore_ascii_case("organization") => {
                    (RepositoryStatus::Included, true, None, Some(Utc::now()))
                }
                Ok(_) => (
                    RepositoryStatus::UnsupportedOwner,
                    false,
                    Some("GitHub reports that this repository is not organization-owned".into()),
                    Some(Utc::now()),
                ),
                Err(error) => {
                    let was_known = previous_entry.is_some_and(|entry| entry.cached);
                    (
                        if was_known {
                            RepositoryStatus::Stale
                        } else {
                            RepositoryStatus::Inaccessible
                        },
                        true,
                        Some(error.message),
                        Some(Utc::now()),
                    )
                }
            }
        };
        let relative_path = child_path
            .strip_prefix(workspace)
            .unwrap_or(&child_path)
            .to_string_lossy()
            .into_owned();
        current.insert(
            repository.clone(),
            RepositoryEntry {
                repository,
                path: relative_path,
                discovered: true,
                status,
                included,
                cached: previous_entry.is_some_and(|entry| entry.cached)
                    || status == RepositoryStatus::Included,
                origin,
                error,
                last_checked_at: checked_at,
            },
        );
    }

    for (repository, mut entry) in previous {
        if current.contains_key(&repository) {
            continue;
        }
        if let Some(reason) = unavailable_paths.get(&entry.path) {
            entry.discovered = true;
            entry.status = RepositoryStatus::Stale;
            entry.error = Some(reason.clone());
        } else {
            entry.discovered = false;
            entry.status = RepositoryStatus::Removed;
            entry.included = false;
            entry.error = Some("repository directory or GitHub origin is no longer present".into());
        }
        current.insert(repository, entry);
    }

    catalog.organization = Some(organization);
    catalog.last_refreshed_at = Some(Utc::now());
    catalog.repositories = current;
    catalog.non_git_directories = non_git_directories;
    catalog.missing_origins = missing_origins;
    catalog.unsupported_origins = unsupported_origins;
    save_catalog(&path, &catalog)?;
    let count = catalog.repositories.len();
    let data = json!({
        "workspace": workspace,
        "last_refreshed_at": catalog.last_refreshed_at,
        "repository_count": count,
        "repositories": catalog.repositories.values().collect::<Vec<_>>(),
        "non_git_directories": catalog.non_git_directories,
        "missing_origins": catalog.missing_origins,
        "unsupported_origins": catalog.unsupported_origins,
    });
    Ok(crate::output::CommandOutput::new(
        data,
        format!("Refreshed Workspace repository catalog ({count} repositories)"),
    ))
}

fn set_override(
    board: &Board,
    repository: &str,
    include: bool,
) -> Result<crate::output::CommandOutput> {
    let repository = crate::github::GitHubConfig::normalize_repo(repository)?;
    let organization = board.config.github.organization.as_deref().ok_or_else(|| {
        OrbitError::new(
            code::GITHUB_NOT_CONFIGURED,
            "Workspace repository overrides need `[github].organization` configured first",
        )
    })?;
    let owner = repository
        .split_once('/')
        .expect("normalized repo has owner")
        .0;
    if !owner.eq_ignore_ascii_case(organization) {
        return Err(OrbitError::invalid_argument(format!(
            "Workspace repositories must be owned by configured organization `{organization}`"
        )));
    }
    board.save_config(
        if include {
            "orbit: workspace include repository"
        } else {
            "orbit: workspace exclude repository"
        },
        |config| {
            config
                .workspace
                .detached
                .retain(|item| !item.eq_ignore_ascii_case(&repository));
            let (selected, other) = if include {
                (&mut config.workspace.include, &mut config.workspace.exclude)
            } else {
                (&mut config.workspace.exclude, &mut config.workspace.include)
            };
            other.retain(|item| !item.eq_ignore_ascii_case(&repository));
            if !selected
                .iter()
                .any(|item| item.eq_ignore_ascii_case(&repository))
            {
                selected.push(repository.clone());
            }
            Ok(())
        },
    )?;
    let action = if include { "Included" } else { "Excluded" };
    Ok(crate::output::CommandOutput::new(
        json!({ "repository": repository, "override": if include { "include" } else { "exclude" } }),
        format!("{action} {repository}; run `orbit workspace refresh` to update discovery"),
    ))
}

pub(crate) fn set_repository_detached(
    board: &Board,
    repository: &str,
    detached: bool,
) -> Result<()> {
    let repository = crate::github::GitHubConfig::normalize_repo(repository)?;
    let organization = board.config.github.organization.as_deref().ok_or_else(|| {
        OrbitError::new(
            code::GITHUB_NOT_CONFIGURED,
            "Workspace repository changes need `[github].organization` configured first",
        )
    })?;
    let owner = repository
        .split_once('/')
        .expect("normalized repo has owner")
        .0;
    if !owner.eq_ignore_ascii_case(organization) {
        return Err(OrbitError::invalid_argument(format!(
            "Workspace repositories must be owned by configured organization `{organization}`"
        )));
    }

    board.save_config(
        if detached {
            "orbit: detach Workspace repository"
        } else {
            "orbit: reattach Workspace repository"
        },
        |config| {
            config
                .workspace
                .detached
                .retain(|item| !item.eq_ignore_ascii_case(&repository));
            if detached {
                config.workspace.detached.push(repository.clone());
            } else {
                config
                    .workspace
                    .exclude
                    .retain(|item| !item.eq_ignore_ascii_case(&repository));
            }
            Ok(())
        },
    )?;

    let path = catalog_path(&workspace_root(board)?);
    let mut catalog = load_catalog(&path)?;
    if let Some(entry) = catalog.repositories.get_mut(&repository) {
        if detached {
            entry.included = false;
            entry.status = RepositoryStatus::Excluded;
            entry.error = None;
        } else {
            entry.included = true;
            entry.status = if entry.cached {
                RepositoryStatus::Stale
            } else {
                RepositoryStatus::Inaccessible
            };
            entry.error = Some(
                "repository was reattached locally; run `orbit workspace refresh` to verify access"
                    .into(),
            );
        }
    }
    save_catalog(&path, &catalog)
}

pub(crate) fn is_direct_git_repository(path: &Path) -> bool {
    let Ok(output) = git_output(path, &["rev-parse", "--show-toplevel"]) else {
        return false;
    };
    let top = dunce::canonicalize(output.trim()).unwrap_or_else(|_| PathBuf::from(output.trim()));
    let candidate = dunce::canonicalize(path).unwrap_or_else(|_| path.to_path_buf());
    top == candidate
}

fn git_output(path: &Path, args: &[&str]) -> std::result::Result<String, String> {
    let output = Command::new("git")
        .arg("-C")
        .arg(path)
        .args(args)
        .output()
        .map_err(|error| format!("could not run git: {error}"))?;
    if !output.status.success() {
        return Err(String::from_utf8_lossy(&output.stderr).trim().to_string());
    }
    Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
}

fn parse_github_origin(origin: &str) -> std::result::Result<String, String> {
    let path = origin
        .strip_prefix("https://github.com/")
        .or_else(|| origin.strip_prefix("http://github.com/"))
        .or_else(|| origin.strip_prefix("ssh://git@github.com/"))
        .or_else(|| origin.strip_prefix("git@github.com:"))
        .ok_or_else(|| "origin is not a supported github.com HTTPS or SSH URL".to_string())?;
    let path = path.trim_end_matches('/');
    let path = path.strip_suffix(".git").unwrap_or(path);
    crate::github::GitHubConfig::normalize_repo(path).map_err(|error| error.message)
}

fn format_status(status: RepositoryStatus) -> &'static str {
    match status {
        RepositoryStatus::Included => "included",
        RepositoryStatus::Excluded => "excluded",
        RepositoryStatus::Inaccessible => "inaccessible",
        RepositoryStatus::Stale => "stale",
        RepositoryStatus::Removed => "removed",
        RepositoryStatus::UnsupportedOwner => "unsupported_owner",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_supported_github_remote_styles() {
        for (remote, expected) in [
            ("https://github.com/acme/api.git", "acme/api"),
            ("git@github.com:acme/api.git", "acme/api"),
            ("ssh://git@github.com/acme/api", "acme/api"),
        ] {
            assert_eq!(parse_github_origin(remote).unwrap(), expected);
        }
    }

    #[test]
    fn rejects_non_github_origins() {
        assert!(parse_github_origin("git@gitlab.com:acme/api.git").is_err());
    }
}

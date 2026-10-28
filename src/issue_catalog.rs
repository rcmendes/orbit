use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::json;

use crate::board::Board;
use crate::cli::{WorkspaceIssueInspectArgs, WorkspaceIssuesCommand, WorkspaceIssuesListArgs};
use crate::error::{OrbitError, Result};
use crate::github::client::{IssueCatalogItem, IssueComment, client_from_env};
use crate::github::model::IssueState;
use crate::graph::{Graph, LoadMode};

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
struct IssueCatalog {
    #[serde(default)]
    repositories: BTreeMap<String, CachedRepository>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
struct CachedRepository {
    #[serde(default)]
    last_refreshed_at: Option<DateTime<Utc>>,
    #[serde(default)]
    error: Option<String>,
    #[serde(default)]
    issues: Vec<IssueCatalogItem>,
}

#[derive(Debug, Clone, Serialize)]
struct RepositoryStatus {
    repository: String,
    last_refreshed_at: Option<DateTime<Utc>>,
    status: &'static str,
    stale: bool,
    available: bool,
    error: Option<String>,
    issue_count: usize,
}

pub fn run(board: &Board, command: WorkspaceIssuesCommand) -> Result<crate::output::CommandOutput> {
    let workspace = crate::workspace::workspace_root(board)?;
    match command {
        WorkspaceIssuesCommand::Refresh => refresh(board, &workspace),
        WorkspaceIssuesCommand::List(args) => list(board, &workspace, args),
        WorkspaceIssuesCommand::Inspect(args) => inspect(board, &workspace, args),
    }
}

fn cache_path(workspace: &Path) -> PathBuf {
    workspace.join(".orbit").join("cache").join("issues.json")
}

fn load(path: &Path) -> Result<IssueCatalog> {
    match std::fs::read(path) {
        Ok(bytes) => serde_json::from_slice(&bytes)
            .map_err(|error| OrbitError::config(format!("invalid {}: {error}", path.display()))),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(IssueCatalog::default()),
        Err(error) => Err(OrbitError::io(format!("read {}", path.display()), error)),
    }
}

fn save(path: &Path, catalog: &IssueCatalog) -> Result<()> {
    let parent = path
        .parent()
        .ok_or_else(|| OrbitError::config("Workspace cache has no parent directory"))?;
    std::fs::create_dir_all(parent)
        .map_err(|error| OrbitError::io(format!("create {}", parent.display()), error))?;
    let contents = serde_json::to_vec_pretty(catalog)
        .map_err(|error| OrbitError::config(format!("serialize Remote Issue catalog: {error}")))?;
    crate::atomic::write_atomic(path, &contents, true)
}

fn refresh(board: &Board, workspace: &Path) -> Result<crate::output::CommandOutput> {
    // A single explicit catalog refresh also reconciles direct child repositories,
    // so repositories added since the previous run are included immediately.
    crate::workspace::refresh(board, workspace)?;
    let client = client_from_env()?;
    let path = cache_path(workspace);
    let mut catalog = load(&path)?;
    let repositories = crate::workspace::included_repositories(workspace)?;
    let mut errors = Vec::new();
    let mut refreshed_count = 0;

    for repository in repositories {
        let result = fetch_repository(client.as_ref(), &repository);
        let entry = catalog.repositories.entry(repository.clone()).or_default();
        match result {
            Ok(issues) => {
                entry.issues = issues;
                entry.last_refreshed_at = Some(Utc::now());
                entry.error = None;
                refreshed_count += 1;
            }
            Err(error) => {
                let had_snapshot = entry.last_refreshed_at.is_some();
                entry.error = Some(error.message.clone());
                errors.push(json!({
                    "repository": repository,
                    "error": error.message,
                    "stale": had_snapshot,
                    "status": if had_snapshot { "stale" } else { "unavailable" },
                }));
            }
        }
    }
    save(&path, &catalog)?;
    let repositories = repository_statuses(&catalog);
    let data = json!({
        "workspace": workspace,
        "refreshed_count": refreshed_count,
        "errors": errors,
        "repositories": repositories,
        "issue_count": catalog.repositories.values().map(|repo| repo.issues.len()).sum::<usize>(),
    });
    let summary = format!(
        "Refreshed Remote Issue catalog for {refreshed_count} repositories ({} errors)",
        errors.len()
    );
    Ok(crate::output::CommandOutput::new(data, summary))
}

fn fetch_repository(
    client: &dyn crate::github::GitHubClient,
    repository: &str,
) -> Result<Vec<IssueCatalogItem>> {
    let mut page = 1;
    let mut issues = BTreeMap::new();
    loop {
        let results = client.list_issues_page(repository, page)?;
        if results.is_empty() {
            break;
        }
        for issue in results {
            issues.insert(issue.number, issue);
        }
        page += 1;
    }
    Ok(issues.into_values().collect())
}

fn repository_statuses(catalog: &IssueCatalog) -> Vec<RepositoryStatus> {
    catalog
        .repositories
        .iter()
        .map(|(repository, entry)| RepositoryStatus {
            repository: repository.clone(),
            last_refreshed_at: entry.last_refreshed_at,
            status: match (entry.error.is_some(), entry.last_refreshed_at.is_some()) {
                (false, _) => "fresh",
                (true, true) => "stale",
                (true, false) => "unavailable",
            },
            stale: entry.error.is_some() && entry.last_refreshed_at.is_some(),
            available: entry.last_refreshed_at.is_some(),
            error: entry.error.clone(),
            issue_count: entry.issues.len(),
        })
        .collect()
}

fn linked_identities(board: &Board) -> Result<BTreeSet<String>> {
    let (graph, _) = Graph::load(board, LoadMode::Lenient)?;
    Ok(graph
        .tasks
        .values()
        .filter_map(|task| task.front.github.as_ref())
        .map(|link| format!("{}#{}", link.repo.to_ascii_lowercase(), link.number))
        .collect())
}

fn list(
    board: &Board,
    workspace: &Path,
    args: WorkspaceIssuesListArgs,
) -> Result<crate::output::CommandOutput> {
    let path = cache_path(workspace);
    let catalog = load(&path)?;
    let repository_filter = args
        .repo
        .as_deref()
        .map(crate::github::GitHubConfig::normalize_repo)
        .transpose()?
        .map(|repo| repo.to_ascii_lowercase());
    let text_filter = args.text.as_deref().map(str::to_lowercase);
    let linked = linked_identities(board)?;
    let mut issues = Vec::new();
    for (repository, cached) in &catalog.repositories {
        if repository_filter
            .as_deref()
            .is_some_and(|filter| filter != repository)
        {
            continue;
        }
        for issue in &cached.issues {
            if args
                .state
                .as_deref()
                .is_some_and(|state| state != state_name(issue.state))
            {
                continue;
            }
            if args.label.iter().any(|label| {
                !issue
                    .labels
                    .iter()
                    .any(|item| item.eq_ignore_ascii_case(label))
            }) {
                continue;
            }
            if text_filter
                .as_deref()
                .is_some_and(|text| !issue.title.to_lowercase().contains(text))
            {
                continue;
            }
            let is_linked = linked.contains(&issue.identity().to_ascii_lowercase());
            if args.linked && !is_linked || args.unlinked && is_linked {
                continue;
            }
            let mut value = serde_json::to_value(issue).unwrap_or(serde_json::Value::Null);
            value["identity"] = json!(issue.identity());
            value["linked"] = json!(is_linked);
            value["stale"] = json!(cached.error.is_some());
            value["last_refreshed_at"] = json!(cached.last_refreshed_at);
            issues.push(value);
        }
    }
    issues.sort_by(|a, b| a["identity"].as_str().cmp(&b["identity"].as_str()));
    let data = json!({
        "count": issues.len(),
        "issues": issues,
        "repositories": repository_statuses(&catalog),
    });
    Ok(crate::output::CommandOutput::new(
        data,
        format!("{} Remote Issues", issues.len()),
    ))
}

fn inspect(
    board: &Board,
    workspace: &Path,
    args: WorkspaceIssueInspectArgs,
) -> Result<crate::output::CommandOutput> {
    let (repository, number) = parse_identity(&args.identity)?;
    let path = cache_path(workspace);
    let catalog = load(&path)?;
    let cached = catalog
        .repositories
        .get(&repository)
        .ok_or_else(|| OrbitError::not_found(&args.identity))?;
    let item = cached
        .issues
        .iter()
        .find(|issue| issue.number == number)
        .ok_or_else(|| OrbitError::not_found(&args.identity))?;
    let client = client_from_env()?;
    let detail = client.get_issue(&repository, number)?;
    let mut comments = Vec::<IssueComment>::new();
    let mut page = 1;
    loop {
        let results = client.issue_comments_page(&repository, number, page)?;
        if results.is_empty() {
            break;
        }
        comments.extend(results);
        page += 1;
    }
    let linked = linked_identities(board)?.contains(&item.identity().to_ascii_lowercase());
    let data = json!({
        "identity": item.identity(),
        "repo": item.repo,
        "number": item.number,
        "title": detail.title,
        "state": detail.state,
        "labels": detail.labels,
        "url": item.url,
        "created_at": item.created_at,
        "updated_at": detail.updated_at,
        "area": detail.area,
        "verification": detail.verification,
        "issue_fields": item.issue_fields,
        "body": detail.body,
        "comments": comments,
        "linked": linked,
        "stale": cached.error.is_some(),
        "last_refreshed_at": cached.last_refreshed_at,
    });
    Ok(crate::output::CommandOutput::new(
        data,
        format!("Inspected {}", args.identity),
    ))
}

fn parse_identity(identity: &str) -> Result<(String, u64)> {
    let (repository, number) = identity.rsplit_once('#').ok_or_else(|| {
        OrbitError::invalid_argument("Remote Issue identity must be OWNER/REPO#NUMBER")
    })?;
    let repository = crate::github::GitHubConfig::normalize_repo(repository)?;
    let number = number
        .parse::<u64>()
        .ok()
        .filter(|number| *number > 0)
        .ok_or_else(|| {
            OrbitError::invalid_argument("Remote Issue number must be a positive integer")
        })?;
    Ok((repository.to_ascii_lowercase(), number))
}

fn state_name(state: IssueState) -> &'static str {
    match state {
        IssueState::Open => "open",
        IssueState::Closed => "closed",
    }
}

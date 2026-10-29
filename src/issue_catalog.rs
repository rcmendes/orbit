use std::collections::{BTreeMap, BTreeSet, HashSet};
use std::path::{Path, PathBuf};

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::json;

use crate::board::Board;
use crate::cli::{
    WorkspaceIssueInspectArgs, WorkspaceIssuesCommand, WorkspaceIssuesImportArgs,
    WorkspaceIssuesListArgs,
};
use crate::error::{OrbitError, Result};
use crate::github::client::{IssueCatalogItem, IssueComment, client_from_env};
use crate::github::model::IssueState;
use crate::graph::{Graph, LoadMode};
use crate::task::{self, Status, Task, TaskArea, TaskFrontmatter, TaskType};

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
        WorkspaceIssuesCommand::Import(args) => import(board, &workspace, args),
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
    let detail = client.get_issue(
        &repository,
        number,
        (!board.config.github.mode.is_basic()).then_some(&board.config.github.issue_fields),
    )?;
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

#[derive(Debug, Clone)]
struct PreparedIssue {
    issue: crate::github::model::IssueSnapshot,
    labels: Vec<String>,
    area: Option<TaskArea>,
    verification: Option<String>,
    status: Status,
}

enum ImportAction {
    Create(PreparedIssue),
    Link(PreparedIssue, Box<Task>),
}

struct ImportCandidate {
    identity: String,
    action: ImportAction,
}

fn import(
    board: &Board,
    workspace: &Path,
    args: WorkspaceIssuesImportArgs,
) -> Result<crate::output::CommandOutput> {
    let catalog = load(&cache_path(workspace))?;
    let graph = Graph::load(board, LoadMode::Strict)?.0;
    let mappings = parse_link_mappings(&args.link, &args.identities)?;
    let included = crate::workspace::included_repositories(workspace)?
        .into_iter()
        .map(|repository| repository.to_ascii_lowercase())
        .collect::<BTreeSet<_>>();
    let client = client_from_env()?;

    let mut linked_issues = BTreeMap::new();
    for task in graph.iter() {
        if let Some(link) = task.github_link() {
            linked_issues.insert(
                format!("{}#{}", link.repo.to_ascii_lowercase(), link.number),
                task.id.clone(),
            );
        }
    }

    let mut candidates = Vec::new();
    let mut creates = Vec::new();
    let mut links = Vec::new();
    let mut terminal_tasks = Vec::new();
    let mut skipped_duplicates = Vec::new();
    let mut errors = Vec::new();
    let mut seen_issues = BTreeSet::new();
    let mut seen_tasks = HashSet::new();

    for raw_identity in &args.identities {
        let (repository, number) = match parse_identity(raw_identity) {
            Ok(identity) => identity,
            Err(error) => {
                errors.push(json!({"identity": raw_identity, "message": error.message}));
                continue;
            }
        };
        let identity = format!("{repository}#{number}");
        if !seen_issues.insert(identity.clone()) {
            skipped_duplicates.push(json!({
                "identity": identity,
                "reason": "issue_selected_more_than_once",
            }));
            continue;
        }
        if !included.contains(&repository) {
            errors.push(json!({
                "identity": identity,
                "message": "repository is not currently discovered and included in this Workspace",
            }));
            continue;
        }
        let Some(cached) = catalog.repositories.get(&repository) else {
            errors.push(json!({
                "identity": identity,
                "message": "Remote Issue is not in the local catalog; refresh the Workspace Issue catalog first",
            }));
            continue;
        };
        if !cached.issues.iter().any(|issue| issue.number == number) {
            errors.push(json!({
                "identity": identity,
                "message": "Remote Issue is not in the local catalog; refresh the Workspace Issue catalog first",
            }));
            continue;
        }
        if let Some(task) = linked_issues.get(&identity) {
            skipped_duplicates.push(json!({
                "identity": identity,
                "reason": "issue_already_linked",
                "task": task,
            }));
            continue;
        }

        let existing_task = if let Some(task_id) = mappings.get(&identity) {
            let Some(task) = graph.tasks.get(task_id) else {
                errors.push(json!({
                    "identity": identity,
                    "task": task_id,
                    "message": format!("Local Task `{task_id}` does not exist"),
                }));
                continue;
            };
            if let Some(link) = task.github_link() {
                skipped_duplicates.push(json!({
                    "identity": identity,
                    "reason": "task_already_linked",
                    "task": task_id,
                    "linked_identity": format!("{}#{}", link.repo, link.number),
                }));
                continue;
            }
            if !seen_tasks.insert(task_id.clone()) {
                skipped_duplicates.push(json!({
                    "identity": identity,
                    "reason": "task_selected_more_than_once",
                    "task": task_id,
                }));
                continue;
            }
            Some(task.clone())
        } else {
            None
        };

        let issue = match client.get_issue(
            &repository,
            number,
            (!board.config.github.mode.is_basic()).then_some(&board.config.github.issue_fields),
        ) {
            Ok(issue) => issue,
            Err(error) => {
                errors.push(json!({"identity": identity, "message": error.message}));
                continue;
            }
        };
        let prepared = match prepare_issue(issue) {
            Ok(prepared) => prepared,
            Err(error) => {
                errors.push(json!({"identity": identity, "message": error.message}));
                continue;
            }
        };

        match existing_task {
            Some(task) => {
                let differences = task_issue_differences(&task, &prepared);
                links.push(json!({
                    "identity": identity,
                    "task": task.id,
                    "status": task.front.status,
                    "remote_status": prepared.status,
                    "differences": differences,
                }));
                if task.front.status.is_terminal() {
                    terminal_tasks.push(json!({
                        "identity": identity,
                        "task": task.id,
                        "status": task.front.status,
                    }));
                }
                candidates.push(ImportCandidate {
                    identity,
                    action: ImportAction::Link(prepared, Box::new(task)),
                });
            }
            None => {
                creates.push(json!({
                    "identity": identity,
                    "title": prepared.issue.title,
                    "status": prepared.status,
                    "terminal": prepared.status.is_terminal(),
                    "url": format!("https://github.com/{repository}/issues/{number}"),
                }));
                if prepared.status.is_terminal() {
                    terminal_tasks.push(json!({
                        "identity": identity,
                        "status": prepared.status,
                    }));
                }
                candidates.push(ImportCandidate {
                    identity,
                    action: ImportAction::Create(prepared),
                });
            }
        }
    }

    let mut imported = Vec::new();
    if args.apply {
        let mut reserved_ids = graph.tasks.keys().cloned().collect::<HashSet<_>>();
        let mut writes = Vec::new();
        for candidate in candidates {
            let (task, created) = match candidate.action {
                ImportAction::Create(prepared) => {
                    let task = create_linked_task(board, &prepared, &mut reserved_ids)?;
                    (task, true)
                }
                ImportAction::Link(prepared, task) => {
                    let mut task = *task;
                    task.front.updated_at = Utc::now();
                    task.front.github = Some(crate::github::GitHubLink::new(
                        prepared.issue.repo.to_ascii_lowercase(),
                        prepared.issue.number,
                    ));
                    let baseline = {
                        let link = task.front.github.as_ref().expect("link was set");
                        crate::github::sync::baseline_from(
                            crate::github::sync::OrbitSyncView { task: &task, link },
                            &prepared.issue,
                        )
                    };
                    task.front.github.as_mut().expect("link was set").last_sync = Some(baseline);
                    (task, false)
                }
            };
            let path = board.task_path(&task.id);
            let rendered = task::render_task(&task)?;
            writes.push((path, rendered, task.clone(), created, candidate.identity));
        }

        for (path, rendered, task, created, identity) in &writes {
            crate::atomic::write_atomic(path, rendered.as_bytes(), !created)?;
            imported.push(json!({
                "identity": identity,
                "task": task.id,
                "action": if *created { "created" } else { "linked" },
                "status": task.front.status,
            }));
        }
        if !writes.is_empty() {
            board.commit("orbit: import Workspace Remote Issues", false)?;
        }
    }

    let data = json!({
        "apply": args.apply,
        "creates": creates,
        "links": links,
        "terminal_tasks": terminal_tasks,
        "skipped_duplicates": skipped_duplicates,
        "errors": errors,
        "imported": imported,
    });
    let summary = if args.apply {
        format!(
            "Imported {} Remote Issue(s); GitHub was not changed",
            data["imported"].as_array().map_or(0, Vec::len)
        )
    } else {
        format!(
            "Remote Issue import preview: {} create(s), {} link(s), {} duplicate(s), {} error(s); nothing was written",
            data["creates"].as_array().map_or(0, Vec::len),
            data["links"].as_array().map_or(0, Vec::len),
            data["skipped_duplicates"].as_array().map_or(0, Vec::len),
            data["errors"].as_array().map_or(0, Vec::len),
        )
    };
    Ok(crate::output::CommandOutput::new(data, summary))
}

fn parse_link_mappings(
    mappings: &[String],
    identities: &[String],
) -> Result<BTreeMap<String, String>> {
    let selected = identities
        .iter()
        .filter_map(|identity| parse_identity(identity).ok())
        .map(|(repository, number)| format!("{repository}#{number}"))
        .collect::<BTreeSet<_>>();
    let mut parsed = BTreeMap::new();
    for mapping in mappings {
        let (identity, task) = mapping.split_once('=').ok_or_else(|| {
            OrbitError::invalid_argument("--link must use OWNER/REPO#NUMBER=TASK")
        })?;
        let (repository, number) = parse_identity(identity)?;
        let identity = format!("{repository}#{number}");
        let task = task.trim();
        if task.is_empty() {
            return Err(OrbitError::invalid_argument(
                "--link task ID must not be empty",
            ));
        }
        if !selected.contains(&identity) {
            return Err(OrbitError::invalid_argument(format!(
                "--link identity `{identity}` is not in the selected import batch"
            )));
        }
        if parsed.insert(identity.clone(), task.to_string()).is_some() {
            return Err(OrbitError::invalid_argument(format!(
                "multiple --link mappings were provided for `{identity}`"
            )));
        }
    }
    Ok(parsed)
}

fn prepare_issue(issue: crate::github::model::IssueSnapshot) -> Result<PreparedIssue> {
    if issue.title.trim().is_empty() {
        return Err(OrbitError::invalid_argument(
            "GitHub Issue title must not be empty",
        ));
    }
    let mut labels = Vec::with_capacity(issue.labels.len());
    for label in &issue.labels {
        let label = task::normalize_label(label)?;
        if !labels.contains(&label) {
            labels.push(label);
        }
    }
    labels.sort();
    let area = issue
        .area
        .as_deref()
        .map(crate::github::fields::parse_area)
        .transpose()?;
    let verification = task::normalize_verification(issue.verification.clone())?;
    let status = crate::github::status::status_from_issue(issue.state, &issue.labels);
    Ok(PreparedIssue {
        issue,
        labels,
        area,
        verification,
        status,
    })
}

fn task_issue_differences(task: &Task, prepared: &PreparedIssue) -> Vec<serde_json::Value> {
    let issue = &prepared.issue;
    let local_area = task.front.area.map(|area| area.as_str().to_string());
    let fields = [
        ("title", json!(task.front.title), json!(issue.title)),
        ("body", json!(task.body), json!(issue.body)),
        ("labels", json!(task.front.labels), json!(issue.labels)),
        ("status", json!(task.front.status), json!(prepared.status)),
        ("area", json!(local_area), json!(issue.area)),
        (
            "verification",
            json!(task.front.verification),
            json!(issue.verification),
        ),
    ];
    fields
        .into_iter()
        .filter(|(_, local, remote)| local != remote)
        .map(|(field, orbit, github)| json!({"field": field, "orbit": orbit, "github": github}))
        .collect()
}

fn create_linked_task(
    board: &Board,
    prepared: &PreparedIssue,
    reserved_ids: &mut HashSet<String>,
) -> Result<Task> {
    let id = loop {
        let id = task::generate_id(&board.config.id.prefix, board.config.id.length);
        if reserved_ids.insert(id.clone()) && !board.task_path(&id).exists() {
            break id;
        }
    };
    let now = Utc::now();
    let mut task = Task {
        id,
        front: TaskFrontmatter {
            title: prepared.issue.title.trim().to_string(),
            status: prepared.status,
            task_type: TaskType::Task,
            priority: board.config.board.default_priority,
            labels: prepared.labels.clone(),
            depends_on: Vec::new(),
            parent: None,
            waiting_on: None,
            checkpoint: None,
            area: prepared.area,
            verification: prepared.verification.clone(),
            github: Some(crate::github::GitHubLink::new(
                prepared.issue.repo.to_ascii_lowercase(),
                prepared.issue.number,
            )),
            claim: None,
            stray_claim_times: None,
            created_at: now,
            updated_at: now,
            extra: BTreeMap::new(),
        },
        body: prepared.issue.body.clone(),
    };
    let link = task.front.github.as_ref().expect("link was set");
    task.front.github.as_mut().expect("link was set").last_sync =
        Some(crate::github::sync::baseline_from(
            crate::github::sync::OrbitSyncView { task: &task, link },
            &prepared.issue,
        ));
    Ok(task)
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

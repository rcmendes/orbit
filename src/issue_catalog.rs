use std::collections::{BTreeMap, BTreeSet, HashSet};
use std::path::{Path, PathBuf};

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::json;

use crate::board::Board;
use crate::cli::{
    WorkspaceIssueInspectArgs, WorkspaceIssueMutationArgs, WorkspaceIssuesCommand,
    WorkspaceIssuesImportArgs, WorkspaceIssuesListArgs,
};
use crate::error::{OrbitError, Result};
use crate::github::client::{IssueCatalogItem, IssueComment, client_from_env};
use crate::github::model::IssueState;
use crate::graph::{Graph, LoadMode};
use crate::task::{self, Status, Task, TaskArea, TaskFrontmatter, TaskType};

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub(crate) struct IssueCatalog {
    #[serde(default)]
    pub(crate) repositories: BTreeMap<String, CachedRepository>,
    #[serde(default)]
    pub(crate) archived_issues: BTreeMap<String, BTreeMap<u64, IssueCatalogItem>>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub(crate) struct CachedRepository {
    #[serde(default)]
    pub(crate) last_refreshed_at: Option<DateTime<Utc>>,
    #[serde(default)]
    pub(crate) error: Option<String>,
    #[serde(default)]
    pub(crate) issues: Vec<IssueCatalogItem>,
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
        WorkspaceIssuesCommand::Detach(args) => detach_task(board, &workspace, args),
        WorkspaceIssuesCommand::Reattach(args) => reattach_task(board, &workspace, args),
    }
}

pub(crate) fn cache_path(workspace: &Path) -> PathBuf {
    workspace.join(".orbit").join("cache").join("issues.json")
}

pub(crate) fn catalog_for_diagnostics(workspace: &Path) -> Result<IssueCatalog> {
    load(&cache_path(workspace))
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
        .filter_map(|task| task.github_link())
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
            if let Some(link) = task.front.github.as_ref().filter(|link| !link.active) {
                errors.push(json!({
                    "identity": identity,
                    "task": task_id,
                    "message": format!(
                        "Local Task `{task_id}` retains inactive provenance for {}#{}; use workspace issues reattach",
                        link.repo,
                        link.number
                    ),
                }));
                continue;
            }
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

pub(crate) fn detach_repository(
    board: &Board,
    workspace: &Path,
    args: crate::cli::WorkspaceRepositoryMutationArgs,
) -> Result<crate::output::CommandOutput> {
    let repository =
        crate::github::GitHubConfig::normalize_repo(&args.repository)?.to_ascii_lowercase();
    let (graph, _) = Graph::load(board, LoadMode::Strict)?;
    let tasks = graph
        .iter()
        .filter_map(|task| {
            task.github_link()
                .filter(|link| link.repo.eq_ignore_ascii_case(&repository))
                .map(|link| {
                    json!({
                        "task": task.id,
                        "identity": format!("{}#{}", link.repo, link.number),
                        "title": task.front.title,
                    })
                })
        })
        .collect::<Vec<_>>();
    let path = cache_path(workspace);
    let mut catalog = load(&path)?;
    let remote_issue_snapshots = catalog
        .repositories
        .get(&repository)
        .map(|cached| {
            cached
                .issues
                .iter()
                .map(|issue| json!({ "identity": issue.identity(), "snapshot": issue }))
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    let issue_count = catalog
        .repositories
        .get(&repository)
        .map_or(0, |cached| cached.issues.len());
    let archived_count = catalog
        .archived_issues
        .get(&repository)
        .map_or(0, BTreeMap::len);
    let already_detached = board
        .config
        .workspace
        .detached
        .iter()
        .any(|item| item.eq_ignore_ascii_case(&repository))
        && tasks.is_empty()
        && issue_count == 0;

    let mut detached = Vec::new();
    if args.apply && !already_detached {
        for task in graph.iter().filter(|task| {
            task.github_link()
                .is_some_and(|link| link.repo.eq_ignore_ascii_case(&repository))
        }) {
            let mut task = task.clone();
            let link = task.front.github.as_mut().expect("active link exists");
            let identity = format!("{}#{}", link.repo, link.number);
            link.active = false;
            task.front.updated_at = Utc::now();
            let rendered = task::render_task(&task)?;
            crate::atomic::write_atomic(&board.task_path(&task.id), rendered.as_bytes(), true)?;
            detached.push(json!({
                "task": task.id,
                "identity": identity,
            }));
        }
        if let Some(cached) = catalog.repositories.get_mut(&repository) {
            let archived = catalog
                .archived_issues
                .entry(repository.clone())
                .or_default();
            for issue in cached.issues.drain(..) {
                archived.insert(issue.number, issue);
            }
        }
        save(&path, &catalog)?;
        crate::workspace::set_repository_detached(board, &repository, true)?;
        if !detached.is_empty() {
            board.commit("orbit: detach Workspace repository", false)?;
        }
    }

    let data = json!({
        "repository": repository,
        "apply": args.apply,
        "already_detached": already_detached,
        "tasks": tasks,
        "remote_issues": issue_count,
        "remote_issue_snapshots": remote_issue_snapshots,
        "archived_issues": archived_count + if args.apply { issue_count } else { 0 },
        "detached": detached,
    });
    let summary = if args.apply {
        format!("Detached {repository}; GitHub was not changed")
    } else {
        format!(
            "Workspace repository detach preview: {} Linked Task(s), {issue_count} Remote Issue(s); nothing was written",
            data["tasks"].as_array().map_or(0, Vec::len)
        )
    };
    Ok(crate::output::CommandOutput::new(data, summary))
}

pub(crate) fn reattach_repository(
    board: &Board,
    workspace: &Path,
    args: crate::cli::WorkspaceRepositoryMutationArgs,
) -> Result<crate::output::CommandOutput> {
    let repository =
        crate::github::GitHubConfig::normalize_repo(&args.repository)?.to_ascii_lowercase();
    let (graph, _) = Graph::load(board, LoadMode::Strict)?;
    let inactive = graph
        .iter()
        .filter_map(|task| {
            task.front
                .github
                .as_ref()
                .filter(|link| !link.active && link.repo.eq_ignore_ascii_case(&repository))
                .map(|link| (task, link))
        })
        .collect::<Vec<_>>();
    for (task, link) in &inactive {
        let duplicate_count = graph
            .iter()
            .filter(|candidate| {
                candidate
                    .front
                    .github
                    .as_ref()
                    .is_some_and(|candidate_link| {
                        candidate_link.repo.eq_ignore_ascii_case(&link.repo)
                            && candidate_link.number == link.number
                    })
            })
            .count();
        if duplicate_count > 1 {
            return Err(OrbitError::invalid_argument(format!(
                "cannot reattach {}#{}: multiple Tasks retain that GitHub identity, including {}",
                link.repo, link.number, task.id
            )));
        }
    }
    let path = cache_path(workspace);
    let mut catalog = load(&path)?;
    let archived_count = catalog
        .archived_issues
        .get(&repository)
        .map_or(0, BTreeMap::len);
    let archived_issue_snapshots = catalog
        .archived_issues
        .get(&repository)
        .map(|issues| {
            issues
                .values()
                .map(|issue| json!({ "identity": issue.identity(), "snapshot": issue }))
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    let included = crate::workspace::included_repositories(workspace)?
        .iter()
        .any(|item| item.eq_ignore_ascii_case(&repository));
    let already_attached = included && inactive.is_empty() && archived_count == 0;

    let client = if !inactive.is_empty() {
        Some(client_from_env()?)
    } else {
        None
    };
    let mut previews = Vec::new();
    for (task, link) in &inactive {
        let issue = client
            .as_ref()
            .expect("client loaded when inactive links exist")
            .get_issue(
                &repository,
                link.number,
                (!board.config.github.mode.is_basic()).then_some(&board.config.github.issue_fields),
            )?;
        validate_issue_identity(&issue, &repository, link.number)?;
        let plan = crate::github::sync::plan_sync(
            crate::github::sync::OrbitSyncView { task, link },
            &issue,
            crate::github::model::SyncDirection::Both,
        );
        previews.push(json!({
            "task": task.id,
            "identity": format!("{}#{}", link.repo, link.number),
            "issue_title": issue.title,
            "sync_plan": plan,
        }));
    }

    let mut reattached = Vec::new();
    if args.apply && !already_attached {
        if !included
            && board
                .config
                .workspace
                .exclude
                .iter()
                .all(|item| !item.eq_ignore_ascii_case(&repository))
            && board
                .config
                .workspace
                .detached
                .iter()
                .all(|item| !item.eq_ignore_ascii_case(&repository))
        {
            return Err(OrbitError::invalid_argument(format!(
                "repository `{repository}` is not included in this Workspace; reattach the repository first"
            )));
        }
        for (task, link) in inactive {
            let mut task = task.clone();
            let stored = task
                .front
                .github
                .as_mut()
                .expect("inactive provenance exists");
            // The selected Task was matched on the exact normalized repository and number.
            stored.active = true;
            task.front.updated_at = Utc::now();
            let rendered = task::render_task(&task)?;
            crate::atomic::write_atomic(&board.task_path(&task.id), rendered.as_bytes(), true)?;
            reattached.push(json!({
                "task": task.id,
                "identity": format!("{}#{}", link.repo, link.number),
            }));
        }
        if let Some(archived) = catalog.archived_issues.remove(&repository) {
            let active = catalog.repositories.entry(repository.clone()).or_default();
            for (number, issue) in archived {
                active.issues.retain(|item| item.number != number);
                active.issues.push(issue);
            }
            active.issues.sort_by_key(|issue| issue.number);
        }
        save(&path, &catalog)?;
        crate::workspace::set_repository_detached(board, &repository, false)?;
        if !reattached.is_empty() {
            board.commit("orbit: reattach Workspace repository", false)?;
        }
    }

    let data = json!({
        "repository": repository,
        "apply": args.apply,
        "already_attached": already_attached,
        "archived_issues": archived_count,
        "archived_issue_snapshots": archived_issue_snapshots,
        "tasks": previews,
        "reattached": reattached,
    });
    let summary = if args.apply {
        format!("Reattached {repository}; GitHub was not changed")
    } else {
        format!(
            "Workspace repository reattach preview: {} Linked Task(s), {archived_count} archived Remote Issue(s); conflicts require review",
            previews.len()
        )
    };
    Ok(crate::output::CommandOutput::new(data, summary))
}

fn detach_task(
    board: &Board,
    workspace: &Path,
    args: WorkspaceIssueMutationArgs,
) -> Result<crate::output::CommandOutput> {
    let (repository, number) = parse_identity(&args.identity)?;
    let identity = format!("{repository}#{number}");
    let graph = Graph::load(board, LoadMode::Strict)?.0;
    let active = graph
        .iter()
        .filter(|task| {
            task.github_link().is_some_and(|link| {
                link.repo.eq_ignore_ascii_case(&repository) && link.number == number
            })
        })
        .collect::<Vec<_>>();
    let inactive = graph.iter().find(|task| {
        task.front.github.as_ref().is_some_and(|link| {
            !link.active && link.repo.eq_ignore_ascii_case(&repository) && link.number == number
        })
    });
    let mut catalog = load(&cache_path(workspace))?;
    let already_detached = active.is_empty() && inactive.is_some();
    let tasks = active
        .iter()
        .map(|task| {
            let snapshot = catalog
                .repositories
                .get(&repository)
                .and_then(|cached| cached.issues.iter().find(|issue| issue.number == number));
            json!({
                "task": task.id,
                "title": task.front.title,
                "remote_issue": snapshot.map(|issue| json!({
                    "identity": issue.identity(),
                    "snapshot": issue,
                })),
            })
        })
        .collect::<Vec<_>>();
    let mut detached = Vec::new();

    if active.is_empty() && inactive.is_none() {
        return Err(OrbitError::not_found(&identity));
    }
    if args.apply && !already_detached {
        for source in active {
            let mut task = source.clone();
            let link = task.front.github.as_mut().expect("active link exists");
            let linked_repo = link.repo.clone();
            let linked_number = link.number;
            link.active = false;
            task.front.updated_at = Utc::now();
            let rendered = task::render_task(&task)?;
            crate::atomic::write_atomic(&board.task_path(&task.id), rendered.as_bytes(), true)?;
            detached.push(json!({
                "task": task.id,
                "identity": format!("{linked_repo}#{linked_number}"),
            }));
        }
        if let Some(repository_cache) = catalog.repositories.get_mut(&repository)
            && let Some(index) = repository_cache
                .issues
                .iter()
                .position(|issue| issue.number == number)
        {
            let issue = repository_cache.issues.remove(index);
            catalog
                .archived_issues
                .entry(repository.clone())
                .or_default()
                .insert(number, issue);
        }
        save(&cache_path(workspace), &catalog)?;
        if !detached.is_empty() {
            board.commit("orbit: detach Linked Task", false)?;
        }
    }

    let data = json!({
        "identity": identity,
        "apply": args.apply,
        "already_detached": already_detached,
        "tasks": tasks,
        "detached": detached,
    });
    let summary = if args.apply {
        format!("Detached {identity}; GitHub was not changed")
    } else {
        format!("Linked Task detach preview for {identity}; nothing was written")
    };
    Ok(crate::output::CommandOutput::new(data, summary))
}

fn reattach_task(
    board: &Board,
    workspace: &Path,
    args: WorkspaceIssueMutationArgs,
) -> Result<crate::output::CommandOutput> {
    let (repository, number) = parse_identity(&args.identity)?;
    let identity = format!("{repository}#{number}");
    let graph = Graph::load(board, LoadMode::Strict)?.0;
    let inactive = graph.iter().find(|task| {
        task.front.github.as_ref().is_some_and(|link| {
            !link.active && link.repo.eq_ignore_ascii_case(&repository) && link.number == number
        })
    });
    let provenance_count = graph
        .iter()
        .filter(|task| {
            task.front.github.as_ref().is_some_and(|link| {
                link.repo.eq_ignore_ascii_case(&repository) && link.number == number
            })
        })
        .count();
    if provenance_count > 1 {
        return Err(OrbitError::invalid_argument(format!(
            "cannot reattach {identity}: multiple Tasks retain that GitHub identity"
        )));
    }
    if inactive.is_none()
        && graph.iter().any(|task| {
            task.github_link().is_some_and(|link| {
                link.repo.eq_ignore_ascii_case(&repository) && link.number == number
            })
        })
    {
        return Ok(crate::output::CommandOutput::new(
            json!({ "identity": identity, "apply": args.apply, "already_attached": true, "tasks": [], "reattached": [] }),
            format!("{identity} is already attached"),
        ));
    }
    let Some(task) = inactive else {
        return Err(OrbitError::not_found(&identity));
    };
    let included = crate::workspace::included_repositories(workspace)?
        .iter()
        .any(|item| item.eq_ignore_ascii_case(&repository));
    if !included {
        return Err(OrbitError::invalid_argument(format!(
            "repository `{repository}` is not included in this Workspace; reattach the repository first"
        )));
    }
    let client = client_from_env()?;
    let issue = client.get_issue(
        &repository,
        number,
        (!board.config.github.mode.is_basic()).then_some(&board.config.github.issue_fields),
    )?;
    validate_issue_identity(&issue, &repository, number)?;
    let link = task
        .front
        .github
        .as_ref()
        .expect("inactive provenance exists");
    let sync_plan = crate::github::sync::plan_sync(
        crate::github::sync::OrbitSyncView { task, link },
        &issue,
        crate::github::model::SyncDirection::Both,
    );
    let mut catalog = load(&cache_path(workspace))?;
    let preview = json!({
        "task": task.id,
        "identity": identity,
        "issue_title": issue.title,
        "sync_plan": sync_plan,
    });
    let mut reattached = Vec::new();
    if args.apply {
        let mut task = task.clone();
        task.front
            .github
            .as_mut()
            .expect("inactive provenance exists")
            .active = true;
        task.front.updated_at = Utc::now();
        let rendered = task::render_task(&task)?;
        crate::atomic::write_atomic(&board.task_path(&task.id), rendered.as_bytes(), true)?;
        if let Some(archived) = catalog
            .archived_issues
            .get_mut(&repository)
            .and_then(|issues| issues.remove(&number))
        {
            let active = catalog.repositories.entry(repository.clone()).or_default();
            active.issues.retain(|item| item.number != number);
            active.issues.push(archived);
            active.issues.sort_by_key(|item| item.number);
        }
        if catalog
            .archived_issues
            .get(&repository)
            .is_some_and(BTreeMap::is_empty)
        {
            catalog.archived_issues.remove(&repository);
        }
        save(&cache_path(workspace), &catalog)?;
        board.commit("orbit: reattach Linked Task", false)?;
        reattached.push(json!({ "task": task.id, "identity": identity }));
    }
    let data = json!({
        "identity": identity,
        "apply": args.apply,
        "tasks": [preview],
        "reattached": reattached,
    });
    let summary = if args.apply {
        format!("Reattached {identity}; review the sync plan before synchronizing")
    } else {
        format!("Linked Task reattach preview for {identity}; no data was changed")
    };
    Ok(crate::output::CommandOutput::new(data, summary))
}

fn validate_issue_identity(
    issue: &crate::github::model::IssueSnapshot,
    repository: &str,
    number: u64,
) -> Result<()> {
    let actual_repository =
        crate::github::GitHubConfig::normalize_repo(&issue.repo)?.to_ascii_lowercase();
    if actual_repository != repository || issue.number != number {
        return Err(OrbitError::invalid_argument(format!(
            "GitHub returned {actual_repository}#{} while reattaching {repository}#{number}",
            issue.number
        )));
    }
    Ok(())
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

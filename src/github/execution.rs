//! Synchronization policy and execution behind one injected GitHub seam.
//! The caller holds the Board lock for this complete operation.
use super::client::{CreateIssueInput, GitHubClient, UpdateIssueInput};
use super::config::IssueFieldConfig;
use super::fields::parse_area;
use super::link::GitHubLink;
use super::model::{IssueSnapshot, IssueState, SyncDirection, SyncPlan, SyncResolution};
use super::sync::OrbitSyncView;
use super::{relationships, sync};
use crate::board::Board;
use crate::error::{OrbitError, Result, code};
use crate::graph::{Graph, LoadMode};
use crate::task::Task;
use chrono::Utc;
use serde::Serialize;
use serde_json::json;

pub struct SyncRequest<'a> {
    pub task: Option<&'a str>,
    pub direction: SyncDirection,
    pub resolutions: &'a [String],
    pub apply: bool,
}

/// A per-Task apply failure. The Task's baseline is not advanced, so retrying is safe.
#[derive(Clone, Debug, Serialize)]
pub struct SyncFailure {
    pub task: String,
    pub repository: String,
    pub number: u64,
    pub code: &'static str,
    pub message: String,
    pub retry_safe: bool,
}

/// What happened to one Task: nothing was attempted (preview), it applied, or it failed.
pub enum SyncOutcome {
    Preview,
    Applied,
    Failed(SyncFailure),
}

pub struct SyncTaskResult {
    pub task: Task,
    pub github: GitHubLink,
    pub plan: SyncPlan,
    pub relationships: relationships::RelationshipReport,
    pub outcome: SyncOutcome,
}

pub struct SyncReport {
    pub warnings: Vec<String>,
    pub tasks: Vec<SyncTaskResult>,
    pub failures: Vec<SyncFailure>,
    pub success_count: usize,
}

/// Preflight every target before writing; execution failures remain per-Task and retryable.
pub fn synchronize(
    board: &Board,
    client: &dyn GitHubClient,
    request: SyncRequest<'_>,
) -> Result<SyncReport> {
    let direction = request.direction;
    let resolutions = parse_sync_resolutions(request.resolutions)?;
    let graph = Graph::load(board, LoadMode::Strict)?.0;
    let targets: Vec<String> = if let Some(task) = request.task.map(str::to_owned) {
        vec![task]
    } else {
        graph
            .iter()
            .filter(|task| task.github_link().is_some())
            .map(|task| task.id.clone())
            .collect()
    };
    if targets.is_empty() {
        return Ok(SyncReport {
            tasks: Vec::new(),
            warnings: Vec::new(),
            failures: Vec::new(),
            success_count: 0,
        });
    }

    let mut prepared = Vec::new();
    let mut resolved_fields = std::collections::BTreeSet::new();
    for id in &targets {
        let task = graph.get(id)?;
        let link = task
            .github_link()
            .ok_or_else(|| OrbitError::invalid_argument(format!("task {id} has no GitHub link")))?
            .clone();
        require_repository_available(board, &link.repo)?;
        if board.config.github.mode.is_basic() && direction != SyncDirection::Pull {
            reject_basic_metadata(task)?;
        }
        let issue = fetch_issue(board, client, &link.repo, link.number)?;
        let mut plan = sync::plan_sync(OrbitSyncView { task, link: &link }, &issue, direction);
        resolved_fields.extend(resolve_sync_conflicts(
            &mut plan,
            task,
            &issue,
            &resolutions,
        )?);
        let children: Vec<_> = graph
            .iter()
            .filter(|candidate| candidate.front.parent.as_deref() == Some(task.id.as_str()))
            .filter_map(|candidate| {
                candidate
                    .github_link()
                    .map(|child_link| (candidate.clone(), child_link.clone()))
            })
            .collect();
        let dependencies: Vec<_> = task
            .front
            .depends_on
            .iter()
            .filter_map(|dependency| graph.tasks.get(dependency))
            .filter_map(|dependency| {
                dependency
                    .github_link()
                    .map(|dependency_link| (dependency.clone(), dependency_link.clone()))
            })
            .collect();
        let mut relationship = relationships::plan_relationship_sync(
            &link,
            &(&issue).into(),
            &children,
            &dependencies,
        );
        if direction == SyncDirection::Pull {
            relationship.changes.clear();
        }
        prepared.push((task.clone(), link, issue, plan, relationship));
    }

    let unused_resolutions: Vec<_> = resolutions
        .keys()
        .filter(|field| !resolved_fields.contains(*field))
        .cloned()
        .collect();
    if !unused_resolutions.is_empty() {
        return Err(OrbitError::invalid_argument(format!(
            "no matching sync conflict for resolution(s): {}",
            unused_resolutions.join(", ")
        )));
    }

    let unresolved: Vec<_> = prepared
        .iter()
        .filter(|(_, _, _, plan, _)| plan.has_conflicts())
        .map(|(task, link, _, plan, _)| {
            json!({
                "task": task.id,
                "repository": link.repo,
                "number": link.number,
                "conflicts": plan.conflicts,
            })
        })
        .collect();
    if request.apply && !unresolved.is_empty() {
        let first = &unresolved[0];
        return Err(OrbitError::new(
            code::GITHUB_ISSUE_CONFLICT,
            "sync has unresolved field conflicts; resolve each field before applying",
        )
        .with_details(json!({
            "task": first["task"],
            "repository": first["repository"],
            "number": first["number"],
            "conflicts": first["conflicts"],
            "tasks": unresolved,
        })));
    }

    let mut plans = Vec::with_capacity(prepared.len());
    let mut failures = Vec::new();
    let mut successes = 0usize;
    let mut warnings = Vec::new();
    for (task, link, issue, plan, relationship) in prepared {
        let mut item = SyncTaskResult {
            task: task.clone(),
            github: link.clone(),
            plan: plan.clone(),
            relationships: relationship.clone(),
            outcome: SyncOutcome::Preview,
        };
        if request.apply {
            match apply_plan(
                board,
                client,
                ApplyPlanRequest {
                    task: &task,
                    link: &link,
                    issue: &issue,
                    plan: &plan,
                    relationship: &relationship,
                    advance_baseline: true,
                    commit_message: &format!("orbit: github sync {}", task.id),
                },
            ) {
                Ok(warning) => {
                    warnings.extend(warning);
                    successes += 1;
                    item.outcome = SyncOutcome::Applied;
                }
                Err(error) => {
                    let failure = SyncFailure {
                        task: task.id.clone(),
                        repository: link.repo.clone(),
                        number: link.number,
                        code: error.code,
                        message: error.message,
                        retry_safe: true,
                    };
                    failures.push(failure.clone());
                    item.outcome = SyncOutcome::Failed(failure);
                }
            }
        }
        plans.push(item);
    }

    Ok(SyncReport {
        tasks: plans,
        warnings,
        failures,
        success_count: successes,
    })
}

pub(crate) fn parse_sync_resolutions(
    values: &[String],
) -> Result<std::collections::BTreeMap<String, SyncResolution>> {
    let mut resolutions = std::collections::BTreeMap::new();
    for value in values {
        let (field, choice) = value.split_once('=').ok_or_else(|| {
            OrbitError::invalid_argument(format!(
                "invalid conflict resolution `{value}`; expected FIELD=orbit or FIELD=github"
            ))
        })?;
        let resolution = match choice {
            "orbit" => SyncResolution::PreferOrbit,
            "github" => SyncResolution::PreferGitHub,
            _ => {
                return Err(OrbitError::invalid_argument(format!(
                    "invalid conflict resolution `{value}`; choose `orbit` or `github`"
                )));
            }
        };
        if !matches!(
            field,
            "title" | "body" | "status" | "labels" | "area" | "verification"
        ) {
            return Err(OrbitError::invalid_argument(format!(
                "unknown sync conflict field `{field}`"
            )));
        }
        if resolutions.insert(field.to_string(), resolution).is_some() {
            return Err(OrbitError::invalid_argument(format!(
                "sync field `{field}` has more than one resolution"
            )));
        }
    }
    Ok(resolutions)
}

fn resolve_sync_conflicts(
    plan: &mut SyncPlan,
    task: &Task,
    issue: &IssueSnapshot,
    resolutions: &std::collections::BTreeMap<String, SyncResolution>,
) -> Result<std::collections::BTreeSet<String>> {
    let conflicted: std::collections::BTreeSet<_> = plan
        .conflicts
        .iter()
        .map(|conflict| conflict.field.clone())
        .collect();
    let mut resolved = std::collections::BTreeSet::new();
    for (field, resolution) in resolutions {
        if !conflicted.contains(field) {
            continue;
        }
        resolved.insert(field.clone());
        match (field.as_str(), resolution) {
            ("title", SyncResolution::PreferOrbit) => {
                plan.push_title = Some(task.front.title.clone())
            }
            ("title", SyncResolution::PreferGitHub) => plan.apply_title = Some(issue.title.clone()),
            ("body", SyncResolution::PreferOrbit) => plan.push_body = Some(task.body.clone()),
            ("body", SyncResolution::PreferGitHub) => plan.apply_body = Some(issue.body.clone()),
            ("status", SyncResolution::PreferOrbit) => {
                plan.push_state = Some(crate::github::status::issue_state_from_status(
                    task.front.status,
                ));
                plan.push_state_reason =
                    crate::github::status::issue_state_reason_from_status(task.front.status);
                plan.push_status_label = true;
            }
            ("status", SyncResolution::PreferGitHub) => {
                plan.apply_status = Some(crate::github::status::status_from_issue(
                    issue.state,
                    issue.state_reason,
                    &issue.labels,
                ));
            }
            ("labels", SyncResolution::PreferOrbit) => {
                plan.push_labels = Some(task.front.labels.clone())
            }
            ("labels", SyncResolution::PreferGitHub) => {
                plan.apply_labels = Some(issue.labels.clone())
            }
            ("area", SyncResolution::PreferOrbit) => {
                plan.push_area = Some(task.front.area.map(|area| area.as_str().to_string()));
            }
            ("area", SyncResolution::PreferGitHub) => plan.apply_area = Some(issue.area.clone()),
            ("verification", SyncResolution::PreferOrbit) => {
                plan.push_verification = Some(task.front.verification.clone());
            }
            ("verification", SyncResolution::PreferGitHub) => {
                plan.apply_verification = Some(issue.verification.clone());
            }
            (_, SyncResolution::Skip) => {}
            _ => unreachable!("sync resolution fields are validated when parsed"),
        }
        plan.conflicts.retain(|conflict| conflict.field != *field);
    }
    Ok(resolved)
}

pub(crate) struct ApplyPlanRequest<'a> {
    pub task: &'a Task,
    pub link: &'a GitHubLink,
    pub issue: &'a IssueSnapshot,
    pub plan: &'a SyncPlan,
    pub relationship: &'a relationships::RelationshipReport,
    pub advance_baseline: bool,
    pub commit_message: &'a str,
}

pub(crate) fn apply_plan(
    board: &Board,
    client: &dyn GitHubClient,
    request: ApplyPlanRequest<'_>,
) -> Result<Option<String>> {
    let ApplyPlanRequest {
        task,
        link,
        issue,
        plan,
        relationship,
        advance_baseline,
        commit_message,
    } = request;
    let mut task = task.clone();
    if let Some(title) = &plan.apply_title {
        task.front.title = title.clone();
    }
    if let Some(body) = &plan.apply_body {
        task.body = body.clone();
    }
    if let Some(status) = plan.apply_status {
        task.front.status = status;
    }
    if let Some(labels) = &plan.apply_labels {
        task.front.labels = crate::github::status::user_labels(labels);
    }
    if let Some(area) = &plan.apply_area {
        task.front.area = area.as_ref().map(|value| parse_area(value)).transpose()?;
    }
    if let Some(verification) = &plan.apply_verification {
        task.front.verification = verification.clone();
    }
    task.front.updated_at = Utc::now();

    let labels = if let Some(labels) = &plan.push_labels {
        Some(crate::github::status::labels_for_status(
            labels,
            task.front.status,
        ))
    } else if plan.push_status_label {
        Some(crate::github::status::labels_for_status(
            &crate::github::status::user_labels(&issue.labels),
            task.front.status,
        ))
    } else {
        None
    };

    if plan.push_title.is_some()
        || plan.push_body.is_some()
        || plan.push_state.is_some()
        || labels.is_some()
        || plan.push_area.is_some()
        || plan.push_verification.is_some()
    {
        let metadata_update = plan.push_area.is_some() || plan.push_verification.is_some();
        client.update_issue(
            &link.repo,
            link.number,
            &UpdateIssueInput {
                title: plan.push_title.clone(),
                body: plan.push_body.clone(),
                state: plan.push_state,
                state_reason: plan.push_state_reason,
                labels,
                area: metadata_update
                    .then(|| plan.push_area.clone().unwrap_or_else(|| issue.area.clone())),
                verification: metadata_update.then(|| {
                    plan.push_verification
                        .clone()
                        .unwrap_or_else(|| issue.verification.clone())
                }),
                organization: board.config.github.organization.clone(),
                issue_fields: issue_fields_for(board).cloned(),
            },
        )?;
    }

    let removals = relationship
        .changes
        .iter()
        .filter(|change| change.action == "remove")
        .cloned()
        .collect::<Vec<_>>();
    if plan.direction != SyncDirection::Pull
        && (!relationship.supported.is_empty() || !removals.is_empty())
    {
        client.update_relationships(&link.repo, link.number, &relationship.supported, &removals)?;
    }

    if !advance_baseline {
        return Ok(None);
    }

    let refreshed = fetch_issue(board, client, &link.repo, link.number)?;
    set_baseline(&mut task, &refreshed);
    let outcome = persist_task(board, &task, commit_message)?;
    Ok(crate::mutation::commit_warning(outcome))
}

pub(crate) fn repository_available(board: &Board, repository: &str) -> Result<bool> {
    if let Some(repositories) = crate::workspace::export_repositories(board)? {
        return Ok(repositories
            .iter()
            .any(|candidate| candidate.eq_ignore_ascii_case(repository))
            || board.config.github.contains_repo(repository));
    }
    Ok(board.config.github.contains_repo(repository))
}

pub(crate) fn require_repository_available(board: &Board, repository: &str) -> Result<()> {
    if repository_available(board, repository)? {
        return Ok(());
    }
    let message = format!(
        "repository `{repository}` is not available to this Board; run `orbit github repo link {repository}`"
    );
    Err(
        OrbitError::new(code::GITHUB_REPO_NOT_LINKED, message).with_details(json!({
            "repository": repository,
            "remediation": format!("run `orbit github repo link {repository}`"),
        })),
    )
}

pub(crate) fn reject_basic_metadata(task: &Task) -> Result<()> {
    let fields = [
        task.front.area.map(|area| area.as_str().to_string()),
        task.front.verification.clone(),
    ];
    let present: Vec<_> = ["area", "verification"]
        .into_iter()
        .zip(fields)
        .filter_map(|(name, value)| value.map(|_| name))
        .collect();
    if present.is_empty() {
        return Ok(());
    }
    Err(OrbitError::invalid_argument(format!(
        "GitHub basic mode cannot sync task {} field(s): {}; remove them or use mode = `full` with organization Issue Fields",
        task.id,
        present.join(", ")
    )))
}

pub(crate) fn omit_issue_fields(issue: &mut IssueSnapshot) {
    // REST Issues for personal repositories do not expose organization Issue
    // Fields. Ignore fixture/legacy values rather than importing them.
    issue.area = None;
    issue.verification = None;
}

/// The configured Issue Fields, or `None` in basic mode where they are never
/// requested. One expression for the mode gate every remote read shares.
pub(crate) fn issue_fields_for(board: &Board) -> Option<&IssueFieldConfig> {
    (!board.config.github.mode.is_basic()).then_some(&board.config.github.issue_fields)
}

/// Reads a remote Issue honoring the Board's mode: basic mode never requests
/// configured Issue Fields and strips any that arrive anyway.
pub(crate) fn fetch_issue(
    board: &Board,
    client: &dyn GitHubClient,
    repo: &str,
    number: u64,
) -> Result<IssueSnapshot> {
    let mut issue = client.get_issue(repo, number, issue_fields_for(board))?;
    if board.config.github.mode.is_basic() {
        omit_issue_fields(&mut issue);
    }
    Ok(issue)
}

pub(crate) fn set_baseline(task: &mut Task, issue: &IssueSnapshot) {
    let Some(link) = task.front.github.as_ref() else {
        return;
    };
    let baseline = crate::github::sync::baseline_from(
        crate::github::sync::OrbitSyncView { task, link },
        issue,
    );
    task.front
        .github
        .as_mut()
        .expect("link existed above")
        .last_sync = Some(baseline);
}

/// A follow-up step that failed after the Issue already existed.
pub enum CreateFollowUp {
    /// The link and baseline were not written; the Issue exists but is unlinked.
    LinkNotPersisted(OrbitError),
    /// The Task is terminal but the Issue could not be closed.
    CloseFailed(OrbitError),
    /// The Remote Issue cache was not updated.
    CatalogUpdateFailed(OrbitError),
}

pub struct CreatedIssue {
    /// The Task with its link (and baseline, when persisted).
    pub task: Task,
    /// The Issue as last known: closed when the Task is terminal and the close succeeded.
    pub issue: IssueSnapshot,
    pub failures: Vec<CreateFollowUp>,
    /// Commit warnings from the writes that did persist.
    pub warnings: Vec<String>,
}

/// Creates the Issue for an unlinked Task, links it, and closes it when the Task is
/// terminal. `Err` means nothing was created; later failures are in `failures`.
pub(crate) fn create_and_link(
    board: &Board,
    client: &dyn GitHubClient,
    repo: &str,
    task: &Task,
    commit_message: &str,
) -> Result<CreatedIssue> {
    require_repository_available(board, repo)?;
    if board.config.github.mode.is_basic() {
        reject_basic_metadata(task)?;
    }
    let mut issue = client.create_issue(
        repo,
        &CreateIssueInput {
            title: task.front.title.clone(),
            body: task.body.clone(),
            labels: super::status::labels_for_status(&task.front.labels, task.front.status),
            area: task.front.area.map(|area| area.as_str().to_string()),
            verification: task.front.verification.clone(),
            organization: board.config.github.organization.clone(),
            issue_fields: board.config.github.issue_fields.clone(),
        },
    )?;

    let mut task = task.clone();
    let mut failures = Vec::new();
    let mut warnings = Vec::new();
    task.front.github = Some(GitHubLink::new(repo.to_string(), issue.number));
    task.front.updated_at = Utc::now();
    set_baseline(&mut task, &issue);
    match persist_task(board, &task, commit_message) {
        Ok(outcome) => warnings.extend(crate::mutation::commit_warning(outcome)),
        Err(error) => failures.push(CreateFollowUp::LinkNotPersisted(error)),
    }
    let linked = failures.is_empty();

    let remember = |issue: &IssueSnapshot, failures: &mut Vec<CreateFollowUp>| {
        if let Err(error) = crate::issue_catalog::remember_created_issue(board, repo, issue)
            && !failures
                .iter()
                .any(|failure| matches!(failure, CreateFollowUp::CatalogUpdateFailed(_)))
        {
            failures.push(CreateFollowUp::CatalogUpdateFailed(error));
        }
    };
    remember(&issue, &mut failures);

    if task.front.status.is_terminal() {
        let close = client.update_issue(
            repo,
            issue.number,
            &UpdateIssueInput {
                state: Some(IssueState::Closed),
                state_reason: super::status::issue_state_reason_from_status(task.front.status),
                title: None,
                body: None,
                labels: None,
                area: None,
                verification: None,
                organization: None,
                issue_fields: None,
            },
        );
        match close {
            Ok(closed) => {
                issue = closed;
                if linked {
                    set_baseline(&mut task, &issue);
                    match persist_task(board, &task, commit_message) {
                        Ok(outcome) => warnings.extend(crate::mutation::commit_warning(outcome)),
                        Err(error) => warnings.push(format!(
                            "task {}: closed {repo}#{} but could not record the sync baseline: {}; run `orbit github sync {} --apply`",
                            task.id, issue.number, error.message, task.id
                        )),
                    }
                }
                remember(&issue, &mut failures);
            }
            Err(error) => failures.push(CreateFollowUp::CloseFailed(error)),
        }
    }
    Ok(CreatedIssue {
        task,
        issue,
        failures,
        warnings,
    })
}

use crate::mutation::persist_task;

#[cfg(test)]
mod create_tests {
    use super::*;
    use crate::board::Config;
    use crate::github::client::FixtureClient;
    use crate::task::{Status, TaskFrontmatter, TaskType};
    use std::collections::BTreeMap;
    use std::path::Path;

    const REPO: &str = "acme/widget";

    fn task(status: Status) -> Task {
        Task {
            id: "orb-a7b2c9".to_string(),
            front: TaskFrontmatter {
                title: "Ship it".to_string(),
                status,
                task_type: TaskType::Task,
                priority: 1,
                labels: Vec::new(),
                depends_on: Vec::new(),
                parent: None,
                waiting_on: None,
                checkpoint: None,
                area: None,
                verification: None,
                github: None,
                remote: None,
                repo: None,
                claim: None,
                stray_claim_times: None,
                created_at: Utc::now(),
                updated_at: Utc::now(),
                extra: BTreeMap::new(),
            },
            body: "Body".to_string(),
        }
    }

    /// A Board over a plain temp dir (commits report `Failed`, which is a warning)
    /// and a fixture client whose store is `fixture`.
    fn setup(root: &Path, fixture: serde_json::Value) -> (Board, FixtureClient) {
        let mut config = Config::default();
        config.github.repositories = vec![REPO.to_string()];
        let board = Board {
            root: root.to_path_buf(),
            lock_path: root.join(".lock"),
            config,
            warnings: Vec::new(),
            orbit_dir: None,
            commit_failed: Default::default(),
        };
        let path = root.join("fixture.json");
        std::fs::write(&path, fixture.to_string()).unwrap();
        (board, FixtureClient::load(&path).unwrap())
    }

    #[test]
    fn plain_create_links_the_task() {
        let dir = tempfile::tempdir().unwrap();
        let (board, client) = setup(dir.path(), json!({}));
        let created = create_and_link(&board, &client, REPO, &task(Status::Todo), "msg").unwrap();
        assert!(created.failures.is_empty());
        assert_eq!(created.issue.state, IssueState::Open);
        let link = created.task.front.github.as_ref().unwrap();
        assert_eq!(
            (link.repo.as_str(), link.number),
            (REPO, created.issue.number)
        );
        assert!(link.last_sync.is_some());
        assert!(board.task_path("orb-a7b2c9").exists());
    }

    #[test]
    fn terminal_create_closes_the_issue() {
        let dir = tempfile::tempdir().unwrap();
        let (board, client) = setup(dir.path(), json!({}));
        let created = create_and_link(&board, &client, REPO, &task(Status::Done), "msg").unwrap();
        assert!(created.failures.is_empty());
        assert_eq!(created.issue.state, IssueState::Closed);
        let remote = client.get_issue(REPO, created.issue.number, None).unwrap();
        assert_eq!(remote.state, IssueState::Closed);
    }

    #[test]
    fn close_failure_keeps_the_link() {
        let dir = tempfile::tempdir().unwrap();
        let (board, client) = setup(dir.path(), json!({"update_failures": {REPO: "nope"}}));
        let created = create_and_link(&board, &client, REPO, &task(Status::Done), "msg").unwrap();
        assert!(matches!(
            created.failures.as_slice(),
            [CreateFollowUp::CloseFailed(_)]
        ));
        assert!(created.task.front.github.is_some());
        assert!(board.task_path("orb-a7b2c9").exists());
    }

    #[test]
    fn link_persist_failure_reports_the_created_issue() {
        let dir = tempfile::tempdir().unwrap();
        let (mut board, client) = setup(dir.path(), json!({}));
        // A file where the Board root should be makes every Task write fail.
        let blocker = dir.path().join("not-a-dir");
        std::fs::write(&blocker, "").unwrap();
        board.root = blocker;
        let created = create_and_link(&board, &client, REPO, &task(Status::Todo), "msg").unwrap();
        assert!(matches!(
            created.failures.as_slice(),
            [CreateFollowUp::LinkNotPersisted(_)]
        ));
        assert_eq!(created.issue.number, 1);
    }

    #[test]
    fn create_failure_is_an_error() {
        let dir = tempfile::tempdir().unwrap();
        let (board, client) = setup(dir.path(), json!({"create_failures": {REPO: "denied"}}));
        assert!(create_and_link(&board, &client, REPO, &task(Status::Todo), "msg").is_err());
    }
}

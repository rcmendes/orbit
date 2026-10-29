//! Synchronization policy and execution behind one injected GitHub seam.
//! The caller holds the Board lock for this complete operation.
use super::client::{GitHubClient, UpdateIssueInput};
use super::fields::parse_area;
use super::link::{GitHubLink, GitHubSyncBaseline};
use super::model::{IssueSnapshot, SyncDirection, SyncPlan, SyncResolution};
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
        let mut issue = client.get_issue(
            &link.repo,
            link.number,
            (!board.config.github.mode.is_basic()).then_some(&board.config.github.issue_fields),
        )?;
        if board.config.github.mode.is_basic() {
            omit_issue_fields(&mut issue);
        }
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
        let mut relationship =
            relationships::plan_relationship_sync(task, &link, &issue, &children, &dependencies);
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
            match apply_plan(board, &task, &link, &issue, &plan, &relationship, client) {
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
            }
            ("status", SyncResolution::PreferGitHub) => {
                plan.apply_status = Some(crate::github::status::status_from_issue(
                    issue.state,
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

fn apply_plan(
    board: &Board,
    task: &Task,
    link: &GitHubLink,
    issue: &IssueSnapshot,
    plan: &SyncPlan,
    relationship: &relationships::RelationshipReport,
    client: &dyn GitHubClient,
) -> Result<Option<String>> {
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
        task.front.labels = labels.clone();
    }
    if let Some(area) = &plan.apply_area {
        task.front.area = area.as_ref().map(|value| parse_area(value)).transpose()?;
    }
    if let Some(verification) = &plan.apply_verification {
        task.front.verification = verification.clone();
    }
    task.front.updated_at = Utc::now();

    if plan.push_title.is_some()
        || plan.push_body.is_some()
        || plan.push_labels.is_some()
        || plan.push_area.is_some()
        || plan.push_verification.is_some()
        || plan.push_state.is_some()
    {
        let metadata_update = plan.push_area.is_some() || plan.push_verification.is_some();
        client.update_issue(
            &link.repo,
            link.number,
            &UpdateIssueInput {
                title: plan.push_title.clone(),
                body: plan.push_body.clone(),
                state: plan.push_state,
                labels: plan.push_labels.clone(),
                area: metadata_update
                    .then(|| plan.push_area.clone().unwrap_or_else(|| issue.area.clone())),
                verification: metadata_update.then(|| {
                    plan.push_verification
                        .clone()
                        .unwrap_or_else(|| issue.verification.clone())
                }),
                organization: board.config.github.organization.clone(),
                issue_fields: (!board.config.github.mode.is_basic())
                    .then_some(board.config.github.issue_fields.clone()),
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

    let refreshed = client.get_issue(
        &link.repo,
        link.number,
        (!board.config.github.mode.is_basic()).then_some(&board.config.github.issue_fields),
    )?;
    set_baseline(&mut task, &refreshed);
    let outcome = persist_task(board, &task, &format!("orbit: github sync {}", task.id))?;
    Ok(crate::mutation::commit_warning(outcome))
}

pub(crate) fn repository_available(board: &Board, repository: &str) -> Result<bool> {
    if let Some(repositories) = crate::workspace::export_repositories(board)? {
        return Ok(repositories
            .iter()
            .any(|candidate| candidate.eq_ignore_ascii_case(repository)));
    }
    Ok(board.config.github.contains_repo(repository))
}

pub(crate) fn require_repository_available(board: &Board, repository: &str) -> Result<()> {
    if repository_available(board, repository)? {
        return Ok(());
    }
    let message = if board.is_workspace_board() {
        format!(
            "repository `{repository}` is not included in this Workspace; run `orbit workspace refresh` or include it"
        )
    } else {
        format!(
            "repository `{repository}` is not linked to this Board; run `orbit github repo link {repository}`"
        )
    };
    Err(
        OrbitError::new(code::GITHUB_REPO_NOT_LINKED, message).with_details(json!({
            "repository": repository,
            "remediation": if board.is_workspace_board() {
                "run `orbit workspace refresh` or `orbit workspace include OWNER/REPO`"
            } else {
                "run `orbit github repo link OWNER/REPO`"
            },
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

pub(crate) fn set_baseline(task: &mut Task, issue: &IssueSnapshot) {
    if let Some(link) = task.front.github.as_mut() {
        link.last_sync = Some(GitHubSyncBaseline {
            at: Utc::now(),
            issue_updated_at: issue.updated_at,
            orbit_updated_at: task.front.updated_at,
            title: issue.title.clone(),
            body: issue.body.clone(),
            labels: issue.labels.clone(),
            area: task.front.area.map(|area| area.as_str().to_string()),
            verification: task.front.verification.clone(),
            status: Some(task.front.status),
            issue_state: Some(issue.state),
            sub_issues: issue.blocking.clone(),
            blocked_by: issue.blocked_by.clone(),
        });
    }
}

use crate::mutation::persist_task;

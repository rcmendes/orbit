//! Sync execution behind the Remote Tracker port (ADR 0018).
//! Core owns baselines, three-way conflict detection, previews, and
//! persistence; adapters only translate Remote Issues. The caller holds
//! the Board lock for this complete operation.

use std::collections::{BTreeMap, BTreeSet};

use chrono::Utc;
use serde::Serialize;
use serde_json::json;

use crate::board::Board;
use crate::error::{OrbitError, Result, code};
use crate::graph::{Graph, LoadMode};
use crate::mutation::persist_task;
use crate::remote::RemoteProject;
use crate::remote::link::{active_link, encode_baseline, link_mut, link_view, materialize};
use crate::remote::model::{
    RemoteIssue, RemoteIssueFields, RemoteIssueUpdate, SyncDirection, SyncPlan, SyncResolution,
};
use crate::remote::port::{RemoteAdapter, RemoteResolver};
use crate::remote::relationships::{self, RelationshipReport, RemoteRelationsView};
use crate::remote::sync::{self, OrbitSyncView};
use crate::task::Task;

pub struct SyncRequest<'a> {
    pub task: Option<&'a str>,
    pub direction: SyncDirection,
    pub resolutions: &'a [String],
    pub apply: bool,
    /// Restrict targets to links on this tracker (the GitHub adapter path passes
    /// `"github"`); `None` syncs every linked Task.
    pub tracker: Option<&'a str>,
}

/// A per-Task apply failure. The Task's baseline is not advanced, so retrying is safe.
#[derive(Clone, Debug, Serialize)]
pub struct SyncFailure {
    pub task: String,
    pub tracker: String,
    pub project: String,
    pub id: String,
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
    pub link: crate::remote::link::RemoteLinkView,
    pub plan: SyncPlan,
    pub relationships: RelationshipReport,
    pub outcome: SyncOutcome,
}

pub struct SyncReport {
    pub warnings: Vec<String>,
    pub tasks: Vec<SyncTaskResult>,
    pub failures: Vec<SyncFailure>,
    pub success_count: usize,
}

/// Resolve each needed tracker once per operation.
struct Adapters<'a, R: RemoteResolver + ?Sized> {
    resolver: &'a R,
    cache: BTreeMap<String, Box<dyn RemoteAdapter>>,
}

impl<'a, R: RemoteResolver + ?Sized> Adapters<'a, R> {
    fn new(resolver: &'a R) -> Self {
        Self {
            resolver,
            cache: BTreeMap::new(),
        }
    }

    fn get(&mut self, tracker: &str) -> Result<&dyn RemoteAdapter> {
        if !self.cache.contains_key(tracker) {
            let adapter = self.resolver.adapter(tracker)?;
            self.cache.insert(tracker.to_string(), adapter);
        }
        Ok(self
            .cache
            .get(tracker)
            .expect("adapter cached above")
            .as_ref())
    }
}

/// Preflight every target before writing; execution failures remain per-Task and retryable.
pub fn synchronize<R: RemoteResolver + ?Sized>(
    board: &Board,
    resolver: &R,
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
            .filter(|task| {
                active_link(task).is_some_and(|link| {
                    request
                        .tracker
                        .is_none_or(|tracker| link.tracker == tracker)
                })
            })
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

    let mut adapters = Adapters::new(resolver);
    let mut prepared = Vec::new();
    let mut resolved_fields = BTreeSet::new();
    for id in &targets {
        let task = graph.get(id)?;
        let link = active_link(task)
            .filter(|link| {
                request
                    .tracker
                    .is_none_or(|tracker| link.tracker == tracker)
            })
            .ok_or_else(|| {
                OrbitError::invalid_argument(format!("task {id} has no Remote Issue link"))
            })?
            .clone();
        let project = link.remote_project();
        let adapter = adapters.get(&link.tracker)?;
        require_project_available(board, resolver, &project)?;
        if direction != SyncDirection::Pull {
            sync::reject_unmapped_metadata(adapter, &project, task)?;
        }
        let issue = adapter.fetch_issue(&project, &link.id)?;
        let mut plan = sync::plan_sync(
            adapter,
            OrbitSyncView { task, link: &link },
            &issue,
            direction,
        );
        resolved_fields.extend(resolve_sync_conflicts(
            adapter,
            &mut plan,
            task,
            &issue,
            &resolutions,
        )?);
        let children: Vec<_> = graph
            .iter()
            .filter(|candidate| candidate.front.parent.as_deref() == Some(task.id.as_str()))
            .filter_map(|candidate| active_link(candidate).map(|l| (candidate.clone(), l)))
            .collect();
        let dependencies: Vec<_> = task
            .front
            .depends_on
            .iter()
            .filter_map(|dependency| graph.tasks.get(dependency))
            .filter_map(|dependency| active_link(dependency).map(|l| (dependency.clone(), l)))
            .collect();
        let mut relationship = if adapter.supports_relationships() {
            relationships::plan_relationship_sync(
                &link,
                &RemoteRelationsView::from(&issue.relations),
                &children,
                &dependencies,
            )
        } else {
            RelationshipReport {
                supported: Vec::new(),
                unsupported: Vec::new(),
                changes: Vec::new(),
            }
        };
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
                "tracker": link.tracker,
                "project": link.project,
                "id": link.id,
                "conflicts": plan.conflicts,
            })
        })
        .collect();
    if request.apply && !unresolved.is_empty() {
        let first = &unresolved[0];
        return Err(OrbitError::new(
            code::REMOTE_ISSUE_CONFLICT,
            "sync has unresolved field conflicts; resolve each field before applying",
        )
        .with_details(json!({
            "task": first["task"],
            "tracker": first["tracker"],
            "project": first["project"],
            "id": first["id"],
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
            link: link.clone(),
            plan: plan.clone(),
            relationships: relationship.clone(),
            outcome: SyncOutcome::Preview,
        };
        if request.apply {
            let adapter = match adapters.get(&link.tracker) {
                Ok(adapter) => adapter,
                Err(error) => {
                    let failure = SyncFailure {
                        task: task.id.clone(),
                        tracker: link.tracker.clone(),
                        project: link.project.clone(),
                        id: link.id.clone(),
                        code: error.code,
                        message: error.message,
                        retry_safe: true,
                    };
                    failures.push(failure.clone());
                    item.outcome = SyncOutcome::Failed(failure);
                    plans.push(item);
                    continue;
                }
            };
            match apply_plan(
                board,
                adapter,
                ApplyPlanRequest {
                    task: &task,
                    link: &link,
                    issue: &issue,
                    plan: &plan,
                    relationship: &relationship,
                    advance_baseline: true,
                    commit_message: &format!("orbit: remote sync {}", task.id),
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
                        tracker: link.tracker.clone(),
                        project: link.project.clone(),
                        id: link.id.clone(),
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

pub fn parse_sync_resolutions(values: &[String]) -> Result<BTreeMap<String, SyncResolution>> {
    let mut resolutions = BTreeMap::new();
    for value in values {
        let (field, choice) = value.split_once('=').ok_or_else(|| {
            OrbitError::invalid_argument(format!(
                "invalid conflict resolution `{value}`; expected FIELD=orbit or FIELD=remote"
            ))
        })?;
        let resolution = match choice {
            "orbit" => SyncResolution::PreferOrbit,
            "remote" => SyncResolution::PreferRemote,
            _ => {
                return Err(OrbitError::invalid_argument(format!(
                    "invalid conflict resolution `{value}`; choose `orbit` or `remote`"
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

#[allow(clippy::too_many_arguments)]
fn resolve_sync_conflicts(
    adapter: &dyn RemoteAdapter,
    plan: &mut SyncPlan,
    task: &Task,
    issue: &RemoteIssue,
    resolutions: &BTreeMap<String, SyncResolution>,
) -> Result<BTreeSet<String>> {
    let conflicted: BTreeSet<_> = plan
        .conflicts
        .iter()
        .map(|conflict| conflict.field.clone())
        .collect();
    let mut resolved = BTreeSet::new();
    for (field, resolution) in resolutions {
        if !conflicted.contains(field) {
            continue;
        }
        resolved.insert(field.clone());
        match (field.as_str(), resolution) {
            ("title", SyncResolution::PreferOrbit) => {
                plan.push_title = Some(task.front.title.clone())
            }
            ("title", SyncResolution::PreferRemote) => plan.apply_title = Some(issue.title.clone()),
            ("body", SyncResolution::PreferOrbit) => plan.push_body = Some(task.body.clone()),
            ("body", SyncResolution::PreferRemote) => plan.apply_body = Some(issue.body.clone()),
            ("status", SyncResolution::PreferOrbit) => plan.push_status = Some(task.front.status),
            ("status", SyncResolution::PreferRemote) => plan.apply_status = Some(issue.status),
            ("labels", SyncResolution::PreferOrbit) => {
                plan.push_labels = Some(task.front.labels.clone())
            }
            ("labels", SyncResolution::PreferRemote) => {
                plan.apply_labels = Some(issue.labels.clone())
            }
            ("area", SyncResolution::PreferOrbit) => {
                plan.push_area = Some(task.front.area.clone());
            }
            ("area", SyncResolution::PreferRemote) => plan.apply_area = Some(issue.area.clone()),
            ("verification", SyncResolution::PreferOrbit) => {
                plan.push_verification = Some(task.front.verification.clone());
            }
            ("verification", SyncResolution::PreferRemote) => {
                plan.apply_verification = Some(issue.verification.clone())
            }
            (_, SyncResolution::Skip) => {}
            _ => unreachable!("sync resolution fields are validated when parsed"),
        }
        let _ = adapter;
        plan.conflicts.retain(|conflict| conflict.field != *field);
    }
    Ok(resolved)
}

pub(crate) struct ApplyPlanRequest<'a> {
    pub task: &'a Task,
    pub link: &'a crate::remote::link::RemoteLinkView,
    pub issue: &'a RemoteIssue,
    pub plan: &'a SyncPlan,
    pub relationship: &'a RelationshipReport,
    pub advance_baseline: bool,
    pub commit_message: &'a str,
}

pub(crate) fn apply_plan(
    board: &Board,
    adapter: &dyn RemoteAdapter,
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
    let project = link.remote_project();
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
        if let Some(value) = area
            && !crate::task::is_valid_area(value)
        {
            return Err(OrbitError::invalid_argument(format!(
                "the remote tracker returned an invalid area `{value}`"
            )));
        }
        task.front.area = area.clone();
    }
    if let Some(verification) = &plan.apply_verification {
        task.front.verification = verification.clone();
    }
    task.front.updated_at = Utc::now();

    // `area`/`verification` keep "set vs untouched" semantics: only send the
    // field when a metadata push was planned, carrying the pushed value or
    // the remote's current one.
    let metadata_update = plan.push_area.is_some() || plan.push_verification.is_some();
    let update = RemoteIssueUpdate {
        title: plan.push_title.clone(),
        body: plan.push_body.clone(),
        // The adapter reconciles its own status markers from `status`;
        // labels ride along whenever a status push needs them.
        labels: if plan.push_labels.is_some() || plan.push_status.is_some() {
            Some(
                plan.push_labels
                    .clone()
                    .unwrap_or_else(|| issue.labels.clone()),
            )
        } else {
            None
        },
        status: plan
            .push_status
            .or_else(|| plan.push_labels.is_some().then_some(task.front.status)),
        area: metadata_update.then(|| plan.push_area.clone().unwrap_or_else(|| issue.area.clone())),
        verification: metadata_update.then(|| {
            plan.push_verification
                .clone()
                .unwrap_or_else(|| issue.verification.clone())
        }),
    };

    let has_push = update.title.is_some()
        || update.body.is_some()
        || update.labels.is_some()
        || update.status.is_some()
        || update.area.is_some()
        || update.verification.is_some();
    if has_push {
        adapter.update_issue(&project, &link.id, &update, issue)?;
    }

    let removals: Vec<_> = relationship
        .changes
        .iter()
        .filter(|change| change.action == crate::remote::model::RelationAction::Remove)
        .cloned()
        .collect();
    if plan.direction != SyncDirection::Pull
        && adapter.supports_relationships()
        && (!relationship.supported.is_empty() || !removals.is_empty())
    {
        adapter.update_relationships(&project, &link.id, &relationship.supported, &removals)?;
    }

    if !advance_baseline {
        return Ok(None);
    }

    let refreshed = adapter.fetch_issue(&project, &link.id)?;
    set_baseline(&mut task, &refreshed);
    let outcome = persist_task(board, &task, commit_message)?;
    Ok(crate::mutation::commit_warning(outcome))
}

/// Whether `project` may receive Remote Issues on this Board: configured
/// through `remote.projects`/legacy sections, a member Repository's
/// designated project, or implied by the adapter (Workspace members).
pub fn project_available<R: RemoteResolver + ?Sized>(
    board: &Board,
    resolver: &R,
    project: &RemoteProject,
) -> Result<bool> {
    if crate::remote::configured_projects(board)
        .iter()
        .any(|candidate| {
            candidate.tracker == project.tracker
                && candidate.project.eq_ignore_ascii_case(&project.project)
        })
    {
        return Ok(true);
    }
    let adapter = resolver.adapter(&project.tracker)?;
    adapter.project_available(board, &project.project)
}

pub fn require_project_available<R: RemoteResolver + ?Sized>(
    board: &Board,
    resolver: &R,
    project: &RemoteProject,
) -> Result<()> {
    if project_available(board, resolver, project)? {
        return Ok(());
    }
    let message = format!(
        "Remote Project `{}` on tracker `{}` is not available to this Board",
        project.project, project.tracker
    );
    Err(OrbitError::new(code::REMOTE_PROJECT_NOT_LINKED, message).with_details(json!({
        "tracker": project.tracker,
        "project": project.project,
        "remediation": format!("configure the Remote Project on the Board, e.g. `orbit remote project link {} {}`", project.tracker, project.project),
    })))
}

/// Write the sync baseline onto the Task's Remote link, materializing a
/// legacy `github:` link first.
pub fn set_baseline(task: &mut Task, issue: &RemoteIssue) {
    if link_view(task).is_none() {
        return;
    }
    let baseline = sync::baseline_from(task, issue);
    if let Some(link) = link_mut(task) {
        link.baseline = encode_baseline(&baseline);
    }
}

/// A follow-up step that failed after the Remote Issue already existed.
pub enum CreateFollowUp {
    /// The link and baseline were not written; the Issue exists but is unlinked.
    LinkNotPersisted(OrbitError),
    /// The Task is terminal but the Remote Issue could not be closed.
    CloseFailed(OrbitError),
    /// The Remote Issue cache was not updated.
    CatalogUpdateFailed(OrbitError),
}

pub struct CreatedIssue {
    /// The Task with its link (and baseline, when persisted).
    pub task: Task,
    /// The Remote Issue as last known: closed-status when the Task is
    /// terminal and the close succeeded.
    pub issue: RemoteIssue,
    /// The Remote Project the Issue was created in.
    pub project: RemoteProject,
    pub failures: Vec<CreateFollowUp>,
    /// Commit warnings from the writes that did persist.
    pub warnings: Vec<String>,
}

/// Creates the Remote Issue for an unlinked Task, links it, and closes it
/// when the Task is terminal. `Err` means nothing was created; later
/// failures are in `failures`.
pub fn create_and_link<R: RemoteResolver + ?Sized>(
    board: &Board,
    resolver: &R,
    project: &RemoteProject,
    task: &Task,
    commit_message: &str,
) -> Result<CreatedIssue> {
    require_project_available(board, resolver, project)?;
    let adapter = resolver.adapter(&project.tracker)?;
    sync::reject_unmapped_metadata(adapter.as_ref(), project, task)?;
    let mut issue = adapter.create_issue(
        project,
        RemoteIssueFields {
            title: task.front.title.clone(),
            body: task.body.clone(),
            labels: task.front.labels.clone(),
            status: task.front.status,
            area: task.front.area.clone(),
            verification: task.front.verification.clone(),
        },
    )?;

    let mut task = task.clone();
    let mut failures = Vec::new();
    let mut warnings = Vec::new();
    materialize(&mut task);
    task.front.remote = Some(crate::task::RemoteLink {
        tracker: project.tracker.clone(),
        project: project.project.clone(),
        id: issue.id.clone(),
        url: issue.url.clone(),
        active: true,
        baseline: None,
    });
    task.front.updated_at = Utc::now();
    set_baseline(&mut task, &issue);
    match persist_task(board, &task, commit_message) {
        Ok(outcome) => warnings.extend(crate::mutation::commit_warning(outcome)),
        Err(error) => failures.push(CreateFollowUp::LinkNotPersisted(error)),
    }
    let linked = failures.is_empty();

    let remember = |issue: &RemoteIssue, failures: &mut Vec<CreateFollowUp>| {
        if let Err(error) = crate::remote::catalog::remember_created_issue(board, project, issue)
            && !failures
                .iter()
                .any(|failure| matches!(failure, CreateFollowUp::CatalogUpdateFailed(_)))
        {
            failures.push(CreateFollowUp::CatalogUpdateFailed(error));
        }
    };
    remember(&issue, &mut failures);

    if task.front.status.is_terminal() {
        let close = adapter.update_issue(
            project,
            &issue.id,
            &RemoteIssueUpdate {
                status: Some(task.front.status),
                ..RemoteIssueUpdate::default()
            },
            &issue,
        );
        match close {
            Ok(closed) => {
                issue = closed;
                if linked {
                    set_baseline(&mut task, &issue);
                    match persist_task(board, &task, commit_message) {
                        Ok(outcome) => warnings.extend(crate::mutation::commit_warning(outcome)),
                        Err(error) => warnings.push(format!(
                            "task {}: closed {}#{} but could not record the sync baseline: {}; run `orbit remote sync {} --apply`",
                            task.id, project.project, issue.id, error.message, task.id
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
        project: project.clone(),
        failures,
        warnings,
    })
}

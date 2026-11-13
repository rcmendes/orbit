//! Opt-in push of mapped Task changes to Remote Issues after a command.
//! The caller holds the Board lock for the complete operation.
use std::collections::BTreeMap;
use std::sync::atomic::{AtomicBool, Ordering};

use crate::board::Board;
use crate::error::Result;
use crate::graph::{Graph, LoadMode};
use crate::remote::execution::{self, ApplyPlanRequest};
use crate::remote::link::{active_link, link_view};
use crate::remote::model::SyncDirection;
use crate::remote::port::{BoardResolver, RemoteAdapter, RemoteResolver};
use crate::remote::relationships::RelationshipReport;
use crate::remote::sync::{self, OrbitSyncView};
use crate::task::Task;

static SUPPRESSED: AtomicBool = AtomicBool::new(false);

/// Suppress auto-push for the rest of this process (`orbit --no-sync`).
pub fn suppress() {
    SUPPRESSED.store(true, Ordering::Relaxed);
}

pub fn suppressed() -> bool {
    if SUPPRESSED.load(Ordering::Relaxed) {
        return true;
    }
    std::env::var("ORBIT_NO_AUTO_SYNC")
        .ok()
        .is_some_and(|value| {
            matches!(
                value.trim().to_ascii_lowercase().as_str(),
                "1" | "true" | "yes" | "on"
            )
        })
}

/// Whether the Board opted into Auto-push. The provider-neutral flag lives
/// in `[remote]`; the legacy `[github] auto_push` still counts until
/// migration rewrites it.
pub fn enabled(board: &Board) -> bool {
    board.config.remote.auto_push || board.config.github.auto_push
}

pub type TaskSnapshot = BTreeMap<String, String>;

pub fn snapshot(board: &Board) -> TaskSnapshot {
    let mut snapshot = TaskSnapshot::new();
    let Ok(entries) = std::fs::read_dir(board.tasks_dir()) else {
        return snapshot;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().and_then(|extension| extension.to_str()) != Some("md") {
            continue;
        }
        let Some(id) = path.file_stem().and_then(|stem| stem.to_str()) else {
            continue;
        };
        if let Ok(content) = std::fs::read_to_string(&path) {
            snapshot.insert(id.to_string(), content);
        }
    }
    snapshot
}

/// Push every Task whose mapped fields changed during the command.
pub fn after_command<R: RemoteResolver + ?Sized>(
    board: &Board,
    resolver: &R,
    before: &TaskSnapshot,
) -> Vec<String> {
    if !enabled(board) || suppressed() {
        return Vec::new();
    }
    let after = snapshot(board);
    let changed: Vec<String> = after
        .iter()
        .filter(|(id, content)| before.get(*id) != Some(*content))
        .map(|(id, _)| id.clone())
        .collect();
    let deleted: Vec<String> = before
        .keys()
        .filter(|id| !after.contains_key(*id))
        .cloned()
        .collect();
    if changed.is_empty() && deleted.is_empty() {
        return Vec::new();
    }

    let graph = match Graph::load(board, LoadMode::Strict) {
        Ok((graph, _)) => graph,
        Err(error) => {
            return vec![format!(
                "REMOTE_AUTO_PUSH_FAILED: could not load the Board after the command: {}",
                error.message
            )];
        }
    };

    let mut warnings = Vec::new();
    for id in deleted {
        if before
            .get(&id)
            .is_some_and(|content| content.contains("\nremote:") || content.contains("\ngithub:"))
        {
            warnings.push(format!(
                "task {id} was deleted locally; its Remote Issue was left unchanged"
            ));
        }
    }

    for id in changed {
        let Some(task) = graph.tasks.get(&id) else {
            continue;
        };
        let new_task = !before.contains_key(&id);
        if active_link(task).is_none() {
            if new_task {
                warnings.extend(auto_create(board, resolver, task));
            }
            continue;
        }
        warnings.extend(push_task(board, resolver, task));
    }
    warnings
}

/// Convenience for callers that use the production resolver.
pub fn after_command_default(board: &Board, before: &TaskSnapshot) -> Vec<String> {
    after_command(board, &BoardResolver::new(board), before)
}

/// Candidate Remote Projects a new Task on this Board may publish to:
/// every project connected to the Board (`remote::connected_projects`).
fn create_projects(board: &Board) -> Result<Vec<crate::remote::RemoteProject>> {
    crate::remote::connected_projects(board)
}

fn auto_create<R: RemoteResolver + ?Sized>(
    board: &Board,
    resolver: &R,
    task: &Task,
) -> Vec<String> {
    if link_view(task).is_some() {
        return Vec::new();
    }
    // A Repository may designate the Remote Project for its new Tasks.
    if let Some(project) = task
        .front
        .repo
        .as_deref()
        .and_then(|repo| crate::repository::remote_project_for(board, repo))
    {
        return create_in(board, resolver, task, &project);
    }
    let projects = match create_projects(board) {
        Ok(projects) => projects,
        Err(error) => {
            return vec![format!(
                "REMOTE_AUTO_PUSH_FAILED: task {}: could not resolve Remote Projects: {}",
                task.id, error.message
            )];
        }
    };
    if projects.len() != 1 {
        return vec![format!(
            "task {} stays local: auto-push needs exactly one Remote Project to create a Remote Issue",
            task.id
        )];
    }
    create_in(board, resolver, task, &projects[0])
}

fn create_in<R: RemoteResolver + ?Sized>(
    board: &Board,
    resolver: &R,
    task: &Task,
    project: &crate::remote::RemoteProject,
) -> Vec<String> {
    let created = match execution::create_and_link(
        board,
        resolver,
        project,
        task,
        &format!("orbit: remote auto-create {}", task.id),
    ) {
        Ok(created) => created,
        Err(error) => {
            return vec![format!(
                "REMOTE_AUTO_PUSH_FAILED: task {}: {}",
                task.id, error.message
            )];
        }
    };
    let identity = format!("{}#{}", project.project, created.issue.id);
    let mut warnings = created.warnings;
    for failure in created.failures {
        warnings.push(match failure {
            execution::CreateFollowUp::LinkNotPersisted(error) => format!(
                "REMOTE_AUTO_PUSH_FAILED: task {}: created {identity} but could not persist the link: {}",
                task.id, error.message
            ),
            execution::CreateFollowUp::CloseFailed(error) => format!(
                "REMOTE_AUTO_PUSH_FAILED: task {}: created {identity} but could not close it: {}",
                task.id, error.message
            ),
            execution::CreateFollowUp::CatalogUpdateFailed(error) => format!(
                "task {}: created {identity} but could not update the Remote Issue cache: {}; run `orbit remote issues refresh`",
                task.id, error.message
            ),
        });
    }
    warnings
}

fn push_task<R: RemoteResolver + ?Sized>(board: &Board, resolver: &R, task: &Task) -> Vec<String> {
    let Some(link) = active_link(task) else {
        return Vec::new();
    };
    let project = link.remote_project();
    let adapter = match resolver.adapter(&link.tracker) {
        Ok(adapter) => adapter,
        Err(error) => {
            return vec![format!(
                "REMOTE_AUTO_PUSH_FAILED: task {}: {}",
                task.id, error.message
            )];
        }
    };
    let issue = match adapter.fetch_issue(&project, &link.id) {
        Ok(issue) => issue,
        Err(error) => {
            return vec![format!(
                "REMOTE_AUTO_PUSH_FAILED: task {}: could not read {}: {}",
                task.id,
                link.identity(),
                error.message
            )];
        }
    };
    let mut plan = sync::plan_sync(
        adapter.as_ref(),
        OrbitSyncView { task, link: &link },
        &issue,
        SyncDirection::Both,
    );

    let conflicts: Vec<String> = plan
        .conflicts
        .iter()
        .map(|conflict| conflict.field.clone())
        .collect();
    let remote_pending = !plan.pull_changes().is_empty();

    // Auto-push never pulls. Keep only local changes.
    plan.apply_title = None;
    plan.apply_body = None;
    plan.apply_status = None;
    plan.apply_labels = None;
    plan.apply_area = None;
    plan.apply_verification = None;
    plan.direction = SyncDirection::Push;

    let mut warnings = Vec::new();
    if !conflicts.is_empty() {
        warnings.push(format!(
            "REMOTE_AUTO_PUSH_CONFLICT: task {}: both sides changed {}; run `orbit remote sync {}` to resolve",
            task.id,
            conflicts.join(", "),
            task.id
        ));
    }
    if remote_pending {
        warnings.push(format!(
            "REMOTE_AUTO_PUSH_PENDING: task {}: the remote tracker has changes; run `orbit remote sync {}` to review them",
            task.id, task.id
        ));
    }
    if plan.is_noop() {
        return warnings;
    }

    let relationship = RelationshipReport {
        supported: Vec::new(),
        unsupported: Vec::new(),
        changes: Vec::new(),
    };
    let advance_baseline = conflicts.is_empty() && !remote_pending;
    match execution::apply_plan(
        board,
        adapter.as_ref(),
        ApplyPlanRequest {
            task,
            link: &link,
            issue: &issue,
            plan: &plan,
            relationship: &relationship,
            advance_baseline,
            commit_message: &format!("orbit: remote auto-push {}", task.id),
        },
    ) {
        Ok(Some(warning)) => warnings.push(warning),
        Ok(None) => {}
        Err(error) => warnings.push(format!(
            "REMOTE_AUTO_PUSH_FAILED: task {}: {}: {}",
            task.id, error.code, error.message
        )),
    }
    warnings
}

/// Fetch a Remote Issue through the resolver for catalog/inspect callers.
pub fn fetch_issue<R: RemoteResolver + ?Sized>(
    _board: &Board,
    resolver: &R,
    project: &crate::remote::RemoteProject,
    id: &str,
) -> Result<crate::remote::model::RemoteIssue> {
    let adapter = resolver.adapter(&project.tracker)?;
    adapter.fetch_issue(project, id)
}

/// Close a Remote Issue when its Task reaches a terminal status.
pub fn close_issue<R: RemoteResolver + ?Sized>(
    _board: &Board,
    resolver: &R,
    project: &crate::remote::RemoteProject,
    id: &str,
    status: crate::task::Status,
) -> Result<crate::remote::model::RemoteIssue> {
    let adapter: Box<dyn RemoteAdapter> = resolver.adapter(&project.tracker)?;
    let current = adapter.fetch_issue(project, id)?;
    adapter.update_issue(
        project,
        id,
        &crate::remote::model::RemoteIssueUpdate {
            status: Some(status),
            ..Default::default()
        },
        &current,
    )
}

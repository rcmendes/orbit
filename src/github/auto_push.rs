//! Opt-in push of mapped Task changes to linked GitHub Issues after a command.
//! The caller holds the Board lock for the complete operation.
use std::collections::BTreeMap;
use std::sync::atomic::{AtomicBool, Ordering};

use chrono::Utc;

use crate::board::Board;
use crate::github::client::{CreateIssueInput, GitHubClient, client_from_env};
use crate::github::execution;
use crate::github::link::GitHubLink;
use crate::github::model::SyncDirection;
use crate::github::status::labels_for_status;
use crate::github::{relationships, sync};
use crate::graph::{Graph, LoadMode};
use crate::mutation;
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
pub fn after_command(board: &Board, before: &TaskSnapshot) -> Vec<String> {
    if !board.config.github.auto_push || suppressed() {
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
                "GITHUB_AUTO_PUSH_FAILED: could not load the Board after the command: {}",
                error.message
            )];
        }
    };
    let client = match client_from_env() {
        Ok(client) => client,
        Err(error) => {
            return vec![format!(
                "GITHUB_AUTO_PUSH_FAILED: could not initialize the GitHub client: {}",
                error.message
            )];
        }
    };

    let mut warnings = Vec::new();
    for id in deleted {
        if before
            .get(&id)
            .is_some_and(|content| content.contains("\ngithub:"))
        {
            warnings.push(format!(
                "task {id} was deleted locally; its GitHub Issue was left unchanged"
            ));
        }
    }

    for id in changed {
        let Some(task) = graph.tasks.get(&id) else {
            continue;
        };
        let new_task = !before.contains_key(&id);
        if task.github_link().is_none() {
            if new_task {
                warnings.extend(auto_create(board, client.as_ref(), task));
            }
            continue;
        }
        warnings.extend(push_task(board, client.as_ref(), task));
    }
    warnings
}

fn auto_create(board: &Board, client: &dyn GitHubClient, task: &Task) -> Vec<String> {
    if task.front.github.is_some() {
        return Vec::new();
    }
    let repositories = match crate::workspace::export_repositories(board) {
        Ok(Some(repositories)) => repositories,
        Ok(None) => board.config.github.repositories.clone(),
        Err(error) => {
            return vec![format!(
                "GITHUB_AUTO_PUSH_FAILED: task {}: could not resolve repositories: {}",
                task.id, error.message
            )];
        }
    };
    if repositories.len() != 1 {
        return vec![format!(
            "task {} stays local: auto-push needs exactly one linked or included GitHub repository to create an Issue",
            task.id
        )];
    }
    let repo = match crate::github::GitHubConfig::normalize_repo(&repositories[0]) {
        Ok(repo) => repo,
        Err(error) => {
            return vec![format!(
                "GITHUB_AUTO_PUSH_FAILED: task {}: invalid repository `{}`: {}",
                task.id, repositories[0], error.message
            )];
        }
    };
    if let Err(error) = execution::require_repository_available(board, &repo) {
        return vec![format!(
            "GITHUB_AUTO_PUSH_FAILED: task {}: {}",
            task.id, error.message
        )];
    }
    if board.config.github.mode.is_basic()
        && let Err(error) = execution::reject_basic_metadata(task)
    {
        return vec![format!(
            "GITHUB_AUTO_PUSH_FAILED: task {}: {}",
            task.id, error.message
        )];
    }

    let issue = match client.create_issue(
        &repo,
        &CreateIssueInput {
            title: task.front.title.clone(),
            body: task.body.clone(),
            labels: labels_for_status(&task.front.labels, task.front.status),
            area: task.front.area.map(|area| area.as_str().to_string()),
            verification: task.front.verification.clone(),
            organization: board.config.github.organization.clone(),
            issue_fields: board.config.github.issue_fields.clone(),
        },
    ) {
        Ok(issue) => issue,
        Err(error) => {
            return vec![format!(
                "GITHUB_AUTO_PUSH_FAILED: task {}: could not create an Issue: {}",
                task.id, error.message
            )];
        }
    };

    let mut task = task.clone();
    task.front.github = Some(GitHubLink::new(repo.clone(), issue.number));
    task.front.updated_at = Utc::now();
    execution::set_baseline(&mut task, &issue);
    let mut warnings = Vec::new();
    if let Err(error) = mutation::persist_task(
        board,
        &task,
        &format!("orbit: github auto-create {}", task.id),
    ) {
        warnings.push(format!(
            "GITHUB_AUTO_PUSH_FAILED: task {}: created {repo}#{} but could not persist the link: {}",
            task.id, issue.number, error.message
        ));
        return warnings;
    }
    if let Err(error) = crate::issue_catalog::remember_created_issue(board, &repo, &issue) {
        warnings.push(format!(
            "task {}: created {repo}#{} but could not update the Remote Issue cache: {}; run `orbit workspace issues refresh`",
            task.id, issue.number, error.message
        ));
    }
    warnings
}

fn push_task(board: &Board, client: &dyn GitHubClient, task: &Task) -> Vec<String> {
    let Some(link) = task.github_link().cloned() else {
        return Vec::new();
    };
    let issue = match execution::fetch_issue(board, client, &link.repo, link.number) {
        Ok(issue) => issue,
        Err(error) => {
            return vec![format!(
                "GITHUB_AUTO_PUSH_FAILED: task {}: could not read {}/#{}: {}",
                task.id, link.repo, link.number, error.message
            )];
        }
    };
    let mut plan = sync::plan_sync(
        sync::OrbitSyncView { task, link: &link },
        &issue,
        SyncDirection::Both,
    );
    let basic_metadata_warning = if board.config.github.mode.is_basic()
        && (task.front.area.is_some() || task.front.verification.is_some())
    {
        plan.push_area = None;
        plan.push_verification = None;
        plan.apply_area = None;
        plan.apply_verification = None;
        Some(format!(
            "GITHUB_AUTO_PUSH_PENDING: task {}: basic mode cannot push area or verification; those values stay local",
            task.id
        ))
    } else {
        None
    };

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
    if let Some(warning) = basic_metadata_warning {
        warnings.push(warning);
    }
    if !conflicts.is_empty() {
        warnings.push(format!(
            "GITHUB_AUTO_PUSH_CONFLICT: task {}: both sides changed {}; run `orbit github sync {}` to resolve",
            task.id,
            conflicts.join(", "),
            task.id
        ));
    }
    if remote_pending {
        warnings.push(format!(
            "GITHUB_AUTO_PUSH_PENDING: task {}: GitHub has remote changes; run `orbit github sync {}` to review them",
            task.id, task.id
        ));
    }
    if plan.is_noop() {
        return warnings;
    }

    let relationship = relationships::RelationshipReport {
        supported: Vec::new(),
        unsupported: Vec::new(),
        changes: Vec::new(),
    };
    let advance_baseline = conflicts.is_empty() && !remote_pending;
    match execution::apply_plan(
        board,
        client,
        execution::ApplyPlanRequest {
            task,
            link: &link,
            issue: &issue,
            plan: &plan,
            relationship: &relationship,
            advance_baseline,
            commit_message: &format!("orbit: github auto-push {}", task.id),
        },
    ) {
        Ok(Some(warning)) => warnings.push(warning),
        Ok(None) => {}
        Err(error) => warnings.push(format!(
            "GITHUB_AUTO_PUSH_FAILED: task {}: {}: {}",
            task.id, error.code, error.message
        )),
    }
    warnings
}

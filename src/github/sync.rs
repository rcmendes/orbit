use chrono::Utc;

use crate::github::fields::validate_verification;
use crate::github::link::{GitHubLink, GitHubSyncBaseline};
use crate::github::model::{IssueSnapshot, SyncConflict, SyncDirection, SyncPlan};
use crate::github::status::{issue_state_from_status, status_from_issue};
use crate::task::{Status, Task};

pub struct OrbitSyncView<'a> {
    pub task: &'a Task,
    pub link: &'a GitHubLink,
}

pub fn plan_sync(
    view: OrbitSyncView<'_>,
    issue: &IssueSnapshot,
    direction: SyncDirection,
) -> SyncPlan {
    let mut plan = SyncPlan {
        direction,
        apply_title: None,
        apply_body: None,
        apply_status: None,
        apply_labels: None,
        apply_area: None,
        apply_verification: None,
        push_title: None,
        push_body: None,
        push_labels: None,
        push_area: None,
        push_verification: None,
        push_state: None,
        conflicts: Vec::new(),
        warnings: Vec::new(),
    };

    if !issue.assignees.is_empty() {
        plan.warnings.push(
            "GitHub assignees are informational only and are never mapped to Orbit claims".into(),
        );
    }

    let baseline = view.link.last_sync.as_ref();

    let orbit_title = view.task.front.title.clone();
    let orbit_body = view.task.body.clone();
    let orbit_labels = view.task.front.labels.clone();
    let orbit_area = view.task.front.area.map(|area| area.as_str().to_string());
    let orbit_verification = view.task.front.verification.clone();
    let orbit_status = view.task.front.status;

    plan_scalar(
        &mut plan,
        "title",
        baseline.map(|base| base.title.clone()),
        orbit_title.clone(),
        issue.title.clone(),
        direction,
    );
    plan_scalar(
        &mut plan,
        "body",
        baseline.map(|base| base.body.clone()),
        orbit_body.clone(),
        issue.body.clone(),
        direction,
    );
    plan_scalar(
        &mut plan,
        "labels",
        baseline.map(|base| base.labels.clone()),
        orbit_labels.clone(),
        issue.labels.clone(),
        direction,
    );
    plan_optional_scalar(
        &mut plan,
        "area",
        baseline.map(|base| base.area.clone()),
        orbit_area.clone(),
        issue.area.clone(),
        direction,
    );
    plan_optional_scalar(
        &mut plan,
        "verification",
        baseline.map(|base| base.verification.clone()),
        orbit_verification.clone(),
        issue.verification.clone(),
        direction,
    );

    plan_status(&mut plan, baseline, orbit_status, issue, direction);

    plan
}

pub fn baseline_from(view: OrbitSyncView<'_>, issue: &IssueSnapshot) -> GitHubSyncBaseline {
    GitHubSyncBaseline {
        at: Utc::now(),
        issue_updated_at: issue.updated_at,
        orbit_updated_at: view.task.front.updated_at,
        title: issue.title.clone(),
        body: issue.body.clone(),
        labels: issue.labels.clone(),
        area: view.task.front.area.map(|area| area.as_str().to_string()),
        verification: view.task.front.verification.clone(),
        status: Some(view.task.front.status),
        issue_state: Some(issue.state),
    }
}

fn plan_scalar<T>(
    plan: &mut SyncPlan,
    field: &str,
    baseline: Option<T>,
    orbit_value: T,
    github_value: T,
    direction: SyncDirection,
) where
    T: Clone + PartialEq + serde::Serialize,
{
    if github_value == orbit_value {
        return;
    }
    if let Some(base) = baseline {
        let orbit_changed = orbit_value != base;
        let github_changed = github_value != base;
        if orbit_changed && github_changed {
            add_conflict(plan, field, &orbit_value, &github_value);
            return;
        }
        match direction {
            SyncDirection::Both if github_changed => set_pull(plan, field, &github_value),
            SyncDirection::Both | SyncDirection::Push => set_push(plan, field, &orbit_value),
            SyncDirection::Pull => set_pull(plan, field, &github_value),
        }
    } else {
        match direction {
            SyncDirection::Pull => set_pull(plan, field, &github_value),
            SyncDirection::Both | SyncDirection::Push => set_push(plan, field, &orbit_value),
        }
    }
}

fn plan_optional_scalar(
    plan: &mut SyncPlan,
    field: &str,
    baseline: Option<Option<String>>,
    orbit_value: Option<String>,
    github_value: Option<String>,
    direction: SyncDirection,
) {
    plan_scalar(plan, field, baseline, orbit_value, github_value, direction);
}

fn set_pull<T: serde::Serialize>(plan: &mut SyncPlan, field: &str, value: &T) {
    match field {
        "title" => {
            plan.apply_title = serde_json::from_value(serde_json::to_value(value).unwrap()).ok()
        }
        "body" => {
            plan.apply_body = serde_json::from_value(serde_json::to_value(value).unwrap()).ok()
        }
        "labels" => {
            plan.apply_labels = serde_json::from_value(serde_json::to_value(value).unwrap()).ok()
        }
        "area" => {
            plan.apply_area =
                Some(serde_json::from_value(serde_json::to_value(value).unwrap()).unwrap_or(None))
        }
        "verification" => {
            plan.apply_verification =
                Some(serde_json::from_value(serde_json::to_value(value).unwrap()).unwrap_or(None))
        }
        _ => {}
    }
}

fn set_push<T: serde::Serialize>(plan: &mut SyncPlan, field: &str, value: &T) {
    match field {
        "title" => {
            plan.push_title = serde_json::from_value(serde_json::to_value(value).unwrap()).ok()
        }
        "body" => {
            plan.push_body = serde_json::from_value(serde_json::to_value(value).unwrap()).ok()
        }
        "labels" => {
            plan.push_labels = serde_json::from_value(serde_json::to_value(value).unwrap()).ok()
        }
        "area" => {
            plan.push_area =
                Some(serde_json::from_value(serde_json::to_value(value).unwrap()).unwrap_or(None))
        }
        "verification" => {
            plan.push_verification =
                Some(serde_json::from_value(serde_json::to_value(value).unwrap()).unwrap_or(None))
        }
        _ => {}
    }
}

fn add_conflict<T: serde::Serialize>(plan: &mut SyncPlan, field: &str, orbit: &T, github: &T) {
    plan.conflicts.push(SyncConflict {
        field: field.to_string(),
        orbit_value: serde_json::to_value(orbit).unwrap_or(serde_json::Value::Null),
        github_value: serde_json::to_value(github).unwrap_or(serde_json::Value::Null),
        message: format!("both Orbit and GitHub changed `{field}` since the last sync"),
    });
}

fn plan_status(
    plan: &mut SyncPlan,
    baseline: Option<&GitHubSyncBaseline>,
    orbit_status: Status,
    issue: &IssueSnapshot,
    direction: SyncDirection,
) {
    let base_status = baseline.and_then(|base| base.status);
    let base_state = baseline.and_then(|base| base.issue_state);
    let github_status = status_from_issue(issue.state, &issue.labels);
    let github_changed = base_state.is_some_and(|state| state != issue.state);
    let orbit_changed = base_status.is_some_and(|status| status != orbit_status);

    if orbit_status == github_status && issue_state_from_status(orbit_status) == issue.state {
        return;
    }
    if orbit_changed && github_changed {
        add_conflict(plan, "status", &orbit_status, &github_status);
        return;
    }
    if direction == SyncDirection::Pull || github_changed && !orbit_changed {
        let pulled_status = if issue.state == crate::github::model::IssueState::Open
            && base_status == Some(Status::InProgress)
        {
            Status::InProgress
        } else {
            github_status
        };
        plan.apply_status = Some(pulled_status);
    } else if direction == SyncDirection::Push
        || orbit_changed
        || base_status.is_none() && issue_state_from_status(orbit_status) != issue.state
    {
        plan.push_state = Some(issue_state_from_status(orbit_status));
    }
}

pub fn validate_orbit_metadata(task: &Task) -> Result<(), crate::error::OrbitError> {
    if let Some(verification) = task.front.verification.as_deref() {
        validate_verification(verification)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::task::{TaskArea, TaskFrontmatter, TaskType, default_priority};
    use chrono::TimeZone;
    use std::collections::BTreeMap;

    fn sample_task(title: &str, body: &str, status: Status) -> Task {
        let now = Utc.with_ymd_and_hms(2026, 10, 23, 12, 0, 0).unwrap();
        Task {
            id: "orb-aaaaaa".into(),
            front: TaskFrontmatter {
                title: title.into(),
                status,
                task_type: TaskType::Task,
                priority: default_priority(),
                labels: vec!["bug".into()],
                depends_on: Vec::new(),
                parent: None,
                waiting_on: None,
                checkpoint: None,
                claim: None,
                stray_claim_times: None,
                created_at: now,
                updated_at: now,
                area: Some(TaskArea::Cli),
                verification: Some("cargo test".into()),
                github: Some(GitHubLink {
                    repo: "acme/app".into(),
                    number: 9,
                    active: true,
                    last_sync: Some(GitHubSyncBaseline {
                        at: now,
                        issue_updated_at: now,
                        orbit_updated_at: now,
                        title: "Old".into(),
                        body: "Old body".into(),
                        labels: vec!["bug".into()],
                        area: Some("cli".into()),
                        verification: Some("cargo test".into()),
                        status: Some(Status::Todo),
                        issue_state: Some(crate::github::model::IssueState::Open),
                    }),
                }),
                extra: BTreeMap::new(),
            },
            body: body.into(),
        }
    }

    fn sample_issue(title: &str, body: &str) -> IssueSnapshot {
        IssueSnapshot {
            repo: "acme/app".into(),
            number: 9,
            title: title.into(),
            body: body.into(),
            state: crate::github::model::IssueState::Open,
            labels: vec!["bug".into()],
            assignees: vec!["human".into()],
            updated_at: Utc.with_ymd_and_hms(2026, 10, 23, 13, 0, 0).unwrap(),
            area: Some("cli".into()),
            verification: Some("cargo test".into()),
            parent_issue: vec![],
            blocked_by: vec![],
            blocking: vec![],
        }
    }

    #[test]
    fn detects_title_conflicts_when_both_sides_changed() {
        let mut task = sample_task("Orbit title", "Orbit body", Status::Todo);
        task.front.updated_at = Utc.with_ymd_and_hms(2026, 10, 23, 13, 30, 0).unwrap();
        let issue = sample_issue("GitHub title", "GitHub body");
        let link = task.front.github.as_ref().unwrap();
        let plan = plan_sync(
            OrbitSyncView { task: &task, link },
            &issue,
            SyncDirection::Both,
        );
        assert!(plan.has_conflicts());
        assert!(
            plan.conflicts
                .iter()
                .any(|conflict| conflict.field == "title")
        );
    }

    #[test]
    fn pulls_github_title_when_only_github_changed() {
        let mut task = sample_task("Old", "Old body", Status::Todo);
        task.front.title = "Old".into();
        task.front.updated_at = Utc.with_ymd_and_hms(2026, 10, 23, 12, 0, 0).unwrap();
        let issue = sample_issue("GitHub title", "Old body");
        let link = task.front.github.as_ref().unwrap();
        let plan = plan_sync(
            OrbitSyncView { task: &task, link },
            &issue,
            SyncDirection::Pull,
        );
        assert!(!plan.has_conflicts());
        assert_eq!(plan.apply_title.as_deref(), Some("GitHub title"));
    }
}

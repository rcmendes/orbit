use chrono::Utc;

use crate::github::fields;
use crate::github::link::{GitHubLink, GitHubSyncBaseline};
use crate::github::model::{IssueSnapshot, SyncConflict, SyncDirection, SyncPlan};
use crate::github::status::{self, issue_state_from_status, status_from_issue, WONTFIX_LABEL};
use crate::task::{Task, Status};

pub struct OrbitSyncView<'a> {
    pub task: &'a Task,
    pub link: &'a GitHubLink,
}

pub fn plan_sync(view: OrbitSyncView<'_>, issue: &IssueSnapshot, direction: SyncDirection) -> SyncPlan {
    let mut plan = SyncPlan {
        direction,
        apply_title: None,
        apply_body: None,
        apply_status: None,
        apply_labels: None,
        apply_area: None,
        apply_verification: None,
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
    let orbit_changed = baseline.map(|base| view.task.front.updated_at > base.orbit_updated_at);
    let github_changed = baseline.map(|base| issue.updated_at > base.issue_updated_at);

    let orbit_title = view.task.front.title.clone();
    let orbit_body = view.task.body.clone();
    let orbit_labels = view.task.front.labels.clone();
    let orbit_area = view.task.front.area.clone();
    let orbit_verification = view.task.front.verification.clone();
    let orbit_status = view.task.front.status;

    if matches!(direction, SyncDirection::Both | SyncDirection::Pull) {
        maybe_pull_field(
            &mut plan,
            "title",
            baseline.map(|b| b.title.as_str()),
            orbit_title.as_str(),
            issue.title.as_str(),
            orbit_changed,
            github_changed,
        );
        if plan.conflicts.iter().all(|c| c.field != "title") {
            if issue.title != orbit_title {
                plan.apply_title = Some(issue.title.clone());
            }
        }

        maybe_pull_field(
            &mut plan,
            "body",
            baseline.map(|b| b.body.as_str()),
            orbit_body.as_str(),
            issue.body.as_str(),
            orbit_changed,
            github_changed,
        );
        if plan.conflicts.iter().all(|c| c.field != "body") {
            if issue.body != orbit_body {
                plan.apply_body = Some(issue.body.clone());
            }
        }

        let github_status = status_from_issue(issue.state, &issue.labels);
        if !status::preserves_in_progress(orbit_status) && github_status != orbit_status {
            if orbit_status == Status::InProgress && issue.state == crate::github::model::IssueState::Open
            {
                // Open GitHub issues preserve todo/in_progress; do not downgrade active work.
            } else if github_status != orbit_status {
                plan.apply_status = Some(github_status);
            }
        }

        merge_labels(&mut plan, &orbit_labels, &issue.labels, baseline, orbit_changed, github_changed);

        pull_orbit_owned_field(
            &mut plan,
            "area",
            orbit_area.as_deref(),
            issue.area.as_deref(),
            baseline.and_then(|b| b.area.as_deref()),
            orbit_changed,
            github_changed,
        );
        pull_orbit_owned_field(
            &mut plan,
            "verification",
            orbit_verification.as_deref(),
            issue.verification.as_deref(),
            baseline.and_then(|b| b.verification.as_deref()),
            orbit_changed,
            github_changed,
        );
    }

    if matches!(direction, SyncDirection::Both | SyncDirection::Push) {
        if let Some(labels) = &plan.apply_labels {
            plan.push_labels = Some(labels.clone());
        } else if orbit_labels != issue.labels {
            plan.push_labels = Some(orbit_labels.clone());
        }

        if plan.conflicts.iter().all(|c| c.field != "area")
            && orbit_area != issue.area
        {
            plan.push_area = Some(orbit_area.clone());
        }
        if plan.conflicts.iter().all(|c| c.field != "verification")
            && orbit_verification != issue.verification
        {
            plan.push_verification = Some(orbit_verification.clone());
        }

        let desired_state = issue_state_from_status(orbit_status);
        let needs_wontfix = orbit_status == Status::Cancelled
            && !issue.labels.iter().any(|label| label == WONTFIX_LABEL);
        if needs_wontfix {
            plan.warnings.push(
                "closing as cancelled requires the wontfix label on GitHub; sync will add it when pushing state"
                    .into(),
            );
        }
        if desired_state != issue.state {
            plan.push_state = Some(desired_state);
        }
    }

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
        area: view.task.front.area.clone(),
        verification: view.task.front.verification.clone(),
    }
}

fn maybe_pull_field(
    plan: &mut SyncPlan,
    field: &str,
    baseline: Option<&str>,
    orbit_value: &str,
    github_value: &str,
    orbit_changed: Option<bool>,
    github_changed: Option<bool>,
) {
    if github_value == orbit_value {
        return;
    }
    let Some(base) = baseline else {
        return;
    };
    if orbit_changed == Some(true)
        && github_changed == Some(true)
        && orbit_value != base
        && github_value != base
    {
        plan.conflicts.push(SyncConflict {
            field: field.to_string(),
            orbit_value: serde_json::json!(orbit_value),
            github_value: serde_json::json!(github_value),
            message: format!("both Orbit and GitHub changed `{field}` since the last sync"),
        });
    }
}

fn pull_orbit_owned_field(
    plan: &mut SyncPlan,
    field: &str,
    orbit_value: Option<&str>,
    github_value: Option<&str>,
    baseline: Option<&str>,
    orbit_changed: Option<bool>,
    github_changed: Option<bool>,
) {
    if orbit_value == github_value {
        return;
    }
    if let Some(base) = baseline
        && orbit_changed == Some(true)
        && github_changed == Some(true)
        && orbit_value.unwrap_or("") != base
        && github_value.unwrap_or("") != base
    {
        plan.conflicts.push(SyncConflict {
            field: field.to_string(),
            orbit_value: serde_json::json!(orbit_value),
            github_value: serde_json::json!(github_value),
            message: format!("both Orbit and GitHub changed `{field}` since the last sync"),
        });
        return;
    }
    match field {
        "area" => plan.apply_area = Some(github_value.map(str::to_string)),
        "verification" => plan.apply_verification = Some(github_value.map(str::to_string)),
        _ => {}
    }
}

fn merge_labels(
    plan: &mut SyncPlan,
    orbit_labels: &[String],
    github_labels: &[String],
    baseline: Option<&GitHubSyncBaseline>,
    orbit_changed: Option<bool>,
    github_changed: Option<bool>,
) {
    if orbit_labels == github_labels {
        return;
    }
    if let Some(base) = baseline {
        let orbit_changed_labels = orbit_changed == Some(true) && orbit_labels != &base.labels;
        let github_changed_labels =
            github_changed == Some(true) && github_labels != &base.labels;
        if orbit_changed_labels && github_changed_labels {
            plan.conflicts.push(SyncConflict {
                field: "labels".into(),
                orbit_value: serde_json::json!(orbit_labels),
                github_value: serde_json::json!(github_labels),
                message: "both Orbit and GitHub changed labels since the last sync".into(),
            });
            return;
        }
    }
    let mut merged = orbit_labels.to_vec();
    for label in github_labels {
        if !merged.iter().any(|existing| existing == label) {
            merged.push(label.clone());
        }
    }
    merged.sort();
    merged.dedup();
    if merged != orbit_labels {
        plan.apply_labels = Some(merged);
    }
}

pub fn validate_orbit_metadata(task: &Task) -> Result<(), crate::error::OrbitError> {
    if let Some(area) = task.front.area.as_deref() {
        fields::normalize_area(area)?;
    }
    if let Some(verification) = task.front.verification.as_deref() {
        fields::validate_verification(verification)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;
    use crate::task::{TaskFrontmatter, TaskType, default_priority};
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
                claim: None,
                stray_claim_times: None,
                created_at: now,
                updated_at: now,
                area: Some("backend".into()),
                verification: Some("cargo test".into()),
                github: Some(GitHubLink {
                    repo: "acme/app".into(),
                    number: 9,
                    last_sync: Some(GitHubSyncBaseline {
                        at: now,
                        issue_updated_at: now,
                        orbit_updated_at: now,
                        title: "Old".into(),
                        body: "Old body".into(),
                        labels: vec!["bug".into()],
                        area: Some("backend".into()),
                        verification: Some("cargo test".into()),
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
            area: Some("backend".into()),
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
            OrbitSyncView {
                task: &task,
                link,
            },
            &issue,
            SyncDirection::Both,
        );
        assert!(plan.has_conflicts());
        assert!(plan
            .conflicts
            .iter()
            .any(|conflict| conflict.field == "title"));
    }

    #[test]
    fn pulls_github_title_when_only_github_changed() {
        let mut task = sample_task("Old", "Old body", Status::Todo);
        task.front.title = "Old".into();
        task.front.updated_at = Utc.with_ymd_and_hms(2026, 10, 23, 12, 0, 0).unwrap();
        let issue = sample_issue("GitHub title", "Old body");
        let link = task.front.github.as_ref().unwrap();
        let plan = plan_sync(
            OrbitSyncView {
                task: &task,
                link,
            },
            &issue,
            SyncDirection::Pull,
        );
        assert!(!plan.has_conflicts());
        assert_eq!(plan.apply_title.as_deref(), Some("GitHub title"));
    }
}

//! Three-way sync planning in Orbit terms (ADR 0018).
//!
//! Core owns baselines and conflict detection once; the adapter supplies
//! the fetched [`RemoteIssue`] and its [`MappedFields`]. Only mapped fields
//! are planned — unmapped fields stay local-only.

use chrono::Utc;

use crate::error::{OrbitError, Result};
use crate::remote::RemoteProject;
use crate::remote::link::RemoteLinkView;
use crate::remote::model::{
    MappedFields, RemoteIssue, SyncBaseline, SyncConflict, SyncDirection, SyncPlan,
};
use crate::remote::port::RemoteAdapter;
use crate::task::{self, Status, Task};

pub struct OrbitSyncView<'a> {
    pub task: &'a Task,
    pub link: &'a RemoteLinkView,
}

/// Plan the pull/push for one Linked Task against a fetched Remote Issue.
pub fn plan_sync(
    adapter: &dyn RemoteAdapter,
    view: OrbitSyncView<'_>,
    issue: &RemoteIssue,
    direction: SyncDirection,
) -> SyncPlan {
    let project = RemoteProject {
        tracker: view.link.tracker.clone(),
        project: view.link.project.clone(),
    };
    let mapped = adapter.mapped_fields(&project);
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
        push_status: None,
        conflicts: Vec::new(),
        warnings: Vec::new(),
    };

    if !issue.actors.is_empty() {
        plan.warnings.push(
            "Remote assignees are informational only and are never mapped to Orbit claims".into(),
        );
    }

    let baseline = view.link.baseline.as_ref();

    let orbit_title = view.task.front.title.clone();
    let orbit_body = view.task.body.clone();
    let orbit_labels = view.task.front.labels.clone();
    let orbit_area = view.task.front.area.map(|area| area.as_str().to_string());
    let orbit_verification = view.task.front.verification.clone();
    let orbit_status = view.task.front.status;

    if mapped.title {
        plan_scalar(
            &mut plan,
            "title",
            mapped,
            baseline.map(|base| base.title.clone()),
            orbit_title.clone(),
            issue.title.clone(),
            direction,
        );
    }
    if mapped.body {
        plan_scalar(
            &mut plan,
            "body",
            mapped,
            baseline.map(|base| base.body.clone()),
            orbit_body.clone(),
            issue.body.clone(),
            direction,
        );
    }
    if mapped.labels {
        plan_scalar(
            &mut plan,
            "labels",
            mapped,
            baseline.map(|base| comparable_labels(&base.labels)),
            comparable_labels(&orbit_labels),
            comparable_labels(&issue.labels),
            direction,
        );
    }
    if mapped.area {
        plan_scalar(
            &mut plan,
            "area",
            mapped,
            baseline.map(|base| base.area.clone()),
            orbit_area.clone(),
            issue.area.clone(),
            direction,
        );
    }
    if mapped.verification {
        plan_scalar(
            &mut plan,
            "verification",
            mapped,
            baseline.map(|base| base.verification.clone()),
            orbit_verification.clone(),
            issue.verification.clone(),
            direction,
        );
    }
    if mapped.status {
        plan_status(&mut plan, baseline, orbit_status, issue, direction, adapter);
    }

    plan
}

/// The baseline recorded after a successful sync: remote field values plus
/// the Orbit status on both sides, so the next plan sees who moved.
pub fn baseline_from(task: &Task, issue: &RemoteIssue) -> SyncBaseline {
    SyncBaseline {
        at: Utc::now(),
        remote_updated_at: issue.updated_at,
        orbit_updated_at: task.front.updated_at,
        title: issue.title.clone(),
        body: issue.body.clone(),
        labels: issue.labels.clone(),
        area: task.front.area.map(|area| area.as_str().to_string()),
        verification: task.front.verification.clone(),
        status: Some(task.front.status),
        remote_status: Some(issue.status),
        subtasks: issue.relations.blocking.clone(),
        blocked_by: issue.relations.blocked_by.clone(),
    }
}

/// Label order carries no meaning: sort before comparing so a Task whose
/// labels are merely listed in a different order does not push on every sync.
fn comparable_labels(labels: &[String]) -> Vec<String> {
    let mut labels = labels.to_vec();
    labels.sort();
    labels
}

#[allow(clippy::too_many_arguments)]
fn plan_scalar<T>(
    plan: &mut SyncPlan,
    field: &str,
    mapped: MappedFields,
    baseline: Option<T>,
    orbit_value: T,
    remote_value: T,
    direction: SyncDirection,
) where
    T: Clone + PartialEq + serde::Serialize,
{
    debug_assert!(mapped.maps(field));
    if remote_value == orbit_value {
        return;
    }
    if let Some(base) = baseline {
        let orbit_changed = orbit_value != base;
        let remote_changed = remote_value != base;
        if orbit_changed && remote_changed {
            add_conflict(plan, field, &orbit_value, &remote_value);
            return;
        }
        match direction {
            SyncDirection::Both if remote_changed => set_pull(plan, field, &remote_value),
            SyncDirection::Both | SyncDirection::Push => set_push(plan, field, &orbit_value),
            SyncDirection::Pull => set_pull(plan, field, &remote_value),
        }
    } else {
        match direction {
            SyncDirection::Pull => set_pull(plan, field, &remote_value),
            SyncDirection::Both | SyncDirection::Push => set_push(plan, field, &orbit_value),
        }
    }
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

fn add_conflict<T: serde::Serialize>(plan: &mut SyncPlan, field: &str, orbit: &T, remote: &T) {
    plan.conflicts.push(SyncConflict {
        field: field.to_string(),
        orbit_value: serde_json::to_value(orbit).unwrap_or(serde_json::Value::Null),
        remote_value: serde_json::to_value(remote).unwrap_or(serde_json::Value::Null),
        message: format!("both Orbit and the remote tracker changed `{field}` since the last sync"),
    });
}

/// Status planning in Orbit terms. `issue.status` is the adapter-mapped
/// remote status; the adapter also reports the remote status it expects
/// for an Orbit status (`remote_status_for`) and whether the Remote
/// Issue's status markers agree (`status_markers_agree`). Marker drift
/// never pushes on its own — it rides along with a real status push.
fn plan_status(
    plan: &mut SyncPlan,
    baseline: Option<&SyncBaseline>,
    orbit_status: Status,
    issue: &RemoteIssue,
    direction: SyncDirection,
    adapter: &dyn RemoteAdapter,
) {
    let remote_status = issue.status;
    let orbit_changed = baseline
        .and_then(|base| base.status)
        .is_some_and(|status| status != orbit_status);
    let remote_changed = baseline
        .and_then(|base| base.remote_status)
        .is_some_and(|status| status != remote_status);
    // The remote state does not express the Orbit status when its mapped
    // status differs from what the adapter reports for that Orbit status.
    let state_changed = remote_status != adapter.remote_status_for(orbit_status);
    let markers_changed = !adapter.status_markers_agree(issue, orbit_status);

    if !state_changed && !markers_changed && orbit_status == remote_status {
        return;
    }
    if orbit_changed && remote_changed {
        add_conflict(plan, "status", &orbit_status, &remote_status);
        return;
    }
    if direction == SyncDirection::Pull || remote_changed && !orbit_changed {
        // An open remote issue preserves an in-progress Orbit Task: remote
        // trackers cannot express the claim, so pulling "not started"
        // must not demote it back to todo.
        let pulled_status = if remote_status == Status::Todo
            && baseline.and_then(|base| base.status) == Some(Status::InProgress)
        {
            Status::InProgress
        } else {
            remote_status
        };
        plan.apply_status = Some(pulled_status);
    } else if direction == SyncDirection::Push || orbit_changed || state_changed {
        plan.push_status = Some(orbit_status);
    }
}

/// Reject Orbit metadata the adapter cannot publish before any remote write.
pub fn reject_unmapped_metadata(
    adapter: &dyn RemoteAdapter,
    project: &RemoteProject,
    task: &Task,
) -> Result<()> {
    let mapped = adapter.mapped_fields(project);
    let present: Vec<&str> = [
        ("area", task.front.area.is_some()),
        ("verification", task.front.verification.is_some()),
    ]
    .into_iter()
    .filter(|(name, present)| *present && !mapped.maps(name))
    .map(|(name, _)| name)
    .collect();
    if present.is_empty() {
        return Ok(());
    }
    Err(OrbitError::invalid_argument(format!(
        "tracker `{}` cannot sync task {} field(s): {}; remove them or use a Remote Tracker that maps them",
        project.tracker,
        task.id,
        present.join(", ")
    )))
}

/// Validate Orbit-side metadata before it is planned or pushed.
pub fn validate_orbit_metadata(task: &Task) -> Result<()> {
    if let Some(verification) = task.front.verification.as_deref() {
        task::validate_verification(verification)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::remote::model::{
        DesiredRelation, MappedFields, RelationChange, RemoteIssueFields, RemoteIssueSummary,
        RemoteIssueUpdate, RemoteRelations,
    };
    use crate::task::{TaskArea, TaskFrontmatter, TaskType, default_priority};
    use chrono::TimeZone;
    use std::collections::BTreeMap;

    struct TestAdapter;

    impl RemoteAdapter for TestAdapter {
        fn tracker(&self) -> &str {
            "test"
        }
        fn mapped_fields(&self, _project: &RemoteProject) -> MappedFields {
            MappedFields::ALL
        }
        fn project_available(&self, _board: &crate::board::Board, _project: &str) -> Result<bool> {
            Ok(true)
        }
        fn fetch_issue(&self, _project: &RemoteProject, _id: &str) -> Result<RemoteIssue> {
            unreachable!()
        }
        fn list_issues(&self, _project: &RemoteProject) -> Result<Vec<RemoteIssueSummary>> {
            unreachable!()
        }
        fn create_issue(
            &self,
            _project: &RemoteProject,
            _fields: RemoteIssueFields,
        ) -> Result<RemoteIssue> {
            unreachable!()
        }
        fn update_issue(
            &self,
            _project: &RemoteProject,
            _id: &str,
            _update: &RemoteIssueUpdate,
            _current: &RemoteIssue,
        ) -> Result<RemoteIssue> {
            unreachable!()
        }
        fn update_relationships(
            &self,
            _project: &RemoteProject,
            _id: &str,
            _desired: &[DesiredRelation],
            _removals: &[RelationChange],
        ) -> Result<()> {
            unreachable!()
        }
    }

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
                github: None,
                remote: Some(crate::task::RemoteLink {
                    tracker: "test".into(),
                    project: "acme/app".into(),
                    id: "9".into(),
                    url: None,
                    active: true,
                    baseline: None,
                }),
                repo: None,
                extra: BTreeMap::new(),
            },
            body: body.into(),
        }
    }

    fn sample_issue(title: &str, body: &str) -> RemoteIssue {
        RemoteIssue {
            project: "acme/app".into(),
            id: "9".into(),
            url: None,
            title: title.into(),
            body: body.into(),
            labels: vec!["bug".into()],
            markers: Vec::new(),
            area: Some("cli".into()),
            verification: Some("cargo test".into()),
            status: Status::Todo,
            updated_at: Utc.with_ymd_and_hms(2026, 10, 23, 13, 0, 0).unwrap(),
            actors: vec!["human".into()],
            relations: RemoteRelations::default(),
        }
    }

    fn view<'a>(task: &'a Task, link: &'a RemoteLinkView) -> OrbitSyncView<'a> {
        OrbitSyncView { task, link }
    }

    #[test]
    fn detects_title_conflicts_when_both_sides_changed() {
        let mut task = sample_task("Orbit title", "Orbit body", Status::Todo);
        task.front.updated_at = Utc.with_ymd_and_hms(2026, 10, 23, 13, 30, 0).unwrap();
        let issue = sample_issue("Remote title", "Remote body");
        let mut linked = task.clone();
        linked.front.remote.as_mut().unwrap().baseline = Some(
            crate::remote::link::encode_baseline(&SyncBaseline {
                at: Utc.with_ymd_and_hms(2026, 10, 23, 12, 30, 0).unwrap(),
                remote_updated_at: Utc.with_ymd_and_hms(2026, 10, 23, 12, 30, 0).unwrap(),
                orbit_updated_at: Utc.with_ymd_and_hms(2026, 10, 23, 12, 0, 0).unwrap(),
                title: "Old".into(),
                body: "Old body".into(),
                labels: vec!["bug".into()],
                area: Some("cli".into()),
                verification: Some("cargo test".into()),
                status: Some(Status::Todo),
                remote_status: Some(Status::Todo),
                subtasks: Vec::new(),
                blocked_by: Vec::new(),
            })
            .unwrap(),
        );
        let link = crate::remote::link::link_view(&linked).unwrap();
        let plan = plan_sync(
            &TestAdapter,
            view(&linked, &link),
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
    fn pushes_status_when_remote_state_diverged_from_orbit() {
        let mut task = sample_task("Catalog issue", "body", Status::Todo);
        let updated_at = task.front.updated_at;
        let link = task.front.remote.as_mut().unwrap();
        link.baseline = crate::remote::link::encode_baseline(&SyncBaseline {
            at: Utc::now(),
            remote_updated_at: Utc::now(),
            orbit_updated_at: updated_at,
            title: "Catalog issue".into(),
            body: "body".into(),
            labels: vec!["bug".into()],
            area: Some("cli".into()),
            verification: Some("cargo test".into()),
            status: Some(Status::Todo),
            remote_status: Some(Status::Done),
            subtasks: Vec::new(),
            blocked_by: Vec::new(),
        });
        let mut issue = sample_issue("Catalog issue", "body");
        issue.status = Status::Done;
        let link = crate::remote::link::link_view(&task).unwrap();

        let plan = plan_sync(
            &TestAdapter,
            view(&task, &link),
            &issue,
            SyncDirection::Both,
        );

        assert!(!plan.has_conflicts(), "{:?}", plan.conflicts);
        assert_eq!(plan.push_status, Some(Status::Todo));
    }

    #[test]
    fn pulls_remote_status_change_even_when_state_marker_is_unchanged() {
        let mut task = sample_task("Closed issue", "body", Status::Done);
        let updated_at = task.front.updated_at;
        let link = task.front.remote.as_mut().unwrap();
        link.baseline = crate::remote::link::encode_baseline(&SyncBaseline {
            at: Utc::now(),
            remote_updated_at: Utc::now(),
            orbit_updated_at: updated_at,
            title: "Closed issue".into(),
            body: "body".into(),
            labels: vec!["bug".into(), "wontfix".into()],
            area: Some("cli".into()),
            verification: Some("cargo test".into()),
            status: Some(Status::Done),
            remote_status: Some(Status::Done),
            subtasks: Vec::new(),
            blocked_by: Vec::new(),
        });
        let mut issue = sample_issue("Closed issue", "body");
        issue.status = Status::Cancelled;
        issue.labels = vec!["bug".into(), "wontfix".into()];
        let link = crate::remote::link::link_view(&task).unwrap();

        let plan = plan_sync(
            &TestAdapter,
            view(&task, &link),
            &issue,
            SyncDirection::Both,
        );

        assert_eq!(plan.apply_status, Some(Status::Cancelled));
    }

    #[test]
    fn pulls_remote_title_when_only_remote_changed() {
        let mut task = sample_task("Old", "Old body", Status::Todo);
        task.front.title = "Old".into();
        task.front.updated_at = Utc.with_ymd_and_hms(2026, 10, 23, 12, 0, 0).unwrap();
        let issue = sample_issue("Remote title", "Old body");
        let link = crate::remote::link::link_view(&task).unwrap();
        let plan = plan_sync(
            &TestAdapter,
            view(&task, &link),
            &issue,
            SyncDirection::Pull,
        );
        assert!(!plan.has_conflicts());
        assert_eq!(plan.apply_title.as_deref(), Some("Remote title"));
    }
}

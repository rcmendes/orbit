//! Relationship sync planning in Orbit terms (ADR 0018).
//!
//! Orbit parent/depends_on edges become Remote Issue edges only when both
//! ends link to the same Remote Project on the same tracker; anything else
//! is reported unsupported and preserved.

use crate::remote::link::RemoteLinkView;
use crate::remote::model::{
    DesiredRelation, RelationAction, RelationChange, RelationKind, RemoteIssueRef, RemoteRelations,
};
use crate::task::Task;

/// The remote Issue's current relationships — borrowed slices of a
/// [`RemoteIssue`](crate::remote::model::RemoteIssue).
#[derive(Debug, Clone, Copy)]
pub struct RemoteRelationsView<'a> {
    pub parents: &'a [RemoteIssueRef],
    pub blocking: &'a [RemoteIssueRef],
    pub blocked_by: &'a [RemoteIssueRef],
}

impl RemoteRelationsView<'_> {
    /// Empty remote state for previews where no issue was fetched.
    pub const EMPTY: RemoteRelationsView<'static> = RemoteRelationsView {
        parents: &[],
        blocking: &[],
        blocked_by: &[],
    };
}

impl<'a> From<&'a RemoteRelations> for RemoteRelationsView<'a> {
    fn from(relations: &'a RemoteRelations) -> Self {
        Self {
            parents: &relations.parents,
            blocking: &relations.blocking,
            blocked_by: &relations.blocked_by,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct RelationshipReport {
    pub supported: Vec<DesiredRelation>,
    pub unsupported: Vec<String>,
    pub changes: Vec<RelationChange>,
}

/// Plan relationship reconciliation for one Linked Task. `link` is the
/// task's own link; `remote` its fetched Remote Issue's relations;
/// `child_links`/`dependency_links` are the linked Orbit Tasks on the other
/// end of parent/dependency edges.
pub fn plan_relationship_sync(
    link: &RemoteLinkView,
    remote: &RemoteRelationsView,
    child_links: &[(Task, RemoteLinkView)],
    dependency_links: &[(Task, RemoteLinkView)],
) -> RelationshipReport {
    let mut supported = Vec::new();
    let mut unsupported = Vec::new();
    let mut changes = Vec::new();
    let baseline = link.baseline.as_ref();

    for (child, child_link) in child_links {
        if same_project(link, child_link) {
            let reference = RemoteIssueRef {
                project: child_link.project.clone(),
                id: child_link.id.clone(),
            };
            supported.push(DesiredRelation {
                kind: RelationKind::Subtask,
                orbit_task: child.id.clone(),
                issue: reference.clone(),
            });
            if !remote.blocking.contains(&reference) {
                changes.push(RelationChange {
                    action: RelationAction::Add,
                    kind: RelationKind::Subtask,
                    issue: reference,
                });
            }
        } else {
            unsupported.push(format!(
                "SubTask {} links to {} on {} but the parent links to {} on {}; cross-project sub-tasks are not synced",
                child.id, child_link.project, child_link.tracker, link.project, link.tracker
            ));
        }
    }

    for (dependency, dep_link) in dependency_links {
        if same_project(link, dep_link) {
            let reference = RemoteIssueRef {
                project: dep_link.project.clone(),
                id: dep_link.id.clone(),
            };
            supported.push(DesiredRelation {
                kind: RelationKind::BlockedBy,
                orbit_task: dependency.id.clone(),
                issue: reference.clone(),
            });
            if !remote.blocked_by.contains(&reference) {
                changes.push(RelationChange {
                    action: RelationAction::Add,
                    kind: RelationKind::BlockedBy,
                    issue: reference,
                });
            }
        } else {
            unsupported.push(format!(
                "dependency {} is linked to {} on {}; blocked-by sync requires the same Remote Project as {}",
                dependency.id, dep_link.project, dep_link.tracker, link.project
            ));
        }
    }

    for parent in remote.parents {
        if parent.project != link.project || parent.id != link.id {
            unsupported.push(format!(
                "remote parent issue {}#{} is not represented on this Orbit Board",
                parent.project, parent.id
            ));
        }
    }

    for remote_child in remote.blocking {
        if supported.iter().any(|relationship| {
            relationship.kind == RelationKind::Subtask && relationship.issue == *remote_child
        }) {
            continue;
        }
        let previously_synced = baseline.is_some_and(|base| base.subtasks.contains(remote_child));
        if previously_synced && remote_child.project.eq_ignore_ascii_case(&link.project) {
            changes.push(RelationChange {
                action: RelationAction::Remove,
                kind: RelationKind::Subtask,
                issue: remote_child.clone(),
            });
        } else {
            unsupported.push(format!(
                "remote sub-task {}#{} has no matching linked Orbit SubTask; it is preserved",
                remote_child.project, remote_child.id
            ));
        }
    }
    for remote_dependency in remote.blocked_by {
        if supported.iter().any(|relationship| {
            relationship.kind == RelationKind::BlockedBy && relationship.issue == *remote_dependency
        }) {
            continue;
        }
        let previously_synced =
            baseline.is_some_and(|base| base.blocked_by.contains(remote_dependency));
        if previously_synced
            && remote_dependency
                .project
                .eq_ignore_ascii_case(&link.project)
        {
            changes.push(RelationChange {
                action: RelationAction::Remove,
                kind: RelationKind::BlockedBy,
                issue: remote_dependency.clone(),
            });
        } else {
            unsupported.push(format!(
                "remote blocked-by issue {}#{} has no matching linked Orbit Dependency; it is preserved",
                remote_dependency.project, remote_dependency.id
            ));
        }
    }

    RelationshipReport {
        supported,
        unsupported,
        changes,
    }
}

fn same_project(link: &RemoteLinkView, other: &RemoteLinkView) -> bool {
    link.tracker == other.tracker && link.project.eq_ignore_ascii_case(&other.project)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::task::{TaskFrontmatter, TaskType, default_priority};
    use chrono::TimeZone;
    use std::collections::BTreeMap;

    fn link(project: &str, id: &str) -> RemoteLinkView {
        RemoteLinkView {
            tracker: "test".into(),
            project: project.into(),
            id: id.into(),
            url: None,
            active: true,
            baseline: None,
        }
    }

    fn task(id: &str) -> Task {
        let now = chrono::Utc
            .with_ymd_and_hms(2026, 10, 23, 12, 0, 0)
            .unwrap();
        Task {
            id: id.to_string(),
            front: TaskFrontmatter {
                title: id.to_string(),
                status: crate::task::Status::Todo,
                task_type: TaskType::Task,
                priority: default_priority(),
                labels: Vec::new(),
                depends_on: Vec::new(),
                parent: None,
                waiting_on: None,
                checkpoint: None,
                claim: None,
                stray_claim_times: None,
                created_at: now,
                updated_at: now,
                area: None,
                verification: None,
                github: None,
                remote: None,
                repo: None,
                annotations: BTreeMap::new(),
                extra: BTreeMap::new(),
            },
            body: String::new(),
        }
    }

    #[test]
    fn reports_cross_project_subtasks_as_unsupported() {
        let parent_link = link("acme/a", "1");
        let child = task("orb-aaaaaa.bbbbbb");
        let child_link = link("acme/b", "2");
        let report = plan_relationship_sync(
            &parent_link,
            &RemoteRelationsView::EMPTY,
            &[(child, child_link)],
            &[],
        );
        assert!(report.supported.is_empty());
        assert_eq!(report.unsupported.len(), 1);
    }

    #[test]
    fn adds_missing_same_project_edges() {
        let parent_link = link("acme/a", "1");
        let child = task("orb-aaaaaa.bbbbbb");
        let child_link = link("acme/a", "2");
        let report = plan_relationship_sync(
            &parent_link,
            &RemoteRelationsView::EMPTY,
            &[(child, child_link)],
            &[],
        );
        assert_eq!(report.supported.len(), 1);
        assert_eq!(report.supported[0].kind, RelationKind::Subtask);
        assert_eq!(report.changes.len(), 1);
        assert_eq!(report.changes[0].action, RelationAction::Add);
    }

    #[test]
    fn removes_previously_synced_edges_that_disappeared() {
        let mut parent = link("acme/a", "1");
        parent.baseline = Some(crate::remote::model::SyncBaseline {
            at: chrono::Utc::now(),
            remote_updated_at: chrono::Utc::now(),
            orbit_updated_at: chrono::Utc::now(),
            title: "Parent".into(),
            body: String::new(),
            labels: Vec::new(),
            area: None,
            verification: None,
            status: Some(crate::task::Status::Todo),
            remote_status: Some(crate::task::Status::Todo),
            subtasks: vec![RemoteIssueRef {
                project: "acme/a".into(),
                id: "2".into(),
            }],
            blocked_by: vec![RemoteIssueRef {
                project: "acme/a".into(),
                id: "3".into(),
            }],
        });
        let blocking = [RemoteIssueRef {
            project: "acme/a".into(),
            id: "2".into(),
        }];
        let blocked_by = [RemoteIssueRef {
            project: "acme/a".into(),
            id: "3".into(),
        }];
        let remote = RemoteRelationsView {
            parents: &[],
            blocking: &blocking,
            blocked_by: &blocked_by,
        };
        let report = plan_relationship_sync(&parent, &remote, &[], &[]);
        let removals: Vec<_> = report
            .changes
            .iter()
            .filter(|change| change.action == RelationAction::Remove)
            .map(|change| change.kind)
            .collect();
        assert_eq!(
            removals,
            vec![RelationKind::Subtask, RelationKind::BlockedBy]
        );
        assert!(report.unsupported.is_empty(), "{:?}", report.unsupported);
    }
}

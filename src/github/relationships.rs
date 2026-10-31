use crate::github::link::GitHubLink;
use crate::github::model::{IssueRef, IssueSnapshot};
use crate::task::Task;

/// The remote Issue's current relationships — the only part of an
/// `IssueSnapshot` the planner reads.
#[derive(Debug, Clone, Copy)]
pub struct RemoteRelations<'a> {
    pub parent_issue: &'a [IssueRef],
    pub blocking: &'a [IssueRef],
    pub blocked_by: &'a [IssueRef],
}

impl RemoteRelations<'_> {
    /// Empty remote state for previews where no issue was fetched.
    pub const EMPTY: RemoteRelations<'static> = RemoteRelations {
        parent_issue: &[],
        blocking: &[],
        blocked_by: &[],
    };
}

impl<'a> From<&'a IssueSnapshot> for RemoteRelations<'a> {
    fn from(issue: &'a IssueSnapshot) -> Self {
        Self {
            parent_issue: &issue.parent_issue,
            blocking: &issue.blocking,
            blocked_by: &issue.blocked_by,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct RelationshipReport {
    pub supported: Vec<SupportedRelationship>,
    pub unsupported: Vec<String>,
    pub changes: Vec<RelationshipChange>,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct SupportedRelationship {
    pub kind: &'static str,
    pub orbit_task: String,
    pub github: IssueRef,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct RelationshipChange {
    pub action: &'static str,
    pub kind: &'static str,
    pub github: IssueRef,
}

pub fn plan_relationship_sync(
    link: &GitHubLink,
    remote: &RemoteRelations,
    child_links: &[(Task, GitHubLink)],
    dependency_links: &[(Task, GitHubLink)],
) -> RelationshipReport {
    let mut supported = Vec::new();
    let mut unsupported = Vec::new();
    let mut changes = Vec::new();
    let baseline = link.last_sync.as_ref();

    for (child, child_link) in child_links {
        if child_link.repo == link.repo {
            supported.push(SupportedRelationship {
                kind: "sub_issue",
                orbit_task: child.id.clone(),
                github: IssueRef {
                    repo: child_link.repo.clone(),
                    number: child_link.number,
                },
            });
            let reference = IssueRef {
                repo: child_link.repo.clone(),
                number: child_link.number,
            };
            if !remote.blocking.contains(&reference) {
                changes.push(RelationshipChange {
                    action: "add",
                    kind: "sub_issue",
                    github: reference,
                });
            }
        } else {
            unsupported.push(format!(
                "SubTask {} links to {} but parent links to {}; cross-repository sub-issues are not synced",
                child.id, child_link.repo, link.repo
            ));
        }
    }

    for (dependency, dep_link) in dependency_links {
        if dep_link.repo == link.repo {
            supported.push(SupportedRelationship {
                kind: "blocked_by",
                orbit_task: dependency.id.clone(),
                github: IssueRef {
                    repo: dep_link.repo.clone(),
                    number: dep_link.number,
                },
            });
            let reference = IssueRef {
                repo: dep_link.repo.clone(),
                number: dep_link.number,
            };
            if !remote.blocked_by.contains(&reference) {
                changes.push(RelationshipChange {
                    action: "add",
                    kind: "blocked_by",
                    github: reference,
                });
            }
        } else {
            unsupported.push(format!(
                "dependency {} is linked to {}; blocked-by sync requires the same repository as {}",
                dependency.id, dep_link.repo, link.repo
            ));
        }
    }

    for parent in remote.parent_issue {
        if parent.repo != link.repo || parent.number != link.number {
            unsupported.push(format!(
                "GitHub parent issue {}/#{} is not represented on this Orbit Board",
                parent.repo, parent.number
            ));
        }
    }

    for remote_child in remote.blocking {
        if supported.iter().any(|relationship| {
            relationship.kind == "sub_issue" && relationship.github == *remote_child
        }) {
            continue;
        }
        let previously_synced = baseline.is_some_and(|base| base.sub_issues.contains(remote_child));
        if previously_synced && remote_child.repo.eq_ignore_ascii_case(&link.repo) {
            changes.push(RelationshipChange {
                action: "remove",
                kind: "sub_issue",
                github: remote_child.clone(),
            });
        } else {
            unsupported.push(format!(
                "GitHub sub-issue {}/#{} has no matching linked Orbit SubTask; it is preserved",
                remote_child.repo, remote_child.number
            ));
        }
    }
    for remote_dependency in remote.blocked_by {
        if supported.iter().any(|relationship| {
            relationship.kind == "blocked_by" && relationship.github == *remote_dependency
        }) {
            continue;
        }
        let previously_synced =
            baseline.is_some_and(|base| base.blocked_by.contains(remote_dependency));
        if previously_synced && remote_dependency.repo.eq_ignore_ascii_case(&link.repo) {
            changes.push(RelationshipChange {
                action: "remove",
                kind: "blocked_by",
                github: remote_dependency.clone(),
            });
        } else {
            unsupported.push(format!(
                "GitHub blocked-by issue {}/#{} has no matching linked Orbit Dependency; it is preserved",
                remote_dependency.repo, remote_dependency.number
            ));
        }
    }

    RelationshipReport {
        supported,
        unsupported,
        changes,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::github::model::IssueState;
    use crate::task::{TaskFrontmatter, TaskType, default_priority};
    use chrono::TimeZone;
    use std::collections::BTreeMap;

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
                extra: BTreeMap::new(),
            },
            body: String::new(),
        }
    }

    #[test]
    fn removes_previously_synced_relationships_that_disappeared_locally() {
        let mut link = GitHubLink::new("acme/a".to_string(), 1);
        link.last_sync = Some(crate::github::link::GitHubSyncBaseline {
            at: chrono::Utc::now(),
            issue_updated_at: chrono::Utc::now(),
            orbit_updated_at: chrono::Utc::now(),
            title: "Parent".into(),
            body: String::new(),
            labels: Vec::new(),
            area: None,
            verification: None,
            status: Some(crate::task::Status::Todo),
            issue_state: Some(IssueState::Open),
            issue_state_reason: None,
            sub_issues: vec![IssueRef {
                repo: "acme/a".into(),
                number: 2,
            }],
            blocked_by: vec![IssueRef {
                repo: "acme/a".into(),
                number: 3,
            }],
        });
        let mut issue = IssueSnapshot {
            repo: "acme/a".to_string(),
            number: 1,
            title: "t".into(),
            body: String::new(),
            state: IssueState::Open,
            state_reason: None,
            labels: vec![],
            assignees: vec![],
            updated_at: chrono::Utc::now(),
            area: None,
            verification: None,
            parent_issue: vec![],
            blocked_by: vec![IssueRef {
                repo: "acme/a".into(),
                number: 3,
            }],
            blocking: vec![IssueRef {
                repo: "acme/a".into(),
                number: 2,
            }],
        };

        let report = plan_relationship_sync(&link, &RemoteRelations::from(&issue), &[], &[]);

        let removals = report
            .changes
            .iter()
            .filter(|change| change.action == "remove")
            .map(|change| change.kind)
            .collect::<Vec<_>>();
        assert_eq!(removals, vec!["sub_issue", "blocked_by"]);
        assert!(report.unsupported.is_empty(), "{:?}", report.unsupported);

        issue.blocking.clear();
        issue.blocked_by.clear();
        let report = plan_relationship_sync(&link, &RemoteRelations::from(&issue), &[], &[]);
        assert!(report.changes.is_empty());
    }

    #[test]
    fn preserves_remote_only_relationships_without_a_baseline() {
        let link = GitHubLink::new("acme/a".to_string(), 1);
        let issue = IssueSnapshot {
            repo: "acme/a".to_string(),
            number: 1,
            title: "t".into(),
            body: String::new(),
            state: IssueState::Open,
            state_reason: None,
            labels: vec![],
            assignees: vec![],
            updated_at: chrono::Utc::now(),
            area: None,
            verification: None,
            parent_issue: vec![],
            blocked_by: vec![IssueRef {
                repo: "acme/a".into(),
                number: 3,
            }],
            blocking: vec![IssueRef {
                repo: "acme/a".into(),
                number: 2,
            }],
        };

        let report = plan_relationship_sync(&link, &RemoteRelations::from(&issue), &[], &[]);

        assert!(report.changes.is_empty());
        assert_eq!(report.unsupported.len(), 2);
    }

    #[test]
    fn reports_cross_repository_subtasks_as_unsupported() {
        let parent_link = GitHubLink::new("acme/a".to_string(), 1);
        let child = task("orb-aaaaaa.bbbbbb");
        let child_link = GitHubLink::new("acme/b".to_string(), 2);
        let issue = IssueSnapshot {
            repo: "acme/a".to_string(),
            number: 1,
            title: "t".into(),
            body: String::new(),
            state: IssueState::Open,
            state_reason: None,
            labels: vec![],
            assignees: vec![],
            updated_at: chrono::Utc
                .with_ymd_and_hms(2026, 10, 23, 12, 0, 0)
                .unwrap(),
            area: None,
            verification: None,
            parent_issue: vec![],
            blocked_by: vec![],
            blocking: vec![],
        };
        let report = plan_relationship_sync(
            &parent_link,
            &RemoteRelations::from(&issue),
            &[(child, child_link)],
            &[],
        );
        assert_eq!(report.supported.len(), 0);
        assert_eq!(report.unsupported.len(), 1);
    }
}

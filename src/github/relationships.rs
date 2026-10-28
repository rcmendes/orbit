use crate::error::{OrbitError, Result};
use crate::github::link::GitHubLink;
use crate::github::model::{IssueRef, IssueSnapshot};
use crate::task::Task;

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct RelationshipReport {
    pub supported: Vec<SupportedRelationship>,
    pub unsupported: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct SupportedRelationship {
    pub kind: &'static str,
    pub orbit_task: String,
    pub github: IssueRef,
}

pub fn plan_relationship_sync(
    task: &Task,
    link: &GitHubLink,
    issue: &IssueSnapshot,
    child_links: &[(Task, GitHubLink)],
    dependency_links: &[(Task, GitHubLink)],
) -> RelationshipReport {
    let mut supported = Vec::new();
    let mut unsupported = Vec::new();

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
        } else {
            unsupported.push(format!(
                "dependency {} is linked to {}; blocked-by sync requires the same repository as {}",
                dependency.id, dep_link.repo, link.repo
            ));
        }
    }

    for parent in &issue.parent_issue {
        if parent.repo != link.repo || parent.number != link.number {
            unsupported.push(format!(
                "GitHub parent issue {}/#{} is not represented on this Orbit Board",
                parent.repo, parent.number
            ));
        }
    }

    let _ = task;
    RelationshipReport {
        supported,
        unsupported,
    }
}

pub fn ensure_same_repo(left: &GitHubLink, right: &GitHubLink) -> Result<()> {
    if left.repo.eq_ignore_ascii_case(&right.repo) {
        Ok(())
    } else {
        Err(OrbitError::new(
            crate::error::code::GITHUB_UNSUPPORTED_RELATIONSHIP,
            format!(
                "cannot relate {} and {} across repositories",
                left.identity(),
                right.identity()
            ),
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;
    use crate::github::model::IssueState;
    use crate::task::{TaskFrontmatter, TaskType, default_priority};
    use std::collections::BTreeMap;

    fn task(id: &str) -> Task {
        let now = chrono::Utc.with_ymd_and_hms(2026, 10, 23, 12, 0, 0).unwrap();
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
    fn reports_cross_repository_subtasks_as_unsupported() {
        let parent = task("orb-aaaaaa");
        let parent_link = GitHubLink::new("acme/a".to_string(), 1);
        let child = task("orb-aaaaaa.bbbbbb");
        let child_link = GitHubLink::new("acme/b".to_string(), 2);
        let issue = IssueSnapshot {
            repo: "acme/a".to_string(),
            number: 1,
            title: "t".into(),
            body: String::new(),
            state: IssueState::Open,
            labels: vec![],
            assignees: vec![],
            updated_at: chrono::Utc.with_ymd_and_hms(2026, 10, 23, 12, 0, 0).unwrap(),
            area: None,
            verification: None,
            parent_issue: vec![],
            blocked_by: vec![],
            blocking: vec![],
        };
        let report = plan_relationship_sync(&parent, &parent_link, &issue, &[(child, child_link)], &[]);
        assert_eq!(report.supported.len(), 0);
        assert_eq!(report.unsupported.len(), 1);
    }
}

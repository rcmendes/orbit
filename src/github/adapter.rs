//! The GitHub Remote Tracker adapter (ADR 0018).
//!
//! Everything GitHub-specific lives here: Issue State/reason mapping, the
//! reserved `orbit:in-progress` marker, organization Issue Fields, and the
//! `gh` client. The sync engine only sees [`RemoteIssue`] and
//! [`MappedFields`].

use crate::board::Board;
use crate::error::{OrbitError, Result};
use crate::github::client::{
    CreateIssueInput, GitHubClient, IssueCatalogItem, UpdateIssueInput, client_from_env,
};
use crate::github::config::{GitHubConfig, IssueFieldConfig};
use crate::github::model::IssueSnapshot;
use crate::github::relationships::{RelationshipChange, SupportedRelationship};
use crate::github::status::{
    IN_PROGRESS_LABEL, issue_state_from_status, issue_state_reason_from_status, labels_for_status,
    status_from_issue, user_labels,
};
use crate::remote::RemoteProject;
use crate::remote::model::{
    DesiredRelation, MappedFields, RelationAction, RelationChange, RelationKind, RemoteIssue,
    RemoteIssueFields, RemoteIssueRef, RemoteIssueSummary, RemoteIssueUpdate, RemoteRelations,
};
use crate::remote::port::RemoteAdapter;
use crate::task::Status;

/// The GitHub adapter: a [`GitHubClient`] plus the Board's GitHub settings.
pub struct GitHubAdapter {
    client: Box<dyn GitHubClient>,
    config: GitHubConfig,
}

impl GitHubAdapter {
    /// Production adapter: `gh` CLI or the `ORBIT_GITHUB_FIXTURE` client,
    /// configured from the Board's `[github]`/`[remote.github]` settings.
    pub fn from_board(board: &Board) -> Result<Self> {
        Ok(Self {
            client: client_from_env()?,
            config: effective_config(board),
        })
    }

    /// Adapter over an injected client (tests and `orbit remote` commands
    /// that already resolved one).
    pub fn with_client(client: Box<dyn GitHubClient>, config: GitHubConfig) -> Self {
        Self { client, config }
    }

    /// The effective `[github]` settings: `[remote.github]` (written by
    /// `orbit migrate`) wins over the legacy top-level `[github]` section.
    pub fn effective_config(board: &Board) -> GitHubConfig {
        effective_config(board)
    }

    fn issue_fields(&self) -> Option<&IssueFieldConfig> {
        (!self.config.mode.is_basic()).then_some(&self.config.issue_fields)
    }

    fn fetch(&self, repo: &str, number: u64) -> Result<IssueSnapshot> {
        let mut issue = self.client.get_issue(repo, number, self.issue_fields())?;
        if self.config.mode.is_basic() {
            issue.area = None;
            issue.verification = None;
        }
        Ok(issue)
    }
}

/// Merge the legacy `[github]` section with the migrated `[remote.github]`
/// adapter settings. Repositories always come from `[github] repositories`
/// plus `[remote] projects` on the github tracker.
fn effective_config(board: &Board) -> GitHubConfig {
    let mut config = board.config.remote.github.clone().unwrap_or_else(|| {
        // Legacy boards keep everything under `[github]`; carry the
        // adapter-relevant fields forward.
        let mut config = board.config.github.clone();
        config.auto_push = board.config.remote.auto_push || config.auto_push;
        config
    });
    // `[remote] projects` naming github trackers are Remote Projects, not
    // adapter settings; keep `repositories` as the availability list.
    for project in &board.config.remote.projects {
        if project.tracker == "github"
            && !config
                .repositories
                .iter()
                .any(|repo| repo.trim().eq_ignore_ascii_case(project.project.trim()))
        {
            config.repositories.push(project.project.clone());
        }
    }
    config
}

fn snapshot_to_issue(snapshot: &IssueSnapshot, config: &GitHubConfig) -> RemoteIssue {
    let basic = config.mode.is_basic();
    RemoteIssue {
        project: snapshot.repo.clone(),
        id: snapshot.number.to_string(),
        url: Some(format!(
            "https://github.com/{}/issues/{}",
            snapshot.repo, snapshot.number
        )),
        title: snapshot.title.clone(),
        body: snapshot.body.clone(),
        labels: user_labels(&snapshot.labels),
        markers: snapshot
            .labels
            .iter()
            .filter(|label| label.as_str() == IN_PROGRESS_LABEL)
            .cloned()
            .collect(),
        area: (!basic).then(|| snapshot.area.clone()).flatten(),
        verification: if basic {
            None
        } else {
            snapshot.verification.clone()
        },
        status: status_from_issue(snapshot.state, snapshot.state_reason, &snapshot.labels),
        updated_at: snapshot.updated_at,
        actors: snapshot.assignees.clone(),
        relations: RemoteRelations {
            parents: snapshot
                .parent_issue
                .iter()
                .map(|issue| RemoteIssueRef {
                    project: issue.repo.clone(),
                    id: issue.number.to_string(),
                })
                .collect(),
            blocking: snapshot
                .blocking
                .iter()
                .map(|issue| RemoteIssueRef {
                    project: issue.repo.clone(),
                    id: issue.number.to_string(),
                })
                .collect(),
            blocked_by: snapshot
                .blocked_by
                .iter()
                .map(|issue| RemoteIssueRef {
                    project: issue.repo.clone(),
                    id: issue.number.to_string(),
                })
                .collect(),
        },
    }
}

fn item_to_summary(item: &IssueCatalogItem, config: &GitHubConfig) -> RemoteIssueSummary {
    let basic = config.mode.is_basic();
    RemoteIssueSummary {
        project: item.repo.clone(),
        id: item.number.to_string(),
        url: Some(item.url.clone()),
        title: item.title.clone(),
        status: status_from_issue(item.state, item.state_reason, &item.labels),
        labels: user_labels(&item.labels),
        area: if basic { None } else { item.area.clone() },
        verification: if basic {
            None
        } else {
            item.verification.clone()
        },
        updated_at: item.updated_at,
        fields: item.issue_fields.clone(),
    }
}

/// Orbit status a Remote Issue currently projects, recovering `in_progress`
/// from the reserved marker the mapped status cannot express.
fn orbit_status_of(issue: &RemoteIssue) -> Status {
    if issue.status == Status::Todo && issue.markers.iter().any(|m| m == IN_PROGRESS_LABEL) {
        Status::InProgress
    } else {
        issue.status
    }
}

fn relation_kind(kind: RelationKind) -> &'static str {
    match kind {
        RelationKind::Subtask => "sub_issue",
        RelationKind::BlockedBy => "blocked_by",
    }
}

fn parse_number(id: &str) -> Result<u64> {
    id.parse::<u64>()
        .map_err(|_| OrbitError::invalid_argument(format!("invalid GitHub issue id `{id}`")))
}

impl RemoteAdapter for GitHubAdapter {
    fn tracker(&self) -> &str {
        "github"
    }

    fn mapped_fields(&self, _project: &RemoteProject) -> MappedFields {
        let basic = self.config.mode.is_basic();
        MappedFields {
            title: true,
            body: true,
            labels: true,
            status: true,
            area: !basic,
            verification: !basic,
        }
    }

    fn project_available(&self, board: &Board, project: &str) -> Result<bool> {
        let repo = GitHubConfig::normalize_repo(project)?;
        if crate::repository::github_member_projects(board)?
            .iter()
            .any(|candidate| candidate.eq_ignore_ascii_case(&repo))
        {
            return Ok(true);
        }
        Ok(self.config.contains_repo(&repo)
            || crate::remote::configured_projects(board)
                .iter()
                .any(|candidate| {
                    candidate.tracker == "github" && candidate.project.eq_ignore_ascii_case(&repo)
                }))
    }

    fn implicit_projects(&self, board: &Board) -> Result<Vec<RemoteProject>> {
        let mut projects = Vec::new();
        let mut push = |repo: &str| {
            if let Ok(repo) = GitHubConfig::normalize_repo(repo) {
                projects.push(RemoteProject {
                    tracker: "github".to_string(),
                    project: repo,
                });
            }
        };
        for repository in crate::repository::github_member_projects(board)? {
            push(&repository);
        }
        for repository in &self.config.repositories {
            push(repository);
        }
        Ok(projects)
    }

    fn fetch_issue(&self, project: &RemoteProject, id: &str) -> Result<RemoteIssue> {
        let repo = GitHubConfig::normalize_repo(&project.project)?;
        let number = parse_number(id)?;
        self.fetch(&repo, number)
            .map(|snapshot| snapshot_to_issue(&snapshot, &self.config))
    }

    fn list_issues(&self, project: &RemoteProject) -> Result<Vec<RemoteIssueSummary>> {
        let repo = GitHubConfig::normalize_repo(&project.project)?;
        Ok(self
            .client
            .list_all_issues(&repo, self.issue_fields())?
            .iter()
            .map(|item| item_to_summary(item, &self.config))
            .collect())
    }

    fn create_issue(
        &self,
        project: &RemoteProject,
        fields: RemoteIssueFields,
    ) -> Result<RemoteIssue> {
        let repo = GitHubConfig::normalize_repo(&project.project)?;
        let snapshot = self.client.create_issue(
            &repo,
            &CreateIssueInput {
                title: fields.title,
                body: fields.body,
                labels: labels_for_status(&fields.labels, fields.status),
                area: fields.area,
                verification: fields.verification,
                organization: self.config.organization.clone(),
                issue_fields: self.config.issue_fields.clone(),
            },
        )?;
        Ok(snapshot_to_issue(&snapshot, &self.config))
    }

    fn update_issue(
        &self,
        project: &RemoteProject,
        id: &str,
        update: &RemoteIssueUpdate,
        current: &RemoteIssue,
    ) -> Result<RemoteIssue> {
        let repo = GitHubConfig::normalize_repo(&project.project)?;
        let number = parse_number(id)?;
        let basic = self.config.mode.is_basic();
        let orbit_status = update.status.unwrap_or_else(|| orbit_status_of(current));
        let labels = update
            .labels
            .as_ref()
            .map(|labels| labels_for_status(labels, orbit_status));
        let snapshot = self.client.update_issue(
            &repo,
            number,
            &UpdateIssueInput {
                title: update.title.clone(),
                body: update.body.clone(),
                state: update.status.map(issue_state_from_status),
                state_reason: update.status.and_then(issue_state_reason_from_status),
                labels,
                area: if basic { None } else { update.area.clone() },
                verification: if basic {
                    None
                } else {
                    update.verification.clone()
                },
                organization: self.config.organization.clone(),
                issue_fields: self.issue_fields().cloned(),
            },
        )?;
        Ok(snapshot_to_issue(&snapshot, &self.config))
    }

    fn supports_relationships(&self) -> bool {
        true
    }

    fn update_relationships(
        &self,
        project: &RemoteProject,
        id: &str,
        desired: &[DesiredRelation],
        removals: &[RelationChange],
    ) -> Result<()> {
        let repo = GitHubConfig::normalize_repo(&project.project)?;
        let number = parse_number(id)?;
        let to_ref = |issue: &RemoteIssueRef| -> Result<crate::github::model::IssueRef> {
            if !issue.project.eq_ignore_ascii_case(&repo) {
                return Err(OrbitError::invalid_argument(format!(
                    "GitHub relationships require same-repository issues; {} is not in {repo}",
                    issue.project
                )));
            }
            Ok(crate::github::model::IssueRef {
                repo: repo.clone(),
                number: parse_number(&issue.id)?,
            })
        };
        let supported: Vec<SupportedRelationship> = desired
            .iter()
            .map(|relation| {
                Ok(SupportedRelationship {
                    kind: relation_kind(relation.kind),
                    orbit_task: relation.orbit_task.clone(),
                    github: to_ref(&relation.issue)?,
                })
            })
            .collect::<Result<_>>()?;
        let changes: Vec<RelationshipChange> = removals
            .iter()
            .map(|change| {
                Ok(RelationshipChange {
                    action: match change.action {
                        RelationAction::Add => "add",
                        RelationAction::Remove => "remove",
                    },
                    kind: relation_kind(change.kind),
                    github: to_ref(&change.issue)?,
                })
            })
            .collect::<Result<_>>()?;
        // `removals` only ever carries Remove actions from the planner, but
        // pass Add-capable changes through unchanged for completeness.
        self.client
            .update_relationships(&repo, number, &supported, &changes)
    }

    fn remote_status_for(&self, status: Status) -> Status {
        match status {
            // GitHub cannot express in_progress; it stays an open issue
            // marked by the reserved label instead.
            Status::InProgress => Status::Todo,
            other => other,
        }
    }

    fn status_markers_agree(&self, issue: &RemoteIssue, status: Status) -> bool {
        let has_marker = issue.markers.iter().any(|m| m == IN_PROGRESS_LABEL);
        has_marker == (status == Status::InProgress)
    }
}

/// The reserved in-progress marker this adapter manages.
pub fn in_progress_marker() -> &'static str {
    IN_PROGRESS_LABEL
}

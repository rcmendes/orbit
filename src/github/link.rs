use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::error::{OrbitError, Result};
use crate::github::model::{IssueState, IssueStateReason};
use crate::task::Status;

/// One-to-one link between an Orbit Task and a GitHub Issue (`owner/repo` + number).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct GitHubLink {
    pub repo: String,
    pub number: u64,
    /// `false` keeps the old identity as provenance without enabling sync.
    #[serde(default = "default_active", skip_serializing_if = "is_active")]
    pub active: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_sync: Option<GitHubSyncBaseline>,
}

fn default_active() -> bool {
    true
}

fn is_active(active: &bool) -> bool {
    *active
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct GitHubSyncBaseline {
    pub at: DateTime<Utc>,
    pub issue_updated_at: DateTime<Utc>,
    pub orbit_updated_at: DateTime<Utc>,
    pub title: String,
    pub body: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub labels: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub area: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub verification: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub status: Option<Status>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub issue_state: Option<IssueState>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub issue_state_reason: Option<IssueStateReason>,
    /// Same-repository sub-issues known at the last successful sync. A remote
    /// edge in this set but absent from the Task graph is a local removal.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub sub_issues: Vec<crate::github::model::IssueRef>,
    /// Same-repository blocked-by relationships known at the last sync.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub blocked_by: Vec<crate::github::model::IssueRef>,
}

impl GitHubLink {
    pub fn new(repo: String, number: u64) -> Self {
        Self {
            repo,
            number,
            active: true,
            last_sync: None,
        }
    }

    pub fn identity(&self) -> String {
        format!("{}/#{}", self.repo, self.number)
    }

    /// Provider-neutral form of this link (`remote:` frontmatter, ADR 0018).
    pub fn to_remote_link(&self) -> crate::task::RemoteLink {
        crate::task::RemoteLink {
            tracker: "github".to_string(),
            project: self.repo.clone(),
            id: self.number.to_string(),
            url: Some(format!(
                "https://github.com/{}/issues/{}",
                self.repo, self.number
            )),
            active: self.active,
            baseline: self.last_sync.as_ref().map(|_| {
                // The baseline is re-encoded from the decoded view wherever a
                // RemoteLinkView is available; keep the raw GitHub baseline
                // until then so nothing is lost.
                serde_yaml_ng::to_value(self.last_sync.clone().unwrap()).unwrap_or_default()
            }),
        }
    }

    pub fn validate(&self) -> Result<()> {
        crate::github::config::GitHubConfig::normalize_repo(&self.repo)?;
        if self.number == 0 {
            return Err(OrbitError::invalid_argument(
                "github.number must be a positive issue number",
            ));
        }
        Ok(())
    }
}

impl GitHubSyncBaseline {
    /// Provider-neutral baseline view used by the sync engine.
    pub fn to_sync_baseline(&self) -> crate::remote::model::SyncBaseline {
        crate::remote::model::SyncBaseline {
            at: self.at,
            remote_updated_at: self.issue_updated_at,
            orbit_updated_at: self.orbit_updated_at,
            title: self.title.clone(),
            body: self.body.clone(),
            labels: self.labels.clone(),
            area: self.area.clone(),
            verification: self.verification.clone(),
            status: self.status,
            remote_status: self.issue_state.map(|state| {
                crate::github::status::status_from_issue(
                    state,
                    self.issue_state_reason,
                    &self.labels,
                )
            }),
            subtasks: self
                .sub_issues
                .iter()
                .map(|issue| crate::remote::model::RemoteIssueRef {
                    project: issue.repo.clone(),
                    id: issue.number.to_string(),
                })
                .collect(),
            blocked_by: self
                .blocked_by
                .iter()
                .map(|issue| crate::remote::model::RemoteIssueRef {
                    project: issue.repo.clone(),
                    id: issue.number.to_string(),
                })
                .collect(),
        }
    }

    /// GitHub-shaped view of a provider-neutral [`SyncBaseline`] for output
    /// compatibility. The remote status round-trips through Orbit's Status,
    /// so `issue_state_reason` is reconstructed as `completed`/`not_planned`
    /// and `reopened` cannot be recovered.
    pub fn from_sync_baseline(baseline: &crate::remote::model::SyncBaseline) -> Self {
        let to_ref =
            |issue: &crate::remote::model::RemoteIssueRef| crate::github::model::IssueRef {
                repo: issue.project.clone(),
                number: issue.id.parse::<u64>().unwrap_or(0),
            };
        let (issue_state, issue_state_reason) = match baseline.remote_status {
            Some(Status::Done) => (Some(IssueState::Closed), Some(IssueStateReason::Completed)),
            Some(Status::Cancelled) => {
                (Some(IssueState::Closed), Some(IssueStateReason::NotPlanned))
            }
            Some(_) => (Some(IssueState::Open), None),
            None => (None, None),
        };
        Self {
            at: baseline.at,
            issue_updated_at: baseline.remote_updated_at,
            orbit_updated_at: baseline.orbit_updated_at,
            title: baseline.title.clone(),
            body: baseline.body.clone(),
            labels: baseline.labels.clone(),
            area: baseline.area.clone(),
            verification: baseline.verification.clone(),
            status: baseline.status,
            issue_state,
            issue_state_reason,
            sub_issues: baseline.subtasks.iter().map(to_ref).collect(),
            blocked_by: baseline.blocked_by.iter().map(to_ref).collect(),
        }
    }
}

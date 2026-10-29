use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::error::{OrbitError, Result};
use crate::github::model::IssueState;
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

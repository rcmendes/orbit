use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::task::Status;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct IssueSnapshot {
    pub repo: String,
    pub number: u64,
    pub title: String,
    pub body: String,
    pub state: IssueState,
    pub labels: Vec<String>,
    pub assignees: Vec<String>,
    pub updated_at: DateTime<Utc>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub area: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub verification: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub parent_issue: Vec<IssueRef>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub blocked_by: Vec<IssueRef>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub blocking: Vec<IssueRef>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum IssueState {
    Open,
    Closed,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct IssueRef {
    pub repo: String,
    pub number: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SyncConflict {
    pub field: String,
    pub orbit_value: serde_json::Value,
    pub github_value: serde_json::Value,
    pub message: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum SyncDirection {
    #[default]
    Both,
    Pull,
    Push,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SyncPlan {
    pub direction: SyncDirection,
    pub apply_title: Option<String>,
    pub apply_body: Option<String>,
    pub apply_status: Option<Status>,
    pub apply_labels: Option<Vec<String>>,
    pub apply_area: Option<Option<String>>,
    pub apply_verification: Option<Option<String>>,
    pub push_labels: Option<Vec<String>>,
    pub push_area: Option<Option<String>>,
    pub push_verification: Option<Option<String>>,
    pub push_state: Option<IssueState>,
    pub conflicts: Vec<SyncConflict>,
    pub warnings: Vec<String>,
}

impl SyncPlan {
    pub fn has_conflicts(&self) -> bool {
        !self.conflicts.is_empty()
    }

    pub fn is_noop(&self) -> bool {
        self.apply_title.is_none()
            && self.apply_body.is_none()
            && self.apply_status.is_none()
            && self.apply_labels.is_none()
            && self.apply_area.is_none()
            && self.apply_verification.is_none()
            && self.push_labels.is_none()
            && self.push_area.is_none()
            && self.push_verification.is_none()
            && self.push_state.is_none()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SyncResolution {
    PreferOrbit,
    PreferGitHub,
    Skip,
}

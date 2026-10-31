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
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub state_reason: Option<IssueStateReason>,
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

/// GitHub's closed-issue reason: `completed` or `not_planned`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum IssueStateReason {
    Completed,
    NotPlanned,
    Reopened,
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
    pub push_title: Option<String>,
    pub push_body: Option<String>,
    pub push_labels: Option<Vec<String>>,
    pub push_area: Option<Option<String>>,
    pub push_verification: Option<Option<String>>,
    pub push_state: Option<IssueState>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub push_state_reason: Option<IssueStateReason>,
    /// The reserved `orbit:in-progress` label must be added or removed even
    /// though the Issue state itself is unchanged.
    #[serde(default, skip_serializing_if = "is_false")]
    pub push_status_label: bool,
    pub conflicts: Vec<SyncConflict>,
    pub warnings: Vec<String>,
}

fn is_false(value: &bool) -> bool {
    !*value
}

impl SyncPlan {
    pub fn has_conflicts(&self) -> bool {
        !self.conflicts.is_empty()
    }

    /// Field names with a pending pull into Orbit. The single list that
    /// `is_noop`, change counts, and previews share so a new synced field is
    /// registered in one place.
    pub fn pull_changes(&self) -> Vec<&'static str> {
        [
            ("title", self.apply_title.is_some()),
            ("body", self.apply_body.is_some()),
            ("status", self.apply_status.is_some()),
            ("labels", self.apply_labels.is_some()),
            ("area", self.apply_area.is_some()),
            ("verification", self.apply_verification.is_some()),
        ]
        .into_iter()
        .filter_map(|(name, pending)| pending.then_some(name))
        .collect()
    }

    /// Field names with a pending push to GitHub.
    pub fn push_changes(&self) -> Vec<&'static str> {
        [
            ("title", self.push_title.is_some()),
            ("body", self.push_body.is_some()),
            ("labels", self.push_labels.is_some()),
            ("area", self.push_area.is_some()),
            ("verification", self.push_verification.is_some()),
            ("state", self.push_state.is_some()),
            ("status_label", self.push_status_label),
        ]
        .into_iter()
        .filter_map(|(name, pending)| pending.then_some(name))
        .collect()
    }

    pub fn is_noop(&self) -> bool {
        self.pull_changes().is_empty() && self.push_changes().is_empty()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SyncResolution {
    PreferOrbit,
    PreferGitHub,
    Skip,
}

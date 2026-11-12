use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

/// A GitHub Issue as returned by the REST API — the adapter's wire model.
/// The sync engine consumes [`crate::remote::model::RemoteIssue`] instead;
/// [`crate::github::adapter`] translates between the two.
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

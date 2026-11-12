//! Provider-neutral Remote Tracker model (ADR 0018).
//!
//! The sync engine works exclusively in these Orbit-vocabulary types. A
//! Remote Tracker adapter translates its own issue representation into
//! `RemoteIssue`/`RemoteIssueSummary` and declares which Orbit fields it
//! maps through `MappedFields`; everything else stays local-only.

use std::collections::BTreeMap;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::task::Status;

/// Identity of a Remote Issue inside its Remote Project.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RemoteIssueRef {
    /// Remote Project id in the tracker's own terms (`owner/repo` for GitHub).
    pub project: String,
    /// Remote Issue id in the tracker's own terms (the issue number for GitHub).
    pub id: String,
}

impl RemoteIssueRef {
    /// Tracker-normalized project comparison: GitHub repository names are
    /// case-insensitive, and adapters present already-normalized projects.
    pub fn same_project(&self, other: &Self) -> bool {
        self.project.eq_ignore_ascii_case(&other.project)
    }
}

/// Remote-side relationship edges in Orbit terms: `parents` are Remote
/// Projects this issue sits under (SubTask relation), `blocking` are Remote
/// Issues this one blocks (remote children), and `blocked_by` are Remote
/// Issues that block this one (Orbit dependencies).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct RemoteRelations {
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub parents: Vec<RemoteIssueRef>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub blocking: Vec<RemoteIssueRef>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub blocked_by: Vec<RemoteIssueRef>,
}

/// A Remote Issue translated into Orbit field terms — the adapter boundary
/// the sync engine consumes.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RemoteIssue {
    /// Remote Project id in tracker terms.
    pub project: String,
    /// Remote Issue id in tracker terms.
    pub id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub url: Option<String>,
    pub title: String,
    pub body: String,
    /// User labels only; adapter-managed markers live in `markers`.
    #[serde(default)]
    pub labels: Vec<String>,
    /// Adapter-managed labels present on the Remote Issue (for GitHub the
    /// reserved `orbit:in-progress` marker). Excluded from `labels`.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub markers: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub area: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub verification: Option<String>,
    /// The Remote Issue's status mapped onto Orbit's Status.
    pub status: Status,
    pub updated_at: DateTime<Utc>,
    /// Informational actor identities (assignees for GitHub). Never mapped
    /// to Orbit claims.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub actors: Vec<String>,
    #[serde(default)]
    pub relations: RemoteRelations,
}

/// A catalog row: one Remote Issue summarized for the Remote Issue catalog.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RemoteIssueSummary {
    pub project: String,
    pub id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub url: Option<String>,
    pub title: String,
    pub status: Status,
    #[serde(default)]
    pub labels: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub area: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub verification: Option<String>,
    pub updated_at: DateTime<Utc>,
    /// Adapter-declared raw field values kept for display only.
    #[serde(default)]
    pub fields: BTreeMap<String, serde_json::Value>,
}

/// Orbit fields an adapter publishes for one Remote Project. Fields the
/// adapter does not map stay local-only: the engine never pushes them and
/// never pulls remote values into them.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct MappedFields {
    pub title: bool,
    pub body: bool,
    pub labels: bool,
    pub status: bool,
    pub area: bool,
    pub verification: bool,
}

impl MappedFields {
    pub const ALL: Self = Self {
        title: true,
        body: true,
        labels: true,
        status: true,
        area: true,
        verification: true,
    };

    pub fn maps(&self, field: &str) -> bool {
        match field {
            "title" => self.title,
            "body" => self.body,
            "labels" => self.labels,
            "status" => self.status,
            "area" => self.area,
            "verification" => self.verification,
            _ => false,
        }
    }

    /// Orbit field names this adapter does not publish.
    pub fn unmapped(&self) -> Vec<&'static str> {
        [
            ("title", self.title),
            ("body", self.body),
            ("labels", self.labels),
            ("status", self.status),
            ("area", self.area),
            ("verification", self.verification),
        ]
        .into_iter()
        .filter_map(|(name, mapped)| (!mapped).then_some(name))
        .collect()
    }
}

/// Orbit fields used to create a Remote Issue. `labels` holds user labels;
/// the adapter adds its own status markers.
#[derive(Debug, Clone)]
pub struct RemoteIssueFields {
    pub title: String,
    pub body: String,
    pub labels: Vec<String>,
    pub status: Status,
    pub area: Option<String>,
    pub verification: Option<String>,
}

/// Provider-neutral update applied to a Remote Issue. `Some` outer values
/// mark a field for update; for `area`/`verification` the inner `Option`
/// distinguishes "set" from "clear".
#[derive(Debug, Clone, Default)]
pub struct RemoteIssueUpdate {
    pub title: Option<String>,
    pub body: Option<String>,
    /// Desired user labels; the adapter reconciles its status markers from
    /// `status` (or the Remote Issue's current status when `None`).
    pub labels: Option<Vec<String>>,
    /// Desired Orbit status; `None` leaves remote status untouched.
    pub status: Option<Status>,
    pub area: Option<Option<String>>,
    pub verification: Option<Option<String>>,
}

/// The last successfully synced snapshot, stored opaquely on the Task's
/// Remote link. Owned by the sync engine; adapters never read it directly.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SyncBaseline {
    pub at: DateTime<Utc>,
    /// `updated_at` of the Remote Issue when the baseline was written.
    pub remote_updated_at: DateTime<Utc>,
    pub orbit_updated_at: DateTime<Utc>,
    pub title: String,
    pub body: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub labels: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub area: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub verification: Option<String>,
    /// Orbit status when the baseline was written.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub status: Option<Status>,
    /// Mapped remote status when the baseline was written.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub remote_status: Option<Status>,
    /// Remote sub-task edges known at the last sync; a remote edge present
    /// here but absent from the Task graph is a local removal.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub subtasks: Vec<RemoteIssueRef>,
    /// Remote blocked-by edges known at the last sync.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub blocked_by: Vec<RemoteIssueRef>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SyncConflict {
    pub field: String,
    pub orbit_value: serde_json::Value,
    pub remote_value: serde_json::Value,
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
    /// Desired Orbit status to publish; the adapter maps it onto the
    /// Remote Issue's own status representation.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub push_status: Option<Status>,
    pub conflicts: Vec<SyncConflict>,
    pub warnings: Vec<String>,
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

    /// Field names with a pending push to the Remote Tracker.
    pub fn push_changes(&self) -> Vec<&'static str> {
        [
            ("title", self.push_title.is_some()),
            ("body", self.push_body.is_some()),
            ("labels", self.push_labels.is_some()),
            ("area", self.push_area.is_some()),
            ("verification", self.push_verification.is_some()),
            ("status", self.push_status.is_some()),
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
    PreferRemote,
    Skip,
}

/// Relationship kinds the sync engine understands, in Orbit terms.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RelationKind {
    /// A SubTask edge: this Remote Issue is a remote child of another.
    Subtask,
    /// A dependency edge: this Remote Issue is blocked by another.
    BlockedBy,
}

impl RelationKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Subtask => "subtask",
            Self::BlockedBy => "blocked_by",
        }
    }
}

/// A desired remote relationship edge plus the Orbit Task on its other end.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct DesiredRelation {
    pub kind: RelationKind,
    pub orbit_task: String,
    pub issue: RemoteIssueRef,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum RelationAction {
    Add,
    Remove,
}

impl RelationAction {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Add => "add",
            Self::Remove => "remove",
        }
    }
}

/// A remote relationship edge to add or remove.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct RelationChange {
    pub action: RelationAction,
    pub kind: RelationKind,
    pub issue: RemoteIssueRef,
}

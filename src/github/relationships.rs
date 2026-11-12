//! GitHub relationship wire types (ADR 0018). Relationship *planning* is
//! provider-neutral and lives in [`crate::remote::relationships`]; these
//! structs are what the `gh` client sends to the GitHub sub-issues API.

use crate::github::model::IssueRef;

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

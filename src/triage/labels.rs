use crate::error::{OrbitError, Result, code};

pub const CANONICAL_TRIAGE_LABELS: &[&str] = &[
    "needs-triage",
    "needs-info",
    "ready-for-agent",
    "ready-for-human",
    "wontfix",
    "bug",
    "enhancement",
];

pub fn is_canonical(label: &str) -> bool {
    CANONICAL_TRIAGE_LABELS.contains(&label)
}

pub fn validate_label_list(labels: &[String], field: &str) -> Result<()> {
    for label in labels {
        if !is_canonical(label) {
            return Err(OrbitError::new(
                code::TRIAGE_INVALID_PROPOSAL,
                format!(
                    "{field} contains non-canonical label `{label}`; allowed: {}",
                    CANONICAL_TRIAGE_LABELS.join(", ")
                ),
            ));
        }
    }
    Ok(())
}

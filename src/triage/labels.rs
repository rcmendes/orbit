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

pub const STATE_ROLES: &[&str] = &[
    "needs-triage",
    "needs-info",
    "ready-for-agent",
    "ready-for-human",
    "wontfix",
];

pub const CATEGORY_ROLES: &[&str] = &["bug", "enhancement"];

pub fn is_canonical(label: &str) -> bool {
    CANONICAL_TRIAGE_LABELS.contains(&label)
}

pub fn validate_state_roles(labels: &[String]) -> Result<()> {
    reject_conflicting_roles(labels, STATE_ROLES, "state")
}

pub fn validate_category_roles(labels: &[String]) -> Result<()> {
    reject_conflicting_roles(labels, CATEGORY_ROLES, "category")
}

fn reject_conflicting_roles(labels: &[String], roles: &[&str], kind: &str) -> Result<()> {
    let conflicting: Vec<&str> = roles
        .iter()
        .copied()
        .filter(|role| labels.iter().any(|label| label == role))
        .collect();
    if conflicting.len() > 1 {
        return Err(OrbitError::new(
            code::TRIAGE_INVALID_PROPOSAL,
            format!(
                "proposal would leave conflicting {kind} roles: {}",
                conflicting.join(", ")
            ),
        ));
    }
    Ok(())
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

#[cfg(test)]
mod tests {
    use super::{CANONICAL_TRIAGE_LABELS, CATEGORY_ROLES, STATE_ROLES};

    #[test]
    fn canonical_triage_labels_are_state_roles_then_category_roles() {
        let mut expected = Vec::with_capacity(STATE_ROLES.len() + CATEGORY_ROLES.len());
        expected.extend_from_slice(STATE_ROLES);
        expected.extend_from_slice(CATEGORY_ROLES);
        assert_eq!(CANONICAL_TRIAGE_LABELS, expected.as_slice());
    }
}

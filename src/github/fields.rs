use crate::error::{OrbitError, Result};

pub const AREA_VALUES: &[&str] = &[
    "backend",
    "frontend",
    "infra",
    "docs",
    "agents",
    "product",
    "other",
];

pub fn normalize_area(value: &str) -> Result<String> {
    let normalized = value.trim().to_ascii_lowercase();
    if normalized.is_empty() {
        return Err(OrbitError::invalid_argument("area must be non-empty"));
    }
    if !AREA_VALUES.contains(&normalized.as_str()) {
        return Err(OrbitError::invalid_argument(format!(
            "area must be one of: {}",
            AREA_VALUES.join(", ")
        )));
    }
    Ok(normalized)
}

pub fn validate_verification(value: &str) -> Result<()> {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        return Err(OrbitError::invalid_argument(
            "verification must be non-empty when set",
        ));
    }
    if trimmed.len() > 500 {
        return Err(OrbitError::invalid_argument(
            "verification must be at most 500 characters",
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accepts_controlled_area_values() {
        assert_eq!(normalize_area("Backend").unwrap(), "backend");
    }

    #[test]
    fn rejects_unknown_area_values() {
        assert_eq!(
            normalize_area("mobile").unwrap_err().code,
            crate::error::code::INVALID_ARGUMENT
        );
    }
}

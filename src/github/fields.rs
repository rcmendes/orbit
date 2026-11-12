use crate::error::{OrbitError, Result};

/// Parses an Area value from a GitHub payload or command argument. Remote
/// areas are free-form slugs; the Board's `board.areas` restriction applies
/// at the command layer through `task::normalize_area`.
pub fn parse_area(value: &str) -> Result<String> {
    let normalized = value.trim().to_ascii_lowercase();
    if !crate::task::is_valid_area(&normalized) {
        return Err(OrbitError::invalid_argument(
            "area must be a short lowercase slug of letters, digits, and `-`",
        ));
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
    if trimmed.len() > crate::task::MAX_VERIFICATION_LENGTH {
        return Err(OrbitError::invalid_argument(format!(
            "verification must be at most {} characters",
            crate::task::MAX_VERIFICATION_LENGTH
        )));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accepts_lowercase_area_slugs() {
        assert_eq!(parse_area("cli").unwrap(), "cli");
        assert_eq!(parse_area("Mobile-Api").unwrap(), "mobile-api");
    }

    #[test]
    fn rejects_non_slug_area_values() {
        assert_eq!(
            parse_area("Bad Area").unwrap_err().code,
            crate::error::code::INVALID_ARGUMENT
        );
    }
}

use crate::error::{OrbitError, Result};
use crate::task::TaskArea;

pub fn area_to_str(area: TaskArea) -> &'static str {
    area.as_str()
}

pub fn parse_area(value: &str) -> Result<TaskArea> {
    let normalized = value.trim();
    TaskArea::all()
        .iter()
        .copied()
        .find(|area| area.as_str() == normalized)
        .ok_or_else(|| {
            OrbitError::invalid_argument(format!(
                "area must be one of: {}",
                TaskArea::all()
                    .iter()
                    .map(|area| area.as_str())
                    .collect::<Vec<_>>()
                    .join(", ")
            ))
        })
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
    fn accepts_controlled_area_values() {
        assert_eq!(parse_area("cli").unwrap(), TaskArea::Cli);
    }

    #[test]
    fn rejects_unknown_area_values() {
        assert_eq!(
            parse_area("mobile").unwrap_err().code,
            crate::error::code::INVALID_ARGUMENT
        );
    }
}

//! Orbit field validators. The implementations are provider-neutral and
//! live in [`crate::task`]; re-exported here for GitHub-side callers.

pub use crate::task::{parse_area, validate_verification};

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accepts_controlled_area_values() {
        assert_eq!(parse_area("cli").unwrap(), crate::task::TaskArea::Cli);
    }

    #[test]
    fn rejects_unknown_area_values() {
        assert_eq!(
            parse_area("mobile").unwrap_err().code,
            crate::error::code::INVALID_ARGUMENT
        );
    }
}

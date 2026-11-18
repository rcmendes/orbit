//! The Board contract version (ADR 0022).
//!
//! `config.toml` carries `contract = "<semver>"`: the contract version the
//! Board was written for. Every Orbit binary speaks exactly one contract —
//! [`VERSION`] — and refuses to open a Board whose contract is newer, so an
//! older binary never silently misreads a newer Board. `orbit capabilities`
//! reports both versions so a consumer can negotiate before choosing commands.
//!
//! A `config.toml` without the field is a legacy Board: opening it fails with
//! `MODEL_NOT_MIGRATED` pointing at `orbit migrate model`, and so does a Board on
//! an older major. The serde default below only applies to the migration, which
//! writes the field.

use crate::error::{OrbitError, Result, code};

/// The contract version this binary implements.
pub const VERSION: &str = "1.0.0";

/// Serde default for `Config::contract`.
pub fn default_version() -> String {
    VERSION.to_string()
}

/// A parsed `major.minor.patch` version. Only ordering matters: a binary
/// accepts a Board whose contract is not newer than its own.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct Semver {
    pub major: u64,
    pub minor: u64,
    pub patch: u64,
}

impl Semver {
    /// Parses `major.minor.patch`; `major` or `major.minor` are accepted and
    /// the missing components read as zero.
    pub fn parse(text: &str) -> Result<Self> {
        let invalid = || {
            OrbitError::config(format!(
                "contract version `{text}` is not semver (expected major.minor.patch)"
            ))
        };
        let parts: Vec<&str> = text.trim().split('.').collect();
        if parts.is_empty() || parts.len() > 3 {
            return Err(invalid());
        }
        let mut numbers = [0u64; 3];
        for (index, part) in parts.iter().enumerate() {
            if part.is_empty() || !part.chars().all(|c| c.is_ascii_digit()) {
                return Err(invalid());
            }
            numbers[index] = part.parse::<u64>().map_err(|_| invalid())?;
        }
        Ok(Self {
            major: numbers[0],
            minor: numbers[1],
            patch: numbers[2],
        })
    }
}

/// The binary's own contract as a parsed version.
pub fn binary_semver() -> Semver {
    // VERSION is a compile-time constant; a parse failure is a build bug.
    Semver::parse(VERSION).expect("orbit contract VERSION must be semver")
}

/// Refuses a Board whose contract is newer than this binary's. Called from
/// `Config::validate`, the single choke point every Board open passes through.
pub fn check_compatible(board_contract: &str) -> Result<()> {
    let board = Semver::parse(board_contract)?;
    let binary = binary_semver();
    if board.major < binary.major {
        return Err(OrbitError::new(
            code::MODEL_NOT_MIGRATED,
            format!(
                "Board contract {board_contract} is older than this orbit's major ({VERSION}); run `orbit migrate model`"
            ),
        ));
    }
    if board > binary {
        return Err(OrbitError::new(
            code::CONTRACT_VERSION_UNSUPPORTED,
            format!(
                "Board contract {board_contract} is newer than this orbit supports ({VERSION}); upgrade orbit"
            ),
        )
        .with_details(serde_json::json!({
            "board_contract": board_contract,
            "binary_contract": VERSION,
        })));
    }
    Ok(())
}

/// `true` when a Board on `board_contract` can be opened by this binary.
pub fn is_compatible(board_contract: &str) -> bool {
    check_compatible(board_contract).is_ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn semver_orders() {
        assert!(Semver::parse("2.0.0").unwrap() > Semver::parse("1.9.9").unwrap());
        assert!(Semver::parse("1.0.1").unwrap() > Semver::parse("1.0.0").unwrap());
        assert!(Semver::parse("1").unwrap() == Semver::parse("1.0.0").unwrap());
        assert!(Semver::parse("1.2").unwrap() == Semver::parse("1.2.0").unwrap());
        assert!(Semver::parse("1.0.0-rc1").is_err());
        assert!(Semver::parse("v1.0.0").is_err());
        assert!(Semver::parse("").is_err());
    }

    #[test]
    fn newer_board_is_refused() {
        let error = check_compatible("99.0.0").unwrap_err();
        assert_eq!(error.code, code::CONTRACT_VERSION_UNSUPPORTED);
        assert!(check_compatible(VERSION).is_ok());
        assert_eq!(
            check_compatible("0.9.0").unwrap_err().code,
            code::MODEL_NOT_MIGRATED
        );
        assert!(check_compatible("not-semver").is_err());
    }
}

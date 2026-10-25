//! Board Slugs and Task-ID prefixes.
//!
//! A Board stores its literal prefix in `id.prefix` (`teramigo-`). The bare
//! slug (`teramigo`) is only a CLI input and a domain term. Auto-derived slugs
//! are normalized and never block Board creation; explicit `--slug` input is
//! strict.

use std::path::{Path, PathBuf};

use crate::error::{OrbitError, Result, code};

pub const MAX_LEN: usize = 20;
pub const FALLBACK: &str = "orb";

/// Normalizes a folder or anchor name into a slug.
///
/// Lowercases, turns every run outside `[a-z0-9]` into one dash, trims dashes,
/// falls back to `orb`, then truncates to [`MAX_LEN`] at a dash boundary.
pub fn normalize(input: &str) -> String {
    let mut slug = String::with_capacity(input.len());
    let mut pending_dash = false;

    for character in input.chars() {
        if character.is_ascii_alphanumeric() {
            if pending_dash && !slug.is_empty() {
                slug.push('-');
            }
            pending_dash = false;
            slug.push(character.to_ascii_lowercase());
        } else {
            pending_dash = true;
        }
    }

    if slug.is_empty() {
        return FALLBACK.to_string();
    }
    truncate(&slug)
}

fn truncate(slug: &str) -> String {
    if slug.len() <= MAX_LEN {
        return slug.to_string();
    }

    let cut = &slug[..MAX_LEN];
    let candidate = match cut.rfind('-') {
        Some(index) => &cut[..index],
        None => cut,
    };
    let candidate = candidate.trim_end_matches('-');
    if candidate.is_empty() {
        FALLBACK.to_string()
    } else {
        candidate.to_string()
    }
}

/// Strict validation for an explicit `--slug` value.
pub fn is_valid(slug: &str) -> bool {
    !slug.is_empty()
        && slug.len() <= MAX_LEN
        && slug.chars().all(|character| {
            character.is_ascii_lowercase() || character.is_ascii_digit() || character == '-'
        })
        && slug
            .chars()
            .next()
            .is_some_and(|character| character.is_ascii_alphanumeric())
        && slug
            .chars()
            .last()
            .is_some_and(|character| character.is_ascii_alphanumeric())
        && !slug.contains("--")
}

/// The normalization used in an `INVALID_SLUG` suggestion.
pub fn suggest(input: &str) -> String {
    normalize(input)
}

/// The literal prefix stored in `id.prefix`.
pub fn prefix(slug: &str) -> String {
    format!("{slug}-")
}

/// The bare slug stored in a literal prefix. Legacy prefixes may omit the dash.
pub fn slug_of_prefix(prefix: &str) -> &str {
    prefix.strip_suffix('-').unwrap_or(prefix)
}

/// Validates an explicit slug and returns its literal prefix.
pub fn explicit_prefix(input: &str) -> Result<String> {
    if !is_valid(input) {
        return Err(OrbitError::new(
            code::INVALID_SLUG,
            format!("invalid slug `{input}`; use `{}`", suggest(input)),
        ));
    }
    Ok(prefix(input))
}

/// Auto-adopts a folder name. This never fails.
pub fn from_folder_name(name: &str) -> String {
    normalize(name)
}

/// The folder used for auto-adoption during `orbit init`.
///
/// This is the parent of the Git common directory when that directory is
/// literally named `.git`; otherwise the repository root. It is stable across
/// linked worktrees, submodules, and `--separate-git-dir`.
pub fn main_checkout_anchor() -> Result<PathBuf> {
    let common = crate::git::common_dir()?;
    if common.file_name().and_then(|name| name.to_str()) == Some(".git")
        && let Some(parent) = common.parent()
    {
        return Ok(parent.to_path_buf());
    }
    crate::git::repo_root()
}

/// Auto slug and optional warning for `init DIR`.
pub fn auto_slug_for_dir(dir: &Path) -> (String, Option<String>) {
    let home = std::env::var_os("HOME")
        .or_else(|| std::env::var_os("USERPROFILE"))
        .map(PathBuf::from)
        .and_then(|home| dunce::canonicalize(home).ok());
    if home
        .as_deref()
        .is_some_and(|home| dir == home.join(".orbit"))
    {
        return (
            FALLBACK.to_string(),
            Some("global Board uses the orb prefix; pass --slug to choose another".to_string()),
        );
    }

    let anchor = if dir.file_name().and_then(|name| name.to_str()) == Some(".orbit") {
        dir.parent().unwrap_or(dir)
    } else {
        dir
    };
    (
        from_folder_name(&anchor_name(anchor)),
        home_or_root_warning(anchor),
    )
}

fn anchor_name(path: &Path) -> String {
    path.file_name()
        .and_then(|name| name.to_str())
        .unwrap_or_default()
        .to_string()
}

fn home_or_root_warning(anchor: &Path) -> Option<String> {
    let is_root = anchor.parent().is_none();
    let home = std::env::var_os("HOME")
        .or_else(|| std::env::var_os("USERPROFILE"))
        .map(PathBuf::from)
        .and_then(|home| dunce::canonicalize(home).ok());
    let is_home = home.as_deref() == Some(anchor);

    (is_root || is_home)
        .then(|| "anchor is the home directory; pass --slug to choose the task prefix".to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalization_table() {
        for (input, expected) in [
            ("Teramigo", "teramigo"),
            ("teramigo_api", "teramigo-api"),
            ("teramigo.api", "teramigo-api"),
            ("Téramigo", "t-ramigo"),
            ("---", "orb"),
            ("teramigo-customer-portal-redesign", "teramigo-customer"),
            (".config", "config"),
            ("", "orb"),
            ("   ", "orb"),
            ("abcdefghijklmnopqrstu", "abcdefghijklmnopqrst"),
            ("abc-def-ghijklmnopqrstuv", "abc-def"),
            ("a-------------------b", "a-b"),
        ] {
            assert_eq!(normalize(input), expected, "{input}");
        }
    }

    #[test]
    fn explicit_validation_is_strict() {
        let twenty = "a".repeat(20);
        for valid in ["teramigo", "a", "a-b", "orb", twenty.as_str()] {
            assert!(is_valid(valid), "{valid}");
        }

        let twenty_one = "a".repeat(21);
        for invalid in [
            "",
            "Teramigo",
            "teramigo_api",
            "teramigo.api",
            "teramigo api",
            "téramigo",
            "-teramigo",
            "teramigo-",
            "teramigo--api",
            twenty_one.as_str(),
        ] {
            assert!(!is_valid(invalid), "{invalid}");
        }
    }

    #[test]
    fn suggestions_are_normalized() {
        assert_eq!(suggest("Teramigo_API"), "teramigo-api");
        assert_eq!(suggest("---"), "orb");
    }

    #[test]
    fn prefix_slug_ignores_the_optional_dash() {
        assert_eq!(slug_of_prefix("orb-"), "orb");
        assert_eq!(slug_of_prefix("orb"), "orb");
        assert_eq!(slug_of_prefix("teramigo-"), "teramigo");
    }

    #[test]
    fn explicit_prefix_validates_and_wraps_the_slug() {
        assert_eq!(explicit_prefix("teramigo").unwrap(), "teramigo-");

        let error = explicit_prefix("Teramigo_API").unwrap_err();
        assert_eq!(error.code, code::INVALID_SLUG);
        assert!(
            error.message.contains("use `teramigo-api`"),
            "{}",
            error.message
        );
    }

    #[test]
    fn init_autoslug_uses_the_dir_or_its_orbit_folder_parent() {
        let (slug, warning) = auto_slug_for_dir(Path::new("/tmp/Teramigo_API"));
        assert_eq!(slug, "teramigo-api");
        assert!(warning.is_none());

        let (slug, warning) = auto_slug_for_dir(Path::new("/tmp/Teramigo_API/.orbit"));
        assert_eq!(slug, "teramigo-api");
        assert!(warning.is_none());

        let (slug, warning) = auto_slug_for_dir(Path::new("/"));
        assert_eq!(slug, "orb");
        assert!(warning.is_some());
    }
}

use serde::{Deserialize, Serialize};

use crate::error::{OrbitError, Result};

/// First Orbit release that includes the GitHub provider in its default binary.
pub const MIN_GITHUB_VERSION: &str = "0.9.0";

/// Board-level GitHub provider configuration (`config.toml` `[github]` section).
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct GitHubConfig {
    /// Provider capabilities. `full` preserves organization Issue Fields behavior.
    #[serde(default, skip_serializing_if = "GitHubMode::is_full")]
    pub mode: GitHubMode,
    /// GitHub organization that owns org-level Issue Fields for `area` and `verification`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub organization: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub repositories: Vec<String>,
    /// Publish mapped Task changes to linked Issues after a local mutation.
    #[serde(default, skip_serializing_if = "is_false")]
    pub auto_push: bool,
    #[serde(default)]
    pub issue_fields: IssueFieldConfig,
}

fn is_false(value: &bool) -> bool {
    !*value
}

#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum GitHubMode {
    #[default]
    Full,
    Basic,
}

impl GitHubMode {
    fn is_full(&self) -> bool {
        *self == Self::Full
    }

    pub fn is_basic(self) -> bool {
        self == Self::Basic
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct IssueFieldConfig {
    /// Human-readable Issue Field name in the organization (default `Area`).
    #[serde(default = "default_area_name")]
    pub area_name: String,
    /// Human-readable Issue Field name in the organization (default `Verification`).
    #[serde(default = "default_verification_name")]
    pub verification_name: String,
    /// Resolved GraphQL node id for the area field after `orbit github setup`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub area_id: Option<String>,
    /// Resolved GraphQL node id for the verification field after `orbit github setup`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub verification_id: Option<String>,
}

fn default_area_name() -> String {
    "Area".to_string()
}

fn default_verification_name() -> String {
    "Verification".to_string()
}

impl Default for IssueFieldConfig {
    fn default() -> Self {
        Self {
            area_name: default_area_name(),
            verification_name: default_verification_name(),
            area_id: None,
            verification_id: None,
        }
    }
}

impl GitHubConfig {
    pub fn is_enabled(&self) -> bool {
        self.mode.is_basic() || self.organization.is_some() || !self.repositories.is_empty()
    }

    pub fn normalize_repo(repo: &str) -> Result<String> {
        let repo = repo.trim();
        if repo.is_empty() {
            return Err(OrbitError::invalid_argument(
                "repository name must be non-empty",
            ));
        }
        let parts: Vec<_> = repo.split('/').collect();
        if parts.len() != 2 || parts[0].is_empty() || parts[1].is_empty() {
            return Err(OrbitError::invalid_argument(
                "repository must be `owner/name`",
            ));
        }
        Ok(format!("{}/{}", parts[0], parts[1]))
    }

    pub fn contains_repo(&self, repo: &str) -> bool {
        let normalized = repo.trim().to_lowercase();
        self.repositories
            .iter()
            .any(|linked| linked.trim().eq_ignore_ascii_case(&normalized))
    }

    pub fn link_repo(&mut self, repo: &str) -> Result<bool> {
        let repo = Self::normalize_repo(repo)?;
        if self.contains_repo(&repo) {
            return Ok(false);
        }
        self.repositories.push(repo);
        Ok(true)
    }

    pub fn unlink_repo(&mut self, repo: &str) -> Result<bool> {
        let repo = Self::normalize_repo(repo)?;
        let before = self.repositories.len();
        self.repositories
            .retain(|linked| !linked.eq_ignore_ascii_case(&repo));
        Ok(self.repositories.len() != before)
    }

    pub fn validate(&self) -> Result<()> {
        if self.mode.is_basic()
            && (self.organization.is_some()
                || self.issue_fields.area_id.is_some()
                || self.issue_fields.verification_id.is_some()
                || self.issue_fields.area_name != default_area_name()
                || self.issue_fields.verification_name != default_verification_name())
        {
            return Err(OrbitError::config(
                "github.mode = `basic` cannot be combined with an organization or Issue Fields configuration; use mode = `full` for organization Issue Fields",
            ));
        }
        for repo in &self.repositories {
            Self::normalize_repo(repo)?;
        }
        Ok(())
    }

    pub fn fields_configured(&self) -> bool {
        self.organization.is_some()
            && self.issue_fields.area_id.is_some()
            && self.issue_fields.verification_id.is_some()
    }

    pub fn require_organization(&self) -> Result<&str> {
        if self.mode.is_basic() {
            return Err(OrbitError::invalid_argument(
                "`orbit github setup` is unavailable in basic mode; use mode = `full` with organization Issue Fields",
            ));
        }
        self.organization.as_deref().ok_or_else(|| {
            OrbitError::new(
                crate::error::code::GITHUB_SETUP_INCOMPLETE,
                "github.organization is not set; run `orbit github setup` after configuring config.toml",
            )
            .with_details(serde_json::json!({
                "organization": null,
                "fields": [self.issue_fields.area_name, self.issue_fields.verification_name],
            }))
        })
    }

    pub fn require_linked_repo(&self, repo: &str) -> Result<()> {
        let repo = Self::normalize_repo(repo)?;
        if self.contains_repo(&repo) {
            Ok(())
        } else {
            Err(OrbitError::new(
                crate::error::code::GITHUB_REPO_NOT_LINKED,
                format!("repository `{repo}` is not linked to this Board"),
            )
            .with_details(serde_json::json!({ "repository": repo })))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalizes_and_links_repositories() {
        let mut config = GitHubConfig::default();
        assert!(config.link_repo("Acme/Widget").unwrap());
        assert!(!config.link_repo("acme/widget").unwrap());
        assert_eq!(config.repositories, vec!["Acme/Widget".to_string()]);
        assert!(config.unlink_repo("acme/widget").unwrap());
        assert!(config.repositories.is_empty());
    }

    #[test]
    fn rejects_invalid_repository_names() {
        assert_eq!(
            GitHubConfig::normalize_repo("not-a-repo").unwrap_err().code,
            crate::error::code::INVALID_ARGUMENT
        );
    }
}

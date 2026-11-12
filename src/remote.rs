//! Provider-neutral Remote Tracker configuration and port (ADR 0018).
//!
//! A Board connects to Remote Projects on one or more Remote Trackers. The
//! core owns only the project list and the tracker-neutral auto-push flag;
//! each tracker adapter keeps its own settings under `remote.<tracker>`.

use serde::{Deserialize, Serialize};

use crate::board::Board;
use crate::error::{OrbitError, Result};

pub mod auto_push;
pub mod catalog;
pub mod execution;
pub mod link;
pub mod model;
pub mod port;
pub mod relationships;
pub mod sync;

pub use crate::task::RemoteLink;

/// Remote Projects explicitly configured on the Board: `[remote] projects`
/// plus the legacy `[github] repositories` list, which names GitHub Remote
/// Projects.
pub fn configured_projects(board: &Board) -> Vec<RemoteProject> {
    let mut projects = board.config.remote.projects.clone();
    for repository in &board.config.github.repositories {
        let project = RemoteProject {
            tracker: "github".to_string(),
            project: repository.clone(),
        };
        if !projects.iter().any(|candidate| {
            candidate.tracker == project.tracker
                && candidate.project.eq_ignore_ascii_case(&project.project)
        }) {
            projects.push(project);
        }
    }
    projects
}

/// `[remote]` section of `config.toml`.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct RemoteConfig {
    /// Publish mapped Task changes to Remote Issues after a local mutation.
    #[serde(default, skip_serializing_if = "is_false")]
    pub auto_push: bool,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub projects: Vec<RemoteProject>,
    /// GitHub adapter settings carried over from `[github]` by `orbit migrate`.
    /// Adapter-specific; the sync port replaces it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub github: Option<crate::github::GitHubConfig>,
}

fn is_false(value: &bool) -> bool {
    !*value
}

impl RemoteConfig {
    pub fn is_empty(&self) -> bool {
        !self.auto_push
            && self.projects.is_empty()
            && self
                .github
                .as_ref()
                .is_none_or(|github| github == &crate::github::GitHubConfig::default())
    }

    pub fn validate(&self) -> Result<()> {
        for project in &self.projects {
            project.validate()?;
        }
        if let Some(github) = &self.github {
            let mut github = github.clone();
            github.repositories.clear();
            github.validate()?;
        }
        Ok(())
    }
}

/// One Remote Project: a container of Remote Issues on a Remote Tracker.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct RemoteProject {
    /// Remote Tracker adapter name, e.g. `github`.
    pub tracker: String,
    /// Remote Project identity in the tracker's own terms, e.g. `owner/repo`.
    pub project: String,
}

impl RemoteProject {
    pub fn validate(&self) -> Result<()> {
        if self.tracker.trim().is_empty() {
            return Err(OrbitError::config(
                "remote project tracker must be non-empty",
            ));
        }
        if self.project.trim().is_empty() {
            return Err(OrbitError::config("remote project id must be non-empty"));
        }
        Ok(())
    }
}

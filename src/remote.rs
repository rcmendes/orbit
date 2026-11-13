//! Provider-neutral Remote Tracker configuration and port (ADR 0018).
//!
//! A Board connects to Remote Projects on one or more Remote Trackers. The
//! core owns only the project list and the tracker-neutral auto-push flag;
//! each tracker adapter keeps its own settings under `remote.<tracker>`.

use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::board::Board;
use crate::error::{OrbitError, Result};
use crate::graph::{Graph, LoadMode};

pub mod auto_push;
pub mod catalog;
pub mod commands;
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
        push_project(&mut projects, "github", repository.clone());
    }
    projects
}

/// Append `project` to `projects` unless an entry on the same tracker with
/// a case-insensitive project match already exists.
fn push_project(projects: &mut Vec<RemoteProject>, tracker: &str, project: String) {
    if !projects.iter().any(|candidate| {
        candidate.tracker == tracker && candidate.project.eq_ignore_ascii_case(&project)
    }) {
        projects.push(RemoteProject {
            tracker: tracker.to_string(),
            project,
        });
    }
}

/// Every Remote Project connected to the Board: the configured projects
/// plus the Remote Projects implied by member Repository `origin`s (for
/// GitHub, member checkouts whose `origin` points at github.com). The one
/// rule every "connected projects" surface shares; deduplicated
/// case-insensitively on (tracker, project), configured projects first.
///
/// Member origins cost one `git remote get-url origin` per member, so
/// callers that only need the configured set use [`configured_projects`].
pub fn connected_projects(board: &Board) -> Result<Vec<RemoteProject>> {
    let mut projects = configured_projects(board);
    for repository in crate::repository::github_member_projects(board)? {
        push_project(&mut projects, "github", repository);
    }
    Ok(projects)
}

/// `owner/repo` names of the connected github Remote Projects, in
/// [`connected_projects`] order.
pub fn connected_github_projects(board: &Board) -> Result<Vec<String>> {
    Ok(connected_projects(board)?
        .into_iter()
        .filter(|project| project.tracker == "github")
        .map(|project| project.project)
        .collect())
}

/// Whether the Board has any Remote Tracker configuration at all: Remote
/// Projects, adapter settings, or the Auto-push policy. Task links alone do
/// not count — they are inert provenance on an unconfigured Board.
pub fn configured(board: &Board) -> bool {
    !configured_projects(board).is_empty()
        || board.config.remote.auto_push
        || board.config.github != crate::github::GitHubConfig::default()
        || board
            .config
            .remote
            .github
            .as_ref()
            .is_some_and(|github| *github != crate::github::GitHubConfig::default())
}

/// Whether human surfaces show Remote Tracker material for `board`: a Remote
/// Project is configured (`[remote] projects` or legacy `[github]
/// repositories`) or at least one Task carries a Remote link, active or
/// detached. Local-only Boards hide the `ISSUE` column, the `show` remote
/// line, and the `list` link filters from human output; `--json` is
/// unaffected.
pub fn details_visible(board: &Board, graph: &Graph) -> bool {
    !configured_projects(board).is_empty()
        || graph
            .tasks
            .values()
            .any(|task| link::link_view(task).is_some())
}

/// Resolve the Board for `dir` (`None` walks from the current directory) and
/// report whether Remote Tracker material is visible. Any resolution failure
/// reads as "not visible" so help text and setup templates degrade to the
/// local-only surface instead of guessing.
pub fn details_visible_for(dir: Option<&Path>) -> bool {
    let discovery = Board::discovery(dir);
    let Some(root) = discovery.root else {
        return false;
    };
    let Ok(board) = Board::at_root(root, discovery.warnings) else {
        return false;
    };
    let Ok((graph, _)) = Graph::load(&board, LoadMode::Lenient) else {
        return false;
    };
    details_visible(&board, &graph)
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::board::Config;

    fn board_with(config: Config) -> (tempfile::TempDir, Board) {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().to_path_buf();
        let board = Board {
            lock_path: root.join(".lock"),
            root,
            config,
            warnings: Vec::new(),
            orbit_dir: None,
            needs_migration: false,
            commit_failed: Default::default(),
        };
        (dir, board)
    }

    fn project(tracker: &str, project: &str) -> RemoteProject {
        RemoteProject {
            tracker: tracker.to_string(),
            project: project.to_string(),
        }
    }

    #[test]
    fn configured_projects_merges_remote_projects_and_legacy_github() {
        let mut config = Config::default();
        config.remote.projects = vec![
            project("github", "Acme/Widget"),
            project("jira", "PROJ-1"),
        ];
        config.github.repositories = vec![
            "acme/widget".to_string(),
            "acme/gadget".to_string(),
        ];
        let (_dir, board) = board_with(config);

        assert_eq!(
            configured_projects(&board),
            vec![
                project("github", "Acme/Widget"),
                project("jira", "PROJ-1"),
                project("github", "acme/gadget"),
            ]
        );
    }

    #[test]
    fn connected_projects_matches_configured_without_members() {
        let mut config = Config::default();
        config.remote.projects = vec![project("github", "Acme/Widget")];
        config.github.repositories = vec!["acme/gadget".to_string()];
        let (_dir, board) = board_with(config);

        assert_eq!(
            connected_projects(&board).unwrap(),
            configured_projects(&board)
        );
        assert_eq!(
            connected_github_projects(&board).unwrap(),
            vec!["Acme/Widget".to_string(), "acme/gadget".to_string()]
        );
    }
}

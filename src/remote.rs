//! Provider-neutral Remote Tracker configuration and port (ADR 0018).
//!
//! A Board connects to Remote Projects on one or more Remote Trackers. The
//! core owns only the project list and the tracker-neutral auto-push flag;
//! each tracker adapter keeps its own settings under `remote.<tracker>`.

use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::board::Board;
use crate::error::{OrbitError, Result};
use crate::github::config::GitHubConfig;
use crate::graph::Graph;

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
/// Projects. Entries deduplicate case-insensitively on (tracker, project),
/// first occurrence wins, so hand-edited config cannot list one Remote
/// Project twice.
pub fn configured_projects(board: &Board) -> Vec<RemoteProject> {
    let mut projects = Vec::with_capacity(
        board.config.remote.projects.len() + board.config.github.repositories.len(),
    );
    for project in &board.config.remote.projects {
        push_project(&mut projects, &project.tracker, project.project.clone());
    }
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
    remote_details_visible(board, || {
        graph
            .tasks
            .values()
            .any(|task| link::link_view(task).is_some())
    })
}

/// Resolve the Board for `dir` (`None` walks from the current directory) and
/// report whether Remote Tracker material is visible. Any resolution failure
/// reads as "not visible" so help text and setup templates degrade to the
/// local-only surface instead of guessing.
///
/// Unlike [`details_visible`] this does not load and validate the Task graph:
/// it probes each Task file's frontmatter for a `remote:`/`github:` link key,
/// which answers the same question cheaply enough to run before help renders.
pub fn details_visible_for(dir: Option<&Path>) -> bool {
    let discovery = Board::discovery(dir);
    let Some(root) = discovery.root else {
        return false;
    };
    let Ok(board) = Board::at_root(root, discovery.warnings) else {
        return false;
    };
    remote_details_visible(&board, || any_task_link(&board))
}

fn remote_details_visible(board: &Board, any_link: impl FnOnce() -> bool) -> bool {
    !configured_projects(board).is_empty() || any_link()
}

/// Cheap stand-in for the `graph.tasks.any(link_view)` half of
/// [`details_visible`]: true when any Task file's frontmatter carries a
/// `remote:` or legacy `github:` link, active or detached.
fn any_task_link(board: &Board) -> bool {
    let Ok(ids) = board.task_ids() else {
        return false;
    };
    ids.iter().any(|id| task_has_link(board, id))
}

/// Frontmatter fields probed for a Remote link. Only the two link keys are
/// deserialized: a link that fails to parse reads as "no link", matching a
/// corrupt Task dropped by a Lenient load, while unrelated malformed fields
/// cannot hide a valid link (the safe direction for help gating).
#[derive(Deserialize)]
struct LinkProbe {
    #[serde(default)]
    remote: Option<RemoteLink>,
    #[serde(default)]
    github: Option<crate::github::link::GitHubLink>,
}

fn task_has_link(board: &Board, id: &str) -> bool {
    let Ok(content) = std::fs::read_to_string(board.task_path(id)) else {
        return false;
    };
    let Some((frontmatter, _)) = crate::task::split_frontmatter(&content) else {
        return false;
    };
    serde_yaml_ng::from_str::<LinkProbe>(frontmatter)
        .is_ok_and(|probe| probe.remote.is_some() || probe.github.is_some())
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

/// Parse a Remote Project reference: `TRACKER:PROJECT` or a bare `PROJECT`
/// that resolves to the sole installed Remote Tracker.
pub(crate) fn parse_remote_project(input: &str) -> Result<(String, String)> {
    let input = input.trim();
    if input.is_empty() {
        return Err(OrbitError::invalid_argument(
            "Remote Project must not be empty",
        ));
    }
    if let Some((tracker, project)) = input.split_once(':') {
        let tracker = tracker.trim();
        let project = project.trim();
        if tracker.is_empty() || project.is_empty() {
            return Err(OrbitError::invalid_argument(format!(
                "invalid Remote Project `{input}`; expected `TRACKER:PROJECT`"
            )));
        }
        let project = if tracker == "github" {
            GitHubConfig::normalize_repo(project)?
        } else {
            project.to_string()
        };
        return Ok((tracker.to_string(), project));
    }
    let project = if input.contains('/') {
        GitHubConfig::normalize_repo(input)?
    } else {
        input.to_string()
    };
    Ok(("github".to_string(), project))
}

/// Parse a Remote Issue reference into its Remote Project and issue id.
/// Accepts `TRACKER:PROJECT#ID`, `PROJECT#ID`, or a Remote Tracker Issue
/// URL (GitHub: `https://github.com/OWNER/REPO/issues/NUMBER`).
pub(crate) fn parse_issue_ref(input: &str) -> Result<(RemoteProject, String)> {
    let input = input.trim();
    if input.contains("://") {
        let (project, id) = parse_issue_url(input)?;
        return Ok((
            RemoteProject {
                tracker: "github".to_string(),
                project,
            },
            id,
        ));
    }
    let invalid = || {
        OrbitError::invalid_argument(format!(
            "invalid Remote Issue `{input}`; expected `TRACKER:PROJECT#ID`, `PROJECT#ID`, or a Remote Tracker Issue URL"
        ))
    };
    // A `tracker:` prefix precedes the project; the issue id follows `#`.
    let (tracker, rest) = match input.split_once(':') {
        Some((tracker, rest)) if !tracker.contains(['/', '#']) => (Some(tracker.trim()), rest),
        _ => (None, input),
    };
    let (project_part, id) = rest.rsplit_once('#').ok_or_else(invalid)?;
    let id = id.trim();
    if id.is_empty() || !id.chars().all(|c| c.is_ascii_digit()) {
        return Err(invalid());
    }
    let (default_tracker, project) = parse_remote_project(project_part.trim())?;
    let tracker = tracker.unwrap_or(default_tracker.as_str()).to_string();
    Ok((RemoteProject { tracker, project }, id.to_string()))
}

fn parse_issue_url(url: &str) -> Result<(String, String)> {
    let invalid = || {
        OrbitError::invalid_argument(format!(
            "invalid GitHub Issue URL `{url}`; expected `https://github.com/OWNER/REPO/issues/NUMBER`"
        ))
    };
    let (scheme, address) = url.split_once("://").ok_or_else(invalid)?;
    if !scheme.eq_ignore_ascii_case("https") {
        return Err(invalid());
    }
    let (host, path) = address.split_once('/').ok_or_else(invalid)?;
    if !host.eq_ignore_ascii_case("github.com")
        || path
            .chars()
            .any(|character| character.is_whitespace() || matches!(character, '?' | '#' | '\\'))
    {
        return Err(invalid());
    }
    let parts: Vec<_> = path.split('/').collect();
    let parts = if parts.last() == Some(&"") {
        &parts[..parts.len() - 1]
    } else {
        &parts[..]
    };
    if parts.len() != 4 || parts[2] != "issues" {
        return Err(invalid());
    }
    let repo = GitHubConfig::normalize_repo(&format!("{}/{}", parts[0], parts[1]))
        .map_err(|_| invalid())?;
    let number = parts[3].parse::<u64>().map_err(|_| invalid())?;
    if number == 0 {
        return Err(invalid());
    }
    Ok((repo, number.to_string()))
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
        config.remote.projects = vec![project("github", "Acme/Widget"), project("jira", "PROJ-1")];
        config.github.repositories = vec!["acme/widget".to_string(), "acme/gadget".to_string()];
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
    fn configured_projects_deduplicates_case_insensitively() {
        let mut config = Config::default();
        config.remote.projects = vec![
            project("github", "Acme/Widget"),
            project("github", "acme/widget"),
            project("github", "ACME/WIDGET"),
            project("jira", "PROJ-1"),
        ];
        config.github.repositories = vec!["acme/widget".to_string(), "acme/gadget".to_string()];
        let (_dir, board) = board_with(config);

        // First occurrence wins; the legacy `[github] repositories` entry
        // matching a `[remote] projects` project adds nothing.
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
    fn connected_projects_includes_member_origins_deduplicated() {
        let dir = tempfile::tempdir().unwrap();
        let member = dir.path().join("member");
        crate::git::init_repo(&member).unwrap();
        crate::git::git_output(
            &member,
            &["remote", "add", "origin", "https://github.com/Acme/Widget.git"],
        )
        .unwrap();
        let other = dir.path().join("other");
        crate::git::init_repo(&other).unwrap();
        crate::git::git_output(
            &other,
            &["remote", "add", "origin", "git@github.com:acme/gadget.git"],
        )
        .unwrap();

        let mut config = Config::default();
        config.remote.projects = vec![project("github", "acme/widget")];
        config.repositories = vec![
            crate::repository::RepositoryConfig {
                name: "member".to_string(),
                path: member,
                remote_project: None,
            },
            crate::repository::RepositoryConfig {
                name: "other".to_string(),
                path: other,
                remote_project: None,
            },
        ];
        let (_dir, board) = board_with(config);

        // The first member's origin matches the configured project
        // case-insensitively and adds nothing; the second member's origin
        // joins the connected set after the configured projects.
        assert_eq!(
            connected_projects(&board).unwrap(),
            vec![
                project("github", "acme/widget"),
                project("github", "acme/gadget"),
            ]
        );
        assert_eq!(
            connected_github_projects(&board).unwrap(),
            vec!["acme/widget".to_string(), "acme/gadget".to_string()]
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

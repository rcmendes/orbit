//! Board host management (ADR 0019).
//!
//! A Board has no host (the default: its history never leaves the machine) or
//! exactly one host: a member Repository or another git URL whose remote stores
//! the Board's `orbit` branch. `board.host` keeps the user-facing spec — an
//! absolute path, a git URL, or a member Repository name resolved at sync time
//! — while the git remote `host` always mirrors the resolved URL.

use std::path::{Path, PathBuf};

use serde_json::json;

use crate::board::Board;
use crate::error::{OrbitError, Result, code};
use crate::git;
use crate::slug;

/// The git remote Orbit uses inside the Board repository for its host.
pub const HOST_REMOTE: &str = "host";

/// What `orbit board host` changed or found.
#[derive(Debug)]
pub struct HostOutcome {
    /// The spec stored in `board.host`, when one is configured.
    pub host: Option<String>,
    /// The resolved git URL/path, when it could be resolved.
    pub url: Option<String>,
    /// Whether an existing same-Slug host branch was adopted by `init`.
    pub joined: bool,
    /// The remote `orbit` tip observed while checking the host, if any.
    pub remote_tip: Option<String>,
}

/// How a `board.host` / `--host` value is interpreted.
#[derive(Debug, Clone, PartialEq, Eq)]
enum HostSpec {
    /// A local git repository, stored as an absolute path.
    Path(PathBuf),
    /// A git URL (`https://`, `git@`, `ssh://`, scp-like `host:path`, `file:`).
    Url(String),
    /// A member Repository name (`owner/repo` or a catalog name), resolved at
    /// sync time through the Workspace repository catalog.
    Name(String),
}

/// Classify a host spec without touching the network.
fn classify(spec: &str) -> Result<HostSpec> {
    let spec = spec.trim();
    if spec.is_empty() {
        return Err(OrbitError::invalid_argument(
            "the Board host must not be empty",
        ));
    }

    // An existing path wins over URL-looking syntax (`C:\…`, `./repo`).
    let candidate = if Path::new(spec).is_absolute() {
        PathBuf::from(spec)
    } else {
        std::env::current_dir()
            .map_err(|error| OrbitError::io("resolve current directory", error))?
            .join(spec)
    };
    if candidate.exists() {
        let canonical = dunce::canonicalize(&candidate).unwrap_or(candidate);
        if !git::is_git_repo(&canonical) {
            return Err(OrbitError::invalid_argument(format!(
                "host path {} is not a git repository",
                canonical.display()
            )));
        }
        return Ok(HostSpec::Path(canonical));
    }

    // git would read a leading `-` as an option, never as a URL or name.
    if spec.starts_with('-') {
        return Err(OrbitError::invalid_argument(format!(
            "invalid Board host `{spec}`; a host must not start with `-`"
        )));
    }
    if looks_like_url(spec) {
        return Ok(HostSpec::Url(spec.to_string()));
    }
    Ok(HostSpec::Name(spec.to_string()))
}

/// Conservative git-URL detection: scheme URLs, `git@host:…`, scp-like
/// `host.tld:path`, and `file:` URLs. A bare `owner/repo` stays a name.
fn looks_like_url(spec: &str) -> bool {
    if spec.contains("://") || spec.starts_with("git@") || spec.starts_with("file:") {
        return true;
    }
    let Some((host, _)) = spec.split_once(':') else {
        return false;
    };
    !host.is_empty()
        && !host.contains('/')
        && !host.contains('\\')
        && host.contains('.')
        && host
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '_'))
}

/// The value stored in `board.host` for a user-supplied spec: absolute paths
/// and URLs verbatim, member Repository names unchanged for later resolution.
pub fn normalize_host_spec(spec: &str) -> Result<String> {
    match classify(spec)? {
        HostSpec::Path(path) => Ok(path.to_string_lossy().to_string()),
        HostSpec::Url(url) | HostSpec::Name(url) => Ok(url),
    }
}

/// Resolve a stored `board.host` spec to the git URL `fetch`/`push` use.
///
/// Paths and URLs pass through. Names go through the member-Repository seam:
/// a Workspace Board's repository catalog maps a Repository name or `owner/repo`
/// identity to its `origin` (falling back to its local path). The
/// member-Repositories model lands with `orbit repo`; until then this is the
/// single resolution point.
pub fn resolve_host_url(board: &Board, spec: &str) -> Result<String> {
    match classify(spec)? {
        HostSpec::Path(path) => Ok(path.to_string_lossy().to_string()),
        HostSpec::Url(url) => Ok(url),
        HostSpec::Name(name) => member_repo_url(board, &name).ok_or_else(|| {
            OrbitError::invalid_argument(format!(
                "cannot resolve Board host `{name}`; pass a repository path or git URL"
            ))
        }),
    }
}

/// Look up a member Repository by name in the Workspace catalog.
fn member_repo_url(board: &Board, name: &str) -> Option<String> {
    if !board.is_workspace_board() {
        return None;
    }
    let workspace = crate::workspace::workspace_root(board).ok()?;
    let catalog = crate::workspace::catalog_for_diagnostics(&workspace).ok()?;
    let wanted = name.trim();
    catalog.repositories.values().find_map(|entry| {
        let by_identity = entry.repository.eq_ignore_ascii_case(wanted);
        let by_path = Path::new(&entry.path)
            .file_name()
            .and_then(|name| name.to_str())
            .is_some_and(|file| file.eq_ignore_ascii_case(wanted));
        if !(by_identity || by_path) {
            return None;
        }
        if !entry.origin.trim().is_empty() {
            Some(entry.origin.clone())
        } else {
            Some(entry.path.clone())
        }
    })
}

/// Points the Board repository's `host` remote at `url`, creating it when
/// needed. Local configuration only; remote branches are never touched.
pub fn ensure_remote(root: &Path, url: &str) -> Result<()> {
    git::remote_set_url(root, HOST_REMOTE, url)
}

/// The remote tip of the Board branch on the host, when it exists.
pub fn remote_tip(board: &Board, url: &str) -> Result<Option<String>> {
    let branch = board.config.board.branch.clone();
    let reference = format!("refs/heads/{branch}");
    git::ls_remote(&board.root, url, &reference)
}

/// Reads the Board config stored at `commit` (`FETCH_HEAD`-style ref or sha).
/// Returns `None` when the commit has no `config.toml`.
pub fn config_at(root: &Path, commit: &str) -> Result<Option<toml::Value>> {
    let Some(content) = git::show_file(root, commit, "config.toml")? else {
        return Ok(None);
    };
    toml::from_str(&content)
        .map(Some)
        .map_err(|error| OrbitError::config(format!("invalid config.toml at {commit}: {error}")))
}

/// The `id.prefix` slug a Board config declares, when present.
pub fn config_slug(config: &toml::Value) -> Option<String> {
    config
        .get("id")?
        .get("prefix")?
        .as_str()
        .map(slug::slug_of_prefix)
        .map(str::to_string)
}

/// Ensures one Board per host: when the remote already serves an `orbit`
/// branch it must hold a Board with the same Slug.
pub fn check_host_slug(root: &Path, tip: &str, board_slug: &str) -> Result<()> {
    let config = config_at(root, tip)?;
    let Some(config) = config else {
        return Err(OrbitError::new(
            code::BOARD_HOST_CONFLICT,
            "the host's `orbit` branch is not an Orbit Board (no config.toml)",
        ));
    };
    let host_slug = config_slug(&config);
    if host_slug.as_deref() != Some(board_slug) {
        return Err(OrbitError::new(
            code::BOARD_HOST_CONFLICT,
            format!(
                "the host already serves Board `{}`; this Board's Slug is `{board_slug}`",
                host_slug.as_deref().unwrap_or("<unknown>")
            ),
        )
        .with_details(json!({
            "host_slug": host_slug,
            "board_slug": board_slug,
        })));
    }
    Ok(())
}

/// Sets `board.host` on the Board at `root`, validating the host first.
///
/// `join` adopts an existing same-Slug host branch by resetting the fresh
/// Board onto it (`orbit init --host`); a populated Board only records the
/// host and leaves divergence to `orbit board sync`.
pub fn set_host(root: &Path, spec: &str, join: bool) -> Result<HostOutcome> {
    let board = Board::at_root(root.to_path_buf(), Vec::new())?;
    let stored = normalize_host_spec(spec)?;
    let url = resolve_host_url(&board, &stored)?;
    let branch = board.config.board.branch.clone();
    let board_slug = slug::slug_of_prefix(&board.config.id.prefix).to_string();

    ensure_remote(root, &url)?;
    let tip = git::ls_remote(root, HOST_REMOTE, &format!("refs/heads/{branch}"))?
        .map(|sha| {
            // Fetch the objects so the config check and a possible join can
            // read the remote tree.
            git::fetch_head(root, HOST_REMOTE, &branch).map(|_| sha)
        })
        .transpose()?;

    let mut joined = false;
    if let Some(tip) = &tip {
        check_host_slug(root, tip, &board_slug)?;
        if join {
            // The fresh Board has no history worth keeping; adopt the host's.
            git::reset_hard(root, tip)?;
            joined = true;
        }
    }

    let board = Board::at_root(root.to_path_buf(), Vec::new())?;
    board.save_config("orbit: set board host", |config| {
        config.board.host = Some(stored.clone());
        Ok(())
    })?;

    Ok(HostOutcome {
        host: Some(stored),
        url: Some(url),
        joined,
        remote_tip: tip,
    })
}

/// Clears `board.host` without deleting anything on the remote: the git
/// remote is removed locally and the remote `orbit` branch is left alone.
pub fn clear_host(root: &Path) -> Result<HostOutcome> {
    let board = Board::at_root(root.to_path_buf(), Vec::new())?;
    board.save_config("orbit: clear board host", |config| {
        config.board.host = None;
        Ok(())
    })?;
    git::remote_remove(root, HOST_REMOTE)?;
    Ok(HostOutcome {
        host: None,
        url: None,
        joined: false,
        remote_tip: None,
    })
}

/// Reports the configured host without changing anything.
pub fn show_host(board: &Board) -> Result<HostOutcome> {
    let host = board.config.board.host.clone();
    let url = host
        .as_deref()
        .and_then(|spec| resolve_host_url(board, spec).ok());
    Ok(HostOutcome {
        host,
        url,
        joined: false,
        remote_tip: None,
    })
}

/// `orbit board host` with no target: a read-only status view.
pub fn status_output(board: &Board) -> Result<crate::output::CommandOutput> {
    let outcome = show_host(board)?;
    let data = json!({
        "host": outcome.host,
        "url": outcome.url,
        "host_sync": board.config.board.host_sync.as_str(),
    });
    let human = match (&outcome.host, &outcome.url) {
        (Some(host), Some(url)) if host == url => {
            format!(
                "board host: {host}\nhost_sync: {}\n",
                board.config.board.host_sync.as_str()
            )
        }
        (Some(host), Some(url)) => format!(
            "board host: {host} ({url})\nhost_sync: {}\n",
            board.config.board.host_sync.as_str()
        ),
        (Some(host), None) => format!(
            "board host: {host} (unresolved; pass a repository path or git URL)\nhost_sync: {}\n",
            board.config.board.host_sync.as_str()
        ),
        (None, _) => format!(
            "board host: none\nhost_sync: {}\n",
            board.config.board.host_sync.as_str()
        ),
    };
    Ok(crate::output::CommandOutput::new(data, human))
}

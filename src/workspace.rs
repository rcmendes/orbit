//! Workspace compatibility shim.
//!
//! The Workspace/child-repository Board distinction is retired: a Board now
//! has member Repositories (see `crate::repository`). What remains here is
//! the `workspace.*` configuration block and the GitHub repository catalog
//! that legacy GitHub commands still read. The
//! catalog is refresh-free: `orbit repo refresh` owns member discovery.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::board::Board;
use crate::error::{OrbitError, Result};

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct WorkspaceConfig {
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub include: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub exclude: Vec<String>,
    /// Repositories detached locally without changing the user's include filter.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub detached: Vec<String>,
    /// Harness selection reused by later `workspace setup` reconciliations.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agent_harnesses: Option<Vec<String>>,
}

impl WorkspaceConfig {
    pub fn is_empty(&self) -> bool {
        self.include.is_empty()
            && self.exclude.is_empty()
            && self.detached.is_empty()
            && self.agent_harnesses.is_none()
    }

    pub fn validate(&self) -> Result<()> {
        for repository in self
            .include
            .iter()
            .chain(&self.exclude)
            .chain(&self.detached)
        {
            crate::github::GitHubConfig::normalize_repo(repository)?;
        }
        let included = self
            .include
            .iter()
            .map(|repo| repo.to_ascii_lowercase())
            .collect::<BTreeSet<_>>();
        if self
            .exclude
            .iter()
            .any(|repo| included.contains(&repo.to_ascii_lowercase()))
        {
            return Err(OrbitError::config(
                "workspace.include and workspace.exclude cannot contain the same repository",
            ));
        }
        let mut detached = BTreeSet::new();
        if self
            .detached
            .iter()
            .any(|repo| !detached.insert(repo.to_ascii_lowercase()))
        {
            return Err(OrbitError::config(
                "workspace.detached must not contain duplicate repositories",
            ));
        }
        if let Some(harnesses) = &self.agent_harnesses {
            let mut seen = BTreeSet::new();
            for harness in harnesses {
                if crate::setup::SetupClient::harness_from_name(harness).is_none() {
                    return Err(OrbitError::config(format!(
                        "unsupported Workspace agent harness `{harness}`"
                    )));
                }
                if !seen.insert(harness) {
                    return Err(OrbitError::config(format!(
                        "duplicate Workspace agent harness `{harness}`"
                    )));
                }
            }
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub(crate) struct RepositoryCatalog {
    #[serde(default)]
    pub(crate) organization: Option<String>,
    #[serde(default)]
    pub(crate) last_refreshed_at: Option<DateTime<Utc>>,
    #[serde(default)]
    pub(crate) repositories: BTreeMap<String, RepositoryEntry>,
    #[serde(default)]
    pub(crate) non_git_directories: Vec<String>,
    #[serde(default)]
    pub(crate) missing_origins: Vec<String>,
    #[serde(default)]
    pub(crate) unsupported_origins: Vec<UnsupportedOrigin>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct RepositoryEntry {
    pub(crate) repository: String,
    pub(crate) path: String,
    pub(crate) discovered: bool,
    pub(crate) status: RepositoryStatus,
    pub(crate) included: bool,
    #[serde(default)]
    pub(crate) cached: bool,
    pub(crate) origin: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) error: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) last_checked_at: Option<DateTime<Utc>>,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub(crate) enum RepositoryStatus {
    Included,
    Excluded,
    Inaccessible,
    Stale,
    Removed,
    UnsupportedOwner,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct UnsupportedOrigin {
    pub(crate) path: String,
    pub(crate) origin: String,
    pub(crate) reason: String,
}

pub(crate) fn workspace_root(board: &Board) -> Result<PathBuf> {
    workspace_root_if_board(board).ok_or_else(|| {
        OrbitError::invalid_argument(
            "this command requires a Board anchored outside Git at `<anchor>/.orbit/board`",
        )
    })
}

/// The anchor folder of a Board that lives outside every Git working tree —
/// the successor of the old Workspace root.
pub(crate) fn workspace_root_if_board(board: &Board) -> Option<PathBuf> {
    if !board.is_workspace_board() {
        return None;
    }
    // The anchor is the Orbit folder's parent; this also covers visible Board
    // folders anchored by the `.orbit/board` pointer file.
    let orbit = board.resolved_orbit_dir()?;
    let anchor = orbit.parent()?;
    Some(dunce::canonicalize(anchor).unwrap_or_else(|_| anchor.to_path_buf()))
}

pub(crate) fn catalog_path(workspace: &Path) -> PathBuf {
    workspace
        .join(".orbit")
        .join("cache")
        .join("repositories.json")
}

/// Return repositories available for issue creation from the catalog.
/// `None` means that this Board does not anchor member repositories.
pub(crate) fn export_repositories(board: &Board) -> Result<Option<Vec<String>>> {
    if !board.is_workspace_board() {
        return Ok(None);
    }
    let workspace = workspace_root(board)?;
    let catalog = load_catalog(&catalog_path(&workspace))?;
    Ok(Some(
        catalog
            .repositories
            .values()
            .filter(|entry| entry.included)
            .map(|entry| entry.repository.clone())
            .collect(),
    ))
}

pub(crate) fn catalog_for_diagnostics(workspace: &Path) -> Result<RepositoryCatalog> {
    load_catalog(&catalog_path(workspace))
}

fn load_catalog(path: &Path) -> Result<RepositoryCatalog> {
    match std::fs::read(path) {
        Ok(bytes) => serde_json::from_slice(&bytes)
            .map_err(|error| OrbitError::config(format!("invalid {}: {error}", path.display()))),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            Ok(RepositoryCatalog::default())
        }
        Err(error) => Err(OrbitError::io(format!("read {}", path.display()), error)),
    }
}


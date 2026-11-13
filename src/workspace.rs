//! Workspace compatibility shim.
//!
//! The Workspace/child-repository Board distinction is retired: a Board now
//! has member Repositories (see `crate::repository`). What remains here is
//! the `workspace.*` configuration block and the Board-anchor lookup the
//! Remote Issue catalog still uses. Member Repository discovery and origins
//! live in `crate::repository`; `orbit repo refresh` owns the inventory.

use std::collections::BTreeSet;
use std::path::PathBuf;

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

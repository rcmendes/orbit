//! The Remote Issue catalog (ADR 0018): a provider-neutral cache of Remote
//! Issues the Board knows about, keyed by `tracker/project/id`. Adapters
//! supply the summaries; core owns the cache.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::board::Board;
use crate::error::{OrbitError, Result};
use crate::remote::RemoteProject;
use crate::remote::model::RemoteIssueSummary;
use crate::remote::port::RemoteResolver;

/// One cached Remote Project on one tracker.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct CachedRemoteProject {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_refreshed_at: Option<DateTime<Utc>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    #[serde(default)]
    pub issues: Vec<RemoteIssueSummary>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct RemoteIssueCatalog {
    /// Key: `tracker` → project id → cached project.
    #[serde(default)]
    pub projects: BTreeMap<String, BTreeMap<String, CachedRemoteProject>>,
}

pub fn cache_path(workspace: &Path) -> PathBuf {
    workspace
        .join(".orbit")
        .join("cache")
        .join("remote-issues.json")
}

pub fn load(path: &Path) -> Result<RemoteIssueCatalog> {
    match std::fs::read(path) {
        Ok(bytes) => serde_json::from_slice(&bytes)
            .map_err(|error| OrbitError::config(format!("invalid {}: {error}", path.display()))),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            Ok(RemoteIssueCatalog::default())
        }
        Err(error) => Err(OrbitError::io(format!("read {}", path.display()), error)),
    }
}

pub fn save(path: &Path, catalog: &RemoteIssueCatalog) -> Result<()> {
    let parent = path
        .parent()
        .ok_or_else(|| OrbitError::config("Workspace cache has no parent directory"))?;
    std::fs::create_dir_all(parent)
        .map_err(|error| OrbitError::io(format!("create {}", parent.display()), error))?;
    let contents = serde_json::to_vec_pretty(catalog)
        .map_err(|error| OrbitError::config(format!("serialize Remote Issue catalog: {error}")))?;
    crate::atomic::write_atomic(path, &contents, true)
}

/// The cache key for a Remote Issue.
pub fn issue_key(project: &RemoteProject, id: &str) -> String {
    format!("{}:{}#{}", project.tracker, project.project, id)
}

/// Records a Remote Issue Orbit just created in the Workspace cache.
///
/// An explicit catalog refresh stays the source of truth for project
/// completeness and `last_refreshed_at`. This incremental upsert only
/// prevents the freshly linked Issue from looking orphaned to `doctor`
/// before the next refresh. Standalone Boards have no Workspace cache, so
/// this is a no-op there.
pub fn remember_created_issue(
    board: &Board,
    project: &RemoteProject,
    issue: &crate::remote::model::RemoteIssue,
) -> Result<()> {
    if !board.is_workspace_board() {
        return Ok(());
    }
    let workspace = crate::workspace::workspace_root(board)?;
    let path = cache_path(&workspace);
    let mut catalog = load(&path)?;
    let entry = catalog
        .projects
        .entry(project.tracker.clone())
        .or_default()
        .entry(project.project.clone())
        .or_default();
    entry.issues.retain(|cached| cached.id != issue.id);
    entry.issues.push(RemoteIssueSummary {
        project: project.project.clone(),
        id: issue.id.clone(),
        url: issue.url.clone(),
        title: issue.title.clone(),
        status: issue.status,
        labels: issue.labels.clone(),
        area: issue.area.clone(),
        verification: issue.verification.clone(),
        updated_at: issue.updated_at,
        fields: BTreeMap::new(),
    });
    entry
        .issues
        .sort_by(|a, b| a.id.cmp(&b.id).then_with(|| a.project.cmp(&b.project)));
    save(&path, &catalog)
}

/// Refresh one Remote Project's catalog entries through its adapter.
pub fn refresh_project<R: RemoteResolver + ?Sized>(
    board: &Board,
    resolver: &R,
    workspace: &Path,
    project: &RemoteProject,
) -> Result<usize> {
    let adapter = resolver.adapter(&project.tracker)?;
    let issues = adapter.list_issues(project)?;
    let path = cache_path(workspace);
    let mut catalog = load(&path)?;
    let entry = catalog
        .projects
        .entry(project.tracker.clone())
        .or_default()
        .entry(project.project.clone())
        .or_default();
    entry.issues = issues;
    entry.last_refreshed_at = Some(Utc::now());
    entry.error = None;
    let count = entry.issues.len();
    save(&path, &catalog)?;
    let _ = board;
    Ok(count)
}

//! Member Repositories of a Board.
//!
//! A Board has 0..N member Repositories: Git repositories whose work belongs
//! to the Board. Children are discovered by walking down from the folder that
//! holds the Board's Orbit folder; `orbit repo add` records Repositories that
//! live elsewhere. A Task's optional `repo` field names one member.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::board::{self, Board};
use crate::error::{OrbitError, Result, code};
use crate::git;
use crate::task;

/// One explicitly configured member Repository in `config.toml`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct RepositoryConfig {
    /// Stable Repository name: the Task `repo` value and `orbit repo` selector.
    pub name: String,
    /// Absolute path of the Repository's main checkout.
    pub path: PathBuf,
}

impl RepositoryConfig {
    pub fn validate(&self) -> Result<()> {
        if self.name.trim().is_empty() || !task::is_valid_repo(&self.name) {
            return Err(OrbitError::config(format!(
                "repositories entry `{}` is not a valid Repository name",
                self.name
            )));
        }
        if !self.path.is_absolute() {
            return Err(OrbitError::config(format!(
                "repositories entry `{}` must record an absolute path",
                self.name
            )));
        }
        Ok(())
    }
}

/// Where a member Repository came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RepositoryKind {
    /// Found by walking down from the Board's folder.
    Child,
    /// Added explicitly with `orbit repo add`.
    Explicit,
}

impl RepositoryKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Child => "child",
            Self::Explicit => "explicit",
        }
    }
}

/// A resolved member Repository.
#[derive(Debug, Clone)]
pub struct Repository {
    pub name: String,
    /// Canonical absolute path of the Repository's main checkout. Every
    /// worktree of the Repository resolves to this same path.
    pub path: PathBuf,
    pub kind: RepositoryKind,
}

/// Canonical form of a path that may not exist: the nearest existing ancestor
/// is resolved and the rest is appended.
pub fn canonical_lenient(path: &Path) -> PathBuf {
    let mut tail = Vec::new();
    let mut current = path;
    while !current.exists() {
        let (Some(parent), Some(name)) = (current.parent(), current.file_name()) else {
            return path.to_path_buf();
        };
        tail.push(name);
        current = parent;
    }
    let mut resolved = dunce::canonicalize(current).unwrap_or_else(|_| current.to_path_buf());
    resolved.extend(tail.into_iter().rev());
    resolved
}

/// The main checkout of the Git repository containing `path`, canonicalized.
/// A worktree resolves to its main checkout's Repository.
pub fn main_checkout_of(path: &Path) -> Result<Option<PathBuf>> {
    let Some(toplevel) = git::toplevel_containing(path) else {
        return Ok(None);
    };
    let toplevel = dunce::canonicalize(&toplevel).unwrap_or(toplevel);
    let common = git::common_dir_at(&toplevel)?;
    let main = board::main_checkout_for(&toplevel, &common);
    Ok(Some(canonical_lenient(&main)))
}

/// The Repository name for `main` when it is a member of `board` and its name
/// is a valid Task `repo` value.
pub fn member_name(board: &Board, main: &Path) -> Option<String> {
    let main = canonical_lenient(main);
    members(board)
        .ok()?
        .into_iter()
        .find(|member| member.path == main)
        .map(|member| member.name)
        .filter(|name| task::is_valid_repo(name))
}

/// The member Repository name for the current working directory, when the
/// cwd sits inside a member Repository. `orbit add` records it as the Task's
/// `repo` field.
pub fn current_member_name(board: &Board) -> Result<Option<String>> {
    let cwd = std::env::current_dir()
        .map_err(|error| OrbitError::io("resolve current directory", error))?;
    let Some(main) = main_checkout_of(&cwd)? else {
        return Ok(None);
    };
    // The Board's own repository is not a member Repository.
    if main == canonical_lenient(&board.root) {
        return Ok(None);
    }
    Ok(member_name(board, &main))
}

/// All member Repositories of the Board: discovered children merged with the
/// explicitly configured ones, deduplicated by main-checkout path. An explicit
/// entry that matches a child upgrades it and supplies the stable name.
pub fn members(board: &Board) -> Result<Vec<Repository>> {
    let mut by_path: BTreeMap<PathBuf, Repository> = BTreeMap::new();
    if let Some(anchor) = board.anchor_dir() {
        for repository in child_repositories(&anchor, board)? {
            by_path.insert(repository.path.clone(), repository);
        }
    }
    for entry in &board.config.repositories {
        let path = canonical_lenient(&entry.path);
        match by_path.get_mut(&path) {
            Some(existing) => {
                existing.kind = RepositoryKind::Explicit;
                existing.name = entry.name.clone();
            }
            None => {
                by_path.insert(
                    path.clone(),
                    Repository {
                        name: entry.name.clone(),
                        path,
                        kind: RepositoryKind::Explicit,
                    },
                );
            }
        }
    }
    Ok(by_path.into_values().collect())
}

/// Repositories found by walking down from `anchor`, the folder holding the
/// Board's Orbit folder. The anchor itself counts when it is a Git working
/// tree. Repositories are not descended into; a repository that anchors its
/// own Board or links to a different Board is not a member.
pub fn child_repositories(anchor: &Path, board: &Board) -> Result<Vec<Repository>> {
    let board_root = canonical_lenient(&board.root);
    let mut repositories = Vec::new();
    let mut seen = std::collections::BTreeSet::new();
    let mut pending = vec![anchor.to_path_buf()];
    while let Some(directory) = pending.pop() {
        if let Some(main) = repository_main(&directory)? {
            if main != board_root
                && !Board::is_board(&directory)
                && is_member(&directory, &main, board)?
                && seen.insert(main.clone())
            {
                repositories.push(Repository {
                    name: repository_name(&main),
                    path: main,
                    kind: RepositoryKind::Child,
                });
            }
            continue;
        }
        for path in git::child_directories(&directory, false)? {
            let path = path?;
            let name = path.file_name();
            if name == Some(std::ffi::OsStr::new(board::ORBIT_DIR))
                || name == Some(std::ffi::OsStr::new(".git"))
                || name == Some(std::ffi::OsStr::new(board::LEGACY_BOARD_DIR))
            {
                continue;
            }
            pending.push(path);
        }
    }
    repositories.sort_by(|left, right| left.path.cmp(&right.path));
    Ok(repositories)
}

/// The main checkout when `path` is a Git working-tree root (`.git` present,
/// covering linked worktrees), else `None`.
fn repository_main(path: &Path) -> Result<Option<PathBuf>> {
    if !path.join(".git").exists() {
        return Ok(None);
    }
    main_checkout_of(path)
}

/// A child repository stays out of the member set when it anchors its own
/// Board or its link pointer resolves to a different Board.
fn is_member(path: &Path, main: &Path, board: &Board) -> Result<bool> {
    if board::resolve_orbit_dir(&path.join(board::ORBIT_DIR)).is_some() {
        return Ok(false);
    }
    if let Some(toplevel) = git::toplevel_containing(main)
        && let Ok(common) = git::common_dir_at(&toplevel)
        && let Ok(Some(target)) = board::read_link_file(&common.join("orbit").join("link"))
        && let Ok((linked, _)) = board::classify_link_target(&target)
        && canonical_lenient(&linked) != canonical_lenient(&board.root)
    {
        return Ok(false);
    }
    Ok(true)
}

fn repository_name(main: &Path) -> String {
    main.file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_else(|| "repository".to_string())
}

/// The rebuildable Repository inventory written by `orbit repo refresh`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RepositoryInventory {
    pub refreshed_at: DateTime<Utc>,
    #[serde(default)]
    pub repositories: Vec<RepositoryRecord>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RepositoryRecord {
    pub name: String,
    pub path: PathBuf,
    pub kind: RepositoryKind,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub origin: Option<String>,
}

pub fn inventory_path(board: &Board) -> PathBuf {
    // `repositories.json` belongs to the GitHub workspace catalog; the member
    // inventory gets its own name so the two never clobber each other.
    board::board_cache_dir(board).join("member-repositories.json")
}

pub fn load_inventory(board: &Board) -> Result<Option<RepositoryInventory>> {
    let path = inventory_path(board);
    match std::fs::read(&path) {
        Ok(bytes) => serde_json::from_slice(&bytes)
            .map(Some)
            .map_err(|error| OrbitError::config(format!("invalid {}: {error}", path.display()))),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(OrbitError::io(format!("read {}", path.display()), error)),
    }
}

pub fn save_inventory(board: &Board, inventory: &RepositoryInventory) -> Result<()> {
    let path = inventory_path(board);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|error| OrbitError::io(format!("create {}", parent.display()), error))?;
    }
    let bytes = serde_json::to_vec_pretty(inventory)
        .map_err(|error| OrbitError::config(format!("serialize repository inventory: {error}")))?;
    crate::atomic::write_atomic(&path, &bytes, true)
}

/// Best-effort `origin` remote of a member checkout.
pub fn origin_of(path: &Path) -> Option<String> {
    git::git_output(path, &["remote", "get-url", "origin"])
        .ok()
        .filter(|origin| !origin.is_empty())
}

/// Error used when `orbit repo remove` names a discovered child, which cannot
/// be removed because membership comes from the folder layout.
pub fn child_not_removable(name: &str) -> OrbitError {
    OrbitError::new(
        code::INVALID_ARGUMENT,
        format!(
            "`{name}` is a child Repository discovered under the Board's folder; it cannot be removed — move the Repository or give it its own Board with `orbit init`"
        ),
    )
}

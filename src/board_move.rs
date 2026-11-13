//! `orbit board move DIR`: relocate a Board folder (ADR 0019 layout).
//!
//! The Board folder is a standalone Git repository, so moving it is one
//! directory rename. Everything that names the folder is then repointed: the
//! anchor's `.orbit/board` pointer file (or the hidden layout when DIR is
//! `.orbit/board`), the `.git/info/exclude` entry hiding a visible folder, and
//! explicit member Repository link pointers.

use std::path::{Path, PathBuf};

use crate::board::{self, BOARD_DIR, Board};
use crate::error::{OrbitError, Result};
use crate::git;

/// What `orbit board move` changed.
#[derive(Debug)]
pub struct MoveOutcome {
    pub from: PathBuf,
    pub to: PathBuf,
    /// Whether the Board is now a visible folder named by a pointer file.
    pub visible: bool,
    /// Member Repositories whose link pointer was rewritten.
    pub relinked: Vec<PathBuf>,
}

pub fn move_board(board: &Board, target: &Path) -> Result<MoveOutcome> {
    let orbit = board.resolved_orbit_dir().ok_or_else(|| {
        OrbitError::invalid_argument(
            "this Board has no `.orbit` anchor to repoint; move it by hand and run `orbit repo add` from each member",
        )
    })?;
    let orbit = dunce::canonicalize(&orbit).unwrap_or(orbit);
    let from = dunce::canonicalize(&board.root).unwrap_or_else(|_| board.root.clone());
    let to = absolute_target(target)?;
    let hidden = orbit.join(BOARD_DIR);
    let visible = to != hidden;

    if to == from {
        return Err(OrbitError::invalid_argument(format!(
            "the Board already lives at {}",
            from.display()
        )));
    }
    if to.starts_with(&from) {
        return Err(OrbitError::invalid_argument(
            "cannot move the Board into its own folder",
        ));
    }
    // The hidden path may hold the pointer file this move replaces.
    let replaceable = if visible {
        is_empty_dir(&to)
    } else {
        to.is_file()
    };
    if to.exists() && !replaceable {
        return Err(OrbitError::invalid_argument(format!(
            "{} already exists; pass a new or empty directory",
            to.display()
        )));
    }

    // Captured before the move: the folder is still where git expects it.
    let old_exclusion = if from == hidden {
        None
    } else {
        board::worktree_exclusion(&from)?
    };
    let links = linked_members(board, &from)?;

    if to.is_dir() {
        std::fs::remove_dir(&to)
            .map_err(|error| OrbitError::io(format!("remove {}", to.display()), error))?;
    }
    if let Some(parent) = to.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|error| OrbitError::io(format!("create {}", parent.display()), error))?;
    }
    // A visible Board leaves a pointer file where the hidden folder goes.
    if !visible && hidden.is_file() {
        std::fs::remove_file(&hidden)
            .map_err(|error| OrbitError::io(format!("remove {}", hidden.display()), error))?;
    }
    if let Err(error) = std::fs::rename(&from, &to) {
        if !visible {
            let _ = board::write_pointer_file(&orbit, &from);
        }
        return Err(OrbitError::io(
            format!(
                "move {} to {} (the target must be on the same filesystem)",
                from.display(),
                to.display()
            ),
            error,
        ));
    }
    if visible && let Err(error) = board::write_pointer_file(&orbit, &to) {
        // Keep the Board discoverable: undo the rename.
        let _ = std::fs::rename(&to, &from);
        return Err(error);
    }

    if let Some((repo, entry)) = old_exclusion {
        git::remove_exclude_entry(&repo, &entry)?;
    }
    board::ensure_orbit_excluded(&orbit, visible.then_some(to.as_path()))?;

    let mut relinked = Vec::new();
    for (common, member) in links {
        Board::write_link_at(&common, &to)?;
        relinked.push(member);
    }

    Ok(MoveOutcome {
        from,
        to,
        visible,
        relinked,
    })
}

/// Resolves DIR against the current directory, canonicalizing the deepest
/// existing ancestor so comparisons survive symlinks (`/var` → `/private/var`).
fn absolute_target(target: &Path) -> Result<PathBuf> {
    let absolute = if target.is_absolute() {
        target.to_path_buf()
    } else {
        std::env::current_dir()
            .map_err(|error| OrbitError::io("resolve current directory", error))?
            .join(target)
    };
    let mut existing = absolute.as_path();
    let mut rest = Vec::new();
    while !existing.exists() {
        let (Some(parent), Some(name)) = (existing.parent(), existing.file_name()) else {
            break;
        };
        rest.push(name.to_os_string());
        existing = parent;
    }
    let mut resolved = dunce::canonicalize(existing).unwrap_or_else(|_| existing.to_path_buf());
    for name in rest.into_iter().rev() {
        if name == ".." || name == "." {
            return Err(OrbitError::invalid_argument(format!(
                "invalid Board folder {}; use a plain path",
                target.display()
            )));
        }
        resolved.push(name);
    }
    Ok(resolved)
}

fn is_empty_dir(path: &Path) -> bool {
    std::fs::read_dir(path).is_ok_and(|mut entries| entries.next().is_none())
}

/// Explicit members whose `.git/orbit/link` names the Board at `from`, as
/// `(common_dir, repository)` pairs.
fn linked_members(board: &Board, from: &Path) -> Result<Vec<(PathBuf, PathBuf)>> {
    let mut found = Vec::new();
    for entry in &board.config.repositories {
        let Some(toplevel) = git::toplevel_containing(&entry.path) else {
            continue;
        };
        let Ok(common) = git::common_dir_at(&toplevel) else {
            continue;
        };
        let Ok(Some(target)) = board::read_link_file(&common.join("orbit").join("link")) else {
            continue;
        };
        let target = dunce::canonicalize(&target).unwrap_or(target);
        if target == from {
            found.push((common, entry.path.clone()));
        }
    }
    Ok(found)
}

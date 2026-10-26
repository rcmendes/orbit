use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use serde::Serialize;

use crate::board::{Board, DiscoverySource, RepoContext, StorageMode};
use crate::error::{OrbitError, Result, code};
use crate::git;
use crate::graph::{Graph, LoadMode};

#[derive(Debug, Default, Serialize)]
pub struct MigrationReport {
    pub migrated: bool,
    pub resumed: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub from: Option<PathBuf>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub to: Option<PathBuf>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub storage: Option<String>,
    pub tasks: usize,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub pointer: Option<PathBuf>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub removed: Vec<PathBuf>,
    /// Follow-up the migration cannot do itself, such as other Repositories
    /// still linked to the old path.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub notes: Vec<String>,
}

fn workspace_notes(old: &Path, new: &Path) -> Vec<String> {
    vec![format!(
        "other Repositories linked to {} now fail with LINK_NOT_FOUND; run `orbit link {}` in each",
        old.display(),
        new.display()
    )]
}

impl MigrationReport {
    pub fn render_human(&self) -> String {
        if !self.migrated {
            return self
                .reason
                .as_ref()
                .map(|reason| format!("migration: {reason}\n"))
                .unwrap_or_default();
        }
        let from = self
            .from
            .as_ref()
            .map(|path| path.display().to_string())
            .unwrap_or_else(|| "legacy layout".to_string());
        let to = self
            .to
            .as_ref()
            .map(|path| path.display().to_string())
            .unwrap_or_default();
        let mut out = format!(
            "migrated Board from {from} to {to} ({} tasks{})\n",
            self.tasks,
            if self.resumed { ", resumed" } else { "" }
        );
        for note in &self.notes {
            out.push_str(&format!("note: {note}\n"));
        }
        out
    }
}

pub fn migrate(dir: Option<&Path>) -> Result<MigrationReport> {
    let discovery = Board::discovery(dir);
    if let Some(error) = &discovery.error
        && error.code != code::BOARD_NOT_FOUND
    {
        return Err(error.clone());
    }
    let Some(root) = discovery.root.clone() else {
        return Ok(no_legacy());
    };
    let board = Board::at_root(root.clone(), discovery.warnings.clone())?;
    let context = Board::repo_context_at(&root)
        .ok()
        .flatten()
        .or_else(Board::repo_context);

    if let Some((new_root, legacy_root)) = discovery.ambiguous.clone() {
        return finish_ambiguous(&new_root, &legacy_root, &board, context.as_ref());
    }

    let is_legacy_local = discovery.source == DiscoverySource::LegacyLocal
        || board.config.board.storage.is_none()
            && context
                .as_ref()
                .is_some_and(|context| context.legacy_local_board == root);

    if is_legacy_local {
        let context = context.clone().ok_or_else(|| {
            OrbitError::config("a legacy local Board must be migrated from inside its repository")
        })?;
        return migrate_local(&context);
    }

    let is_legacy_workspace = discovery.legacy.is_some()
        && discovery.source == DiscoverySource::Link
        && root.file_name().and_then(|name| name.to_str()) == Some("board");
    if is_legacy_workspace {
        return migrate_workspace(&board);
    }

    if board.config.board.storage.is_none() {
        return keep_existing(&board, context.as_ref());
    }

    Ok(no_legacy())
}

fn migrate_local(context: &RepoContext) -> Result<MigrationReport> {
    let old = context.legacy_local_board.clone();
    let new = context.main_checkout.join(".orbit");

    if Board::is_board(&new) {
        if Board::is_board(&old) {
            verify_same_board(&old, &new)?;
            remove_legacy(&old, Some(context))?;
        }
        let (tasks, _) = finalize(&new, StorageMode::Worktree)?;
        let pointer = update_pointer(&old, &new)?;
        return Ok(MigrationReport {
            migrated: true,
            resumed: true,
            from: Some(old.clone()),
            to: Some(new),
            storage: Some(StorageMode::Worktree.as_str().to_string()),
            tasks,
            pointer,
            removed: vec![old],
            ..MigrationReport::default()
        });
    }

    if !Board::is_board(&old) {
        return Ok(no_legacy());
    }
    if new.exists() {
        return Err(OrbitError::config(format!(
            "{} already exists and is not an Orbit Board; move it before migrating",
            new.display()
        )));
    }

    git::worktree_move(&context.repo_root, &old, &new)?;
    let (tasks, _) = finalize(&new, StorageMode::Worktree)?;
    let pointer = update_pointer(&old, &new)?;
    Ok(MigrationReport {
        migrated: true,
        from: Some(old),
        to: Some(new),
        storage: Some(StorageMode::Worktree.as_str().to_string()),
        tasks,
        pointer,
        ..MigrationReport::default()
    })
}

fn migrate_workspace(board: &Board) -> Result<MigrationReport> {
    let old = board.root.clone();
    let new = old
        .parent()
        .ok_or_else(|| OrbitError::config("legacy workspace Board has no parent directory"))?
        .join(".orbit");

    let pointer_target = pointer_needs_update(&old)?;

    if Board::is_board(&new) {
        verify_same_board(&old, &new)?;
        remove_legacy(&old, None)?;
        let (tasks, _) = finalize(&new, StorageMode::Repository)?;
        if pointer_target.is_some() {
            Board::write_link(&new)?;
        }
        let pointer = pointer_target.map(|_| new.clone());
        let notes = workspace_notes(&old, &new);
        return Ok(MigrationReport {
            migrated: true,
            resumed: true,
            from: Some(old.clone()),
            to: Some(new),
            storage: Some(StorageMode::Repository.as_str().to_string()),
            tasks,
            pointer,
            notes,
            removed: vec![old],
            ..MigrationReport::default()
        });
    }

    if new.exists() {
        return Err(OrbitError::config(format!(
            "{} already exists and is not an Orbit Board; move it before migrating",
            new.display()
        )));
    }
    if let Some(parent) = new.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|error| OrbitError::io(format!("create {}", parent.display()), error))?;
    }
    std::fs::rename(&old, &new).map_err(|error| {
        OrbitError::io(
            format!("move Board from {} to {}", old.display(), new.display()),
            error,
        )
    })?;
    let (tasks, _) = finalize(&new, StorageMode::Repository)?;
    if pointer_target.is_some() {
        Board::write_link(&new)?;
    }
    let pointer = pointer_target.map(|_| new.clone());
    let notes = workspace_notes(&old, &new);
    Ok(MigrationReport {
        migrated: true,
        from: Some(old),
        to: Some(new),
        storage: Some(StorageMode::Repository.as_str().to_string()),
        tasks,
        pointer,
        notes,
        ..MigrationReport::default()
    })
}

fn keep_existing(board: &Board, context: Option<&RepoContext>) -> Result<MigrationReport> {
    let storage = if context.is_some() {
        StorageMode::Worktree
    } else {
        StorageMode::detect(&board.root)
    };
    if storage == StorageMode::Worktree {
        Board::ensure_excluded(&board.root)?;
    }
    let config = Board::at_root(board.root.clone(), Vec::new())?;
    config.set_storage_mode(storage)?;
    let graph = Graph::load(&config, LoadMode::Strict)?.0;
    Ok(MigrationReport {
        migrated: true,
        from: Some(board.root.clone()),
        to: Some(board.root.clone()),
        storage: Some(storage.as_str().to_string()),
        tasks: graph.tasks.len(),
        ..MigrationReport::default()
    })
}

fn finish_ambiguous(
    new: &Path,
    legacy: &Path,
    board: &Board,
    context: Option<&RepoContext>,
) -> Result<MigrationReport> {
    verify_same_board(legacy, new)?;
    let pointer_target = pointer_needs_update(legacy)?;
    remove_legacy(legacy, context)?;
    let storage = board.storage_mode();
    let (tasks, _) = finalize(new, storage)?;
    if pointer_target.is_some() {
        Board::write_link(new)?;
    }
    let pointer = pointer_target.map(|_| new.to_path_buf());
    Ok(MigrationReport {
        migrated: true,
        resumed: true,
        from: Some(legacy.to_path_buf()),
        to: Some(new.to_path_buf()),
        storage: Some(storage.as_str().to_string()),
        tasks,
        pointer,
        removed: vec![legacy.to_path_buf()],
        ..MigrationReport::default()
    })
}

fn finalize(root: &Path, storage: StorageMode) -> Result<(usize, Board)> {
    if storage == StorageMode::Worktree {
        Board::ensure_excluded(root)?;
    }
    let board = Board::at_root(root.to_path_buf(), Vec::new())?;
    board.set_storage_mode(storage)?;
    let graph = Graph::load(&board, LoadMode::Strict)?.0;
    Ok((graph.tasks.len(), board))
}

fn pointer_needs_update(old: &Path) -> Result<Option<PathBuf>> {
    let Some(link) = Board::read_link()? else {
        return Ok(None);
    };
    let canonical = dunce::canonicalize(&link).unwrap_or_else(|_| link.clone());
    if canonical == old || canonical.join("board") == old {
        Ok(Some(link))
    } else {
        Ok(None)
    }
}

fn update_pointer(old: &Path, new: &Path) -> Result<Option<PathBuf>> {
    if pointer_needs_update(old)?.is_some() {
        Board::write_link(new)?;
        Ok(Some(new.to_path_buf()))
    } else {
        Ok(None)
    }
}

fn remove_legacy(path: &Path, context: Option<&RepoContext>) -> Result<()> {
    if !path.exists() {
        return Ok(());
    }
    if let Some(context) = context
        && git::worktree_remove(&context.repo_root, path).is_ok()
    {
        return Ok(());
    }
    // `git worktree remove` refuses a dirty or locked worktree. Wiping it
    // anyway would lose uncommitted Task edits, so only a plain directory (no
    // `.git`) is removed here.
    if path.join(".git").exists() {
        return Err(OrbitError::new(
            code::MIGRATION_BLOCKED,
            format!(
                "{} is a git worktree that could not be removed cleanly (uncommitted changes?); commit or discard them, then re-run doctor --migrate",
                path.display()
            ),
        ));
    }
    std::fs::remove_dir_all(path)
        .map_err(|error| OrbitError::io(format!("remove {}", path.display()), error))
}

fn verify_same_board(old: &Path, new: &Path) -> Result<()> {
    let old_board = Board::at_root(old.to_path_buf(), Vec::new())?;
    let new_board = Board::at_root(new.to_path_buf(), Vec::new())?;
    if old_board.config.id.prefix != new_board.config.id.prefix {
        return Err(OrbitError::new(
            code::MIGRATION_BLOCKED,
            format!(
                "{} and {} use different Task-ID prefixes; refusing to remove either Board",
                old.display(),
                new.display()
            ),
        ));
    }
    let old_ids = task_ids(&old_board)?;
    let new_ids = task_ids(&new_board)?;
    if old_ids != new_ids {
        return Err(OrbitError::new(
            code::MIGRATION_BLOCKED,
            format!(
                "{} and {} contain different Tasks; refusing to remove either Board",
                old.display(),
                new.display()
            ),
        ));
    }
    for dir in ["tasks", "comments"] {
        if dir_contents(&old.join(dir))? != dir_contents(&new.join(dir))? {
            return Err(OrbitError::new(
                code::MIGRATION_BLOCKED,
                format!(
                    "{} and {} have diverged ({dir}/ differs); refusing to remove either Board",
                    old.display(),
                    new.display()
                ),
            ));
        }
    }
    Ok(())
}

/// Relative path -> bytes for every Markdown file under `dir`, so two Boards compare by
/// content and not only by Task ID.
fn dir_contents(dir: &Path) -> Result<std::collections::BTreeMap<PathBuf, Vec<u8>>> {
    let mut files = std::collections::BTreeMap::new();
    let mut pending = vec![dir.to_path_buf()];
    while let Some(current) = pending.pop() {
        let entries = match std::fs::read_dir(&current) {
            Ok(entries) => entries,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => continue,
            Err(error) => {
                return Err(OrbitError::io(format!("read {}", current.display()), error));
            }
        };
        for entry in entries {
            let path = entry
                .map_err(|error| OrbitError::io(format!("read {}", current.display()), error))?
                .path();
            if path.is_dir() {
                pending.push(path);
            } else if path.extension().is_some_and(|ext| ext == "md") {
                let bytes = std::fs::read(&path)
                    .map_err(|error| OrbitError::io(format!("read {}", path.display()), error))?;
                files.insert(path.strip_prefix(dir).unwrap_or(&path).to_path_buf(), bytes);
            }
        }
    }
    Ok(files)
}

fn task_ids(board: &Board) -> Result<BTreeSet<String>> {
    Ok(Graph::load(board, LoadMode::Strict)?
        .0
        .tasks
        .keys()
        .cloned()
        .collect())
}

fn no_legacy() -> MigrationReport {
    MigrationReport {
        migrated: false,
        reason: Some("no legacy layout to migrate".to_string()),
        ..MigrationReport::default()
    }
}

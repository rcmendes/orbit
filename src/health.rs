//! Board-health checks shared by `orbit doctor` and migration preflight. Each
//! function appends `Issue`s; the caller decides which checks run and whether
//! an error aborts or becomes an issue.

use std::path::Path;

use crate::board::Board;
use crate::comment;
use crate::error::{OrbitError, Result, code};
use crate::git;
use crate::graph::{Graph, Issue};

/// Flags `comments/{id}.md` files with no matching task or unreadable content.
/// `verify_header` additionally requires the `# Comments: {id}` header that
/// migrations rewrite; doctor only reports orphans and corruption.
pub fn comment_files(
    board: &Board,
    graph: &Graph,
    verify_header: bool,
    issues: &mut Vec<Issue>,
) -> Result<()> {
    for id in board.comment_ids()? {
        if !graph.tasks.contains_key(&id) {
            issues.push(Issue::new(
                code::ORPHAN_COMMENT,
                Some(&id),
                format!("comments/{id}.md has no matching task"),
            ));
            continue;
        }
        if let Err(error) = board.read_comments(&id) {
            issues.push(Issue::new(error.code, Some(&id), error.message));
            continue;
        }
        if verify_header {
            match header_matches(&board.comment_path(&id), &id) {
                Ok(true) => {}
                Ok(false) => issues.push(Issue::new(
                    code::CORRUPT_COMMENT,
                    Some(&id),
                    format!(
                        "comments/{id}.md does not start with `{}`; repair the header before migrating",
                        comment::header(&id)
                    ),
                )),
                Err(error) => issues.push(Issue::new(error.code, Some(&id), error.message)),
            }
        }
    }
    Ok(())
}

fn header_matches(path: &Path, id: &str) -> Result<bool> {
    let content = std::fs::read_to_string(path)
        .map_err(|error| OrbitError::io(format!("read {}", path.display()), error))?;
    Ok(comment::has_header(&content, id))
}

/// Flags tasks whose `parent` disagrees with the parent encoded in their ID.
/// Skip this entirely when the board has a prefix mismatch: the IDs then use
/// the observed prefix and every SubTask would be flagged.
pub fn parent_id_agreement(board: &Board, graph: &Graph, issues: &mut Vec<Issue>) {
    for task in graph.tasks.values() {
        let encoded_parent = board.config.id.parent_id(&task.id);
        match (&task.front.parent, encoded_parent) {
            (Some(parent), Some(encoded)) if parent != &encoded => {
                issues.push(Issue::new(
                    code::PARENT_MISMATCH,
                    Some(&task.id),
                    format!(
                        "{} has parent {parent} but its ID encodes {encoded}",
                        task.id
                    ),
                ));
            }
            (None, Some(encoded)) => {
                issues.push(Issue::new(
                    code::PARENT_MISMATCH,
                    Some(&task.id),
                    format!(
                        "{} is a SubTask ID but has no parent; expected {encoded}",
                        task.id
                    ),
                ));
            }
            (Some(parent), None) => {
                issues.push(Issue::new(
                    code::PARENT_MISMATCH,
                    Some(&task.id),
                    format!(
                        "{} has parent {parent} but its ID is not a SubTask ID",
                        task.id
                    ),
                ));
            }
            _ => {}
        }
    }
}

/// Flags a Board checked out on a branch other than `board.config.board.branch`.
pub fn branch_mismatch(board: &Board, issues: &mut Vec<Issue>) {
    match git::current_branch(&board.root) {
        Ok(branch) if branch != board.config.board.branch => issues.push(Issue::new(
            code::BOARD_CORRUPT,
            None,
            format!(
                "board is on branch {branch}, expected {}",
                board.config.board.branch
            ),
        )),
        Ok(_) => {}
        Err(error) => issues.push(Issue::new(code::BOARD_CORRUPT, None, error.message)),
    }
}

use std::path::{Path, PathBuf};

use crate::board::Board;

pub const RELATIVE_GUIDANCE_PATH: &str = "docs/agents/project-guidance.md";

pub fn guidance_path(board: &Board) -> PathBuf {
    // Project guidance lives in the repository or Workspace that anchors the
    // Board: the Orbit folder's parent for `.orbit/board` and pointer Boards,
    // the Board folder's parent for unanchored legacy layouts.
    let anchor = board
        .resolved_orbit_dir()
        .and_then(|orbit| orbit.parent().map(|parent| parent.to_path_buf()))
        .or_else(|| board.root.parent().map(|parent| parent.to_path_buf()))
        .unwrap_or_else(|| board.root.clone());
    anchor.join(RELATIVE_GUIDANCE_PATH)
}

pub fn read_project_guidance(board: &Board) -> (Option<String>, Option<PathBuf>) {
    let path = guidance_path(board);
    match std::fs::read_to_string(&path) {
        Ok(content) => (Some(content), Some(path)),
        Err(_) => (None, Some(path)),
    }
}

pub fn guidance_missing_note(path: &Path) -> String {
    format!(
        "project guidance not found at {}; triage should leave metadata unchanged when evidence is insufficient",
        path.display()
    )
}

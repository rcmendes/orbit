use std::path::{Path, PathBuf};

use crate::board::Board;

pub const RELATIVE_GUIDANCE_PATH: &str = "docs/agents/project-guidance.md";

pub fn guidance_path(board: &Board) -> PathBuf {
    board
        .root
        .parent()
        .unwrap_or(&board.root)
        .join(RELATIVE_GUIDANCE_PATH)
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

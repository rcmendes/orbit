use serde_json::{Value, json};

use crate::cli::LinkArgs;

use super::*;

pub(super) fn cmd_link(args: LinkArgs) -> Result<crate::output::CommandOutput> {
    let cwd = std::env::current_dir()
        .map_err(|error| OrbitError::io("resolve current directory", error))?;
    if !crate::git::is_git_repo(&cwd) {
        return Err(OrbitError::new(
            code::NOT_A_REPOSITORY,
            "`orbit link` must run inside a Git repository",
        ));
    }

    let (target, legacy_nested) = Board::resolve_target(&args.path)?;
    let board = Board::discover(Some(&target))?;
    let local = Board::local_root()?;
    let legacy_local = Board::legacy_local_root()?;
    let mut shadowed_boards = Vec::new();
    for candidate in [local.clone(), legacy_local] {
        if candidate != board.root
            && Board::is_board(&candidate)
            && !shadowed_boards.contains(&candidate)
        {
            shadowed_boards.push(candidate);
        }
    }
    let shadowed = !shadowed_boards.is_empty();

    Board::write_link(&board.root)?;

    let root = crate::git::repo_root()?;
    let mut warnings = board.warnings.clone();
    if board.config.board.storage.is_none()
        && !warnings
            .iter()
            .any(|warning| warning.starts_with(crate::board::LEGACY_LAYOUT))
    {
        warnings.push(format!(
            "{}: linked Board at {} predates board.storage",
            crate::board::LEGACY_LAYOUT,
            board.root.display()
        ));
    }
    if legacy_nested {
        warnings.push(format!(
            "{}: linked workspace resolves the nested Board at {}",
            crate::board::LEGACY_LAYOUT,
            board.root.display()
        ));
    }
    for path in &shadowed_boards {
        warnings.push(format!(
            "the local Board at {} is now shadowed but was not deleted",
            path.display()
        ));
    }

    let mut refreshed = Vec::new();
    if !args.no_refresh {
        match crate::setup::refresh(&root) {
            Ok(outcome) => {
                refreshed = outcome
                    .files
                    .iter()
                    .filter(|file| {
                        matches!(
                            file.action,
                            crate::setup::Action::Created
                                | crate::setup::Action::Updated
                                | crate::setup::Action::Removed
                        )
                    })
                    .map(|file| file.relative.to_string_lossy().into_owned())
                    .collect::<Vec<_>>();
                warnings.extend(outcome.output.warnings);
            }
            Err(error) => warnings.push(format!(
                "could not refresh agent configuration: {}; run `orbit setup <client>` to update MCP entries",
                error.message
            )),
        }
    }

    let mut human = format!(
        "linked this repository to the Board at {}\n",
        board.root.display()
    );
    for path in &refreshed {
        human.push_str(&format!("refreshed {path}\n"));
    }

    let mut output = crate::output::CommandOutput::new(
        json!({
            "board": board.root,
            "local_board": local,
            "shadowed": shadowed,
            "shadowed_boards": shadowed_boards,
            "refreshed": refreshed,
        }),
        human,
    );
    output.warnings = warnings;
    Ok(output)
}

pub(super) fn cmd_unlink() -> Result<crate::output::CommandOutput> {
    let cwd = std::env::current_dir()
        .map_err(|error| OrbitError::io("resolve current directory", error))?;
    if !crate::git::is_git_repo(&cwd) {
        return Err(OrbitError::new(
            code::NOT_A_REPOSITORY,
            "`orbit unlink` must run inside a Git repository",
        ));
    }

    let local = Board::local_root()?;
    let local_exists = Board::is_board(&local);
    let unlinked = Board::clear_link()?;
    let root = crate::git::repo_root()?;
    let mut warnings = Vec::new();

    if unlinked {
        match crate::setup::refresh(&root) {
            Ok(outcome) => warnings.extend(outcome.output.warnings),
            Err(error) => warnings.push(format!(
                "could not refresh agent configuration: {}; run `orbit setup <client>` to update MCP entries",
                error.message
            )),
        }
        if !local_exists {
            let legacy = Board::legacy_local_root().ok();
            if let Some(legacy) = legacy.filter(|path| Board::is_board(path)) {
                warnings.push(format!(
                    "this repository has no .orbit Board; it still resolves the legacy Board at {}; run `orbit doctor --migrate`",
                    legacy.display()
                ));
            } else {
                warnings.push(
                    "this repository has no local Board; run `orbit init` to create one"
                        .to_string(),
                );
            }
        }
    }

    let mut output = crate::output::CommandOutput::new(
        json!({
            "unlinked": unlinked,
            "board": if local_exists { json!(local) } else { Value::Null },
        }),
        if !unlinked {
            "this repository is not linked".to_string()
        } else if local_exists {
            format!("unlinked this repository; it now uses {}", local.display())
        } else {
            "unlinked this repository; no local Board exists, run `orbit init`".to_string()
        },
    );
    output.warnings = warnings;
    Ok(output)
}

//! `orbit board host` and `orbit board sync` (ADR 0019).

use serde_json::json;

use crate::board::Board;
use crate::board_host;
use crate::board_sync;
use crate::cli::{BoardHostArgs, BoardSyncArgs};
use crate::error::Result;
use crate::output::CommandOutput;

/// `orbit board host [REPO|URL|--none]` — show, set, or clear the host.
pub(super) fn cmd_host(board: &Board, args: &BoardHostArgs) -> Result<CommandOutput> {
    if args.none {
        let outcome = board_host::clear_host(&board.root)?;
        let data = json!({
            "host": outcome.host,
            "host_sync": board.config.board.host_sync.as_str(),
            "remote_branch_deleted": false,
        });
        return Ok(CommandOutput::new(
            data,
            "board host cleared; the remote `orbit` branch was left untouched\n".to_string(),
        ));
    }

    match &args.target {
        None => board_host::status_output(board),
        Some(spec) => {
            let outcome = board_host::set_host(&board.root, spec, false)?;
            let data = json!({
                "host": outcome.host,
                "url": outcome.url,
                "remote_tip": outcome.remote_tip,
                "joined": outcome.joined,
                "host_sync": board.config.board.host_sync.as_str(),
            });
            let human = match (&outcome.host, &outcome.url, &outcome.remote_tip) {
                (Some(host), Some(url), Some(tip)) if host != url => {
                    format!("board host set to {host} ({url}); remote `orbit` tip {tip}\n")
                }
                (Some(host), Some(_), Some(tip)) => {
                    format!("board host set to {host}; remote `orbit` tip {tip}\n")
                }
                (Some(host), _, _) => format!("board host set to {host}\n"),
                _ => "board host set\n".to_string(),
            };
            Ok(CommandOutput::new(data, human))
        }
    }
}

/// `orbit board sync [--resolve FIELD=local|host …]` — fetch, merge, push.
pub(super) fn cmd_sync(board: &Board, args: &BoardSyncArgs) -> Result<CommandOutput> {
    let report = board_sync::sync(board, &args.resolutions)?;
    let data = json!({
        "state": report.state,
        "remote_tip": report.remote_tip,
        "merged_files": report.merged_files,
        "conflicts_resolved": report.conflicts_resolved,
        "notices": report.notices,
        "unused_resolutions": report.unused_resolutions,
    });
    let mut human = format!("board sync: {}\n", report.state);
    if let Some(tip) = &report.remote_tip {
        human.push_str(&format!("  host `orbit` tip: {tip}\n"));
    }
    if report.state == board_sync::SyncState::Merged {
        human.push_str(&format!(
            "  merged {} divergent files\n",
            report.merged_files
        ));
    }
    for field in &report.conflicts_resolved {
        human.push_str(&format!("  resolved {field} by --resolve\n"));
    }
    for notice in &report.notices {
        human.push_str(&format!("  notice: {notice}\n"));
    }
    for field in &report.unused_resolutions {
        human.push_str(&format!(
            "  warning: resolution `{field}` matched no conflict\n"
        ));
    }
    let mut output = CommandOutput::new(data, human);
    output.warnings = report
        .unused_resolutions
        .iter()
        .map(|field| format!("board sync: resolution `{field}` matched no conflict"))
        .collect();
    Ok(output)
}

/// `orbit board move DIR` — relocate the Board folder.
pub(super) fn cmd_move(
    access: crate::board::BoardAccess,
    args: &crate::cli::BoardMoveArgs,
) -> Result<CommandOutput> {
    let outcome = crate::board_move::move_board(access, &args.target)?;
    let data = json!({
        "from": outcome.from,
        "to": outcome.to,
        "visible": outcome.visible,
        "relinked": outcome.relinked,
    });
    let mut human = format!(
        "moved the Board from {} to {}\n",
        outcome.from.display(),
        outcome.to.display()
    );
    for path in &outcome.relinked {
        human.push_str(&format!("relinked member {}\n", path.display()));
    }
    Ok(CommandOutput::new(data, human))
}

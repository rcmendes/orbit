mod board;
mod crud;
mod init;
mod lifecycle;
mod query;
mod relations;
mod render;
mod repo;
mod slug;

use serde_json::Value;
use std::io::Read;
use std::path::Path;

use crate::board::Board;
use crate::cli::{Cli, Command, CommentCommand, LabelCommand, SlugCommand};
use crate::error::{OrbitError, Result, code};
use crate::graph::{self, Graph};
use crate::task::{self, Task};

use crud::{cmd_add, cmd_delete, cmd_update};
use init::cmd_init;
use lifecycle::{
    cmd_cancel, cmd_checkpoint, cmd_claim, cmd_done, cmd_heartbeat, cmd_release, cmd_reopen,
};
use query::{cmd_export, cmd_list, cmd_prime, cmd_ready, cmd_show, cmd_sync, cmd_tree};
use relations::{cmd_comment, cmd_depend, cmd_label, cmd_undepend};
use repo::run as cmd_repo;
use slug::cmd_slug;

pub fn run(cli: Cli) -> Result<crate::output::CommandOutput> {
    let Cli {
        json,
        dir,
        no_sync,
        command,
    } = cli;
    if no_sync {
        crate::remote::auto_push::suppress();
        crate::board_sync::suppress();
    }

    if json && matches!(&command, Command::Prime(args) if args.hook_json) {
        return Err(OrbitError::invalid_argument(
            "--json and --hook-json are mutually exclusive",
        ));
    }

    match command {
        Command::Migrate(args) => crate::board_migrate::cmd_migrate(&args, dir.as_deref()),
        Command::Init(args) => cmd_init(args, dir.as_deref()),
        Command::Repo(args) => with_board(dir.as_deref(), true, |board| cmd_repo(board, args)),
        Command::Slug(args) => {
            let exclusive = matches!(args.command, SlugCommand::Migrate(_));
            with_board(dir.as_deref(), exclusive, |board| cmd_slug(board, args))
        }
        Command::Add(args) => with_task_board(dir.as_deref(), true, |board| cmd_add(board, args)),
        Command::List(args) => with_board(dir.as_deref(), false, |board| cmd_list(board, args)),
        Command::Ready(args) => {
            let exclusive = args.claim;
            with_task_board(dir.as_deref(), exclusive, |board| cmd_ready(board, args))
        }
        Command::Show(args) => with_board(dir.as_deref(), false, |board| cmd_show(board, args)),
        Command::Tree(args) => with_board(dir.as_deref(), false, |board| cmd_tree(board, args)),
        Command::Claim(args) => {
            with_task_board(dir.as_deref(), true, |board| cmd_claim(board, args))
        }
        Command::Release(args) => {
            with_task_board(dir.as_deref(), true, |board| cmd_release(board, args))
        }
        Command::Heartbeat(args) => {
            with_board(dir.as_deref(), true, |board| cmd_heartbeat(board, args))
        }
        Command::Checkpoint(args) => {
            with_board(dir.as_deref(), true, |board| cmd_checkpoint(board, args))
        }
        Command::Done(args) => with_task_board(dir.as_deref(), true, |board| cmd_done(board, args)),
        Command::Cancel(args) => {
            with_task_board(dir.as_deref(), true, |board| cmd_cancel(board, args))
        }
        Command::Reopen(args) => {
            with_task_board(dir.as_deref(), true, |board| cmd_reopen(board, args))
        }
        Command::Update(args) => {
            with_task_board(dir.as_deref(), true, |board| cmd_update(board, args))
        }
        Command::Label(args) => {
            let exclusive = !matches!(args.command, LabelCommand::List);
            with_task_board(dir.as_deref(), exclusive, |board| cmd_label(board, args))
        }
        Command::Comment(args) => {
            let exclusive = matches!(args.command, CommentCommand::Add(_));
            with_board(dir.as_deref(), exclusive, |board| cmd_comment(board, args))
        }
        Command::Depend(args) => with_board(dir.as_deref(), true, |board| cmd_depend(board, args)),
        Command::Undepend(args) => {
            with_board(dir.as_deref(), true, |board| cmd_undepend(board, args))
        }
        Command::Delete(args) => with_board(dir.as_deref(), true, |board| cmd_delete(board, args)),
        Command::Prime(args) => prime_with_host_sync(dir.as_deref(), args),
        Command::Export => with_board(dir.as_deref(), false, cmd_export),
        Command::Setup(args) => crate::setup::run(args, dir.as_deref()),
        Command::Mcp => crate::mcp::serve(dir.as_deref())
            .map(|()| crate::output::CommandOutput::new(Value::Null, "").with_silent()),
        Command::Doctor(args) => crate::doctor::run(dir.as_deref(), args),
        Command::Sync => with_board(dir.as_deref(), true, cmd_sync),
        Command::Board(args) => match &args.command {
            crate::cli::BoardCommand::Host(host_args) => {
                if host_args.target.is_none() && !host_args.none {
                    with_board(dir.as_deref(), false, |board| {
                        board::cmd_host(board, host_args)
                    })
                } else {
                    with_board(dir.as_deref(), true, |board| {
                        board::cmd_host(board, host_args)
                    })
                }
            }
            crate::cli::BoardCommand::Sync(sync_args) => {
                // The command performs the sync itself; do not auto-sync again.
                crate::board_sync::suppress();
                // `sync` takes the Board lock itself, only around local steps.
                let board = crate::board::BoardAccess::open(
                    dir.as_deref(),
                    crate::board::AccessMode::Write,
                )?
                .into_board();
                let mut output = board::cmd_sync(&board, sync_args)?;
                output.warnings.extend(board.warnings.iter().cloned());
                Ok(output)
            }
            crate::cli::BoardCommand::Move(move_args) => {
                // The old root is gone after the move; never auto-sync from it.
                crate::board_sync::suppress();
                let access = crate::board::BoardAccess::open(
                    dir.as_deref(),
                    crate::board::AccessMode::Write,
                )?;
                let warnings = access.board().warnings.clone();
                let mut output = board::cmd_move(access, move_args)?;
                output.warnings.extend(warnings);
                Ok(output)
            }
        },
        Command::Remote(args) => {
            if remote_is_read_only(&args) {
                return with_board_read_only(dir.as_deref(), |board| {
                    crate::remote::commands::run(board, args)
                });
            }
            with_board(dir.as_deref(), true, |board| {
                crate::remote::commands::run(board, args)
            })
        }
        Command::Triage(args) => with_task_board(dir.as_deref(), true, |board| {
            crate::triage::commands::run(board, args, json)
        }),
    }
}

/// `orbit remote` previews and read-only views run without the exclusive
/// Board lock; anything that writes configuration, the catalog, or Tasks
/// keeps the normal write path.
fn remote_is_read_only(args: &crate::cli::RemoteArgs) -> bool {
    use crate::cli::{RemoteCommand, RemoteIssuesCommand};
    match &args.command {
        RemoteCommand::Status | RemoteCommand::List => true,
        RemoteCommand::Issues(issues) => {
            !matches!(issues.command, Some(RemoteIssuesCommand::Refresh))
        }
        RemoteCommand::Import(args) => !args.apply,
        RemoteCommand::Detach(args) | RemoteCommand::Reattach(args) => !args.apply,
        RemoteCommand::Create(args) => !args.apply,
        RemoteCommand::Sync(args) => !args.apply,
        RemoteCommand::Connect(_)
        | RemoteCommand::Disconnect(_)
        | RemoteCommand::Link(_)
        | RemoteCommand::AutoPush(_) => false,
    }
}

fn with_board<F>(dir: Option<&Path>, exclusive: bool, f: F) -> Result<crate::output::CommandOutput>
where
    F: FnOnce(&Board) -> Result<crate::output::CommandOutput>,
{
    let mode = if exclusive {
        crate::board::AccessMode::Write
    } else {
        crate::board::AccessMode::Read
    };
    with_board_access(dir, mode, AutoPush::Off, f)
}

/// Task commands that may push mapped changes after a successful mutation.
fn with_task_board<F>(
    dir: Option<&Path>,
    exclusive: bool,
    f: F,
) -> Result<crate::output::CommandOutput>
where
    F: FnOnce(&Board) -> Result<crate::output::CommandOutput>,
{
    let mode = if exclusive {
        crate::board::AccessMode::Write
    } else {
        crate::board::AccessMode::Read
    };
    with_board_access(dir, mode, AutoPush::On, f)
}

fn with_board_read_only<F>(dir: Option<&Path>, f: F) -> Result<crate::output::CommandOutput>
where
    F: FnOnce(&Board) -> Result<crate::output::CommandOutput>,
{
    with_board_access(dir, crate::board::AccessMode::Inspect, AutoPush::Off, f)
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum AutoPush {
    Off,
    On,
}

fn with_board_access<F>(
    dir: Option<&Path>,
    mode: crate::board::AccessMode,
    auto_push: AutoPush,
    f: F,
) -> Result<crate::output::CommandOutput>
where
    F: FnOnce(&Board) -> Result<crate::output::CommandOutput>,
{
    let access = crate::board::BoardAccess::open(dir, mode)?;
    let board = access.board();
    let snapshot = (auto_push == AutoPush::On
        && matches!(mode, crate::board::AccessMode::Write)
        && crate::remote::auto_push::enabled(board)
        && !crate::remote::auto_push::suppressed())
    .then(|| crate::remote::auto_push::snapshot(board));
    let mut output = f(board)?;
    if let Some(before) = snapshot {
        if board.commit_failed() {
            output.warnings.push(
                "auto-push skipped because the local Board commit failed; retry after `orbit sync`"
                    .to_string(),
            );
        } else {
            output
                .warnings
                .extend(crate::remote::auto_push::after_command_default(
                    board, &before,
                ));
        }
    }
    // Host sync does network I/O: release the Board lock first.
    let board = access.into_board();
    if matches!(mode, crate::board::AccessMode::Write) {
        output
            .warnings
            .extend(crate::board_sync::auto_after(&board));
    }
    output.warnings.extend(board.warnings.iter().cloned());
    Ok(output)
}

/// `orbit prime` syncs the Board host first when `board.host_sync = auto`,
/// so the primed view already includes the host's changes. Failures surface
/// as warnings; prime itself never fails because of the host.
fn prime_with_host_sync(
    dir: Option<&Path>,
    args: crate::cli::PrimeArgs,
) -> Result<crate::output::CommandOutput> {
    if host_sync_at_prime(dir) {
        // `sync` takes the Board lock itself, only around local steps.
        let board =
            crate::board::BoardAccess::open(dir, crate::board::AccessMode::Write)?.into_board();
        let mut warnings = crate::board_sync::auto_after(&board);
        let mut output = with_board(dir, false, |board| cmd_prime(board, args))?;
        warnings.append(&mut output.warnings);
        output.warnings = warnings;
        return Ok(output);
    }
    with_board(dir, false, |board| cmd_prime(board, args))
}

fn host_sync_at_prime(dir: Option<&Path>) -> bool {
    if crate::board_sync::suppressed() {
        return false;
    }
    let Ok(board) = Board::discover(dir) else {
        return false;
    };
    let Ok(config) = crate::board::Config::load(&board.root) else {
        return false;
    };
    config.board.host_sync == crate::board::HostSyncMode::Auto && config.board.host.is_some()
}

fn mutation_output(
    result: crate::mutation::MutationResult,
    human: String,
) -> crate::output::CommandOutput {
    let mut output =
        crate::output::CommandOutput::new(graph::task_detail(&result.graph, &result.task), human);
    output.warnings = result.warnings;
    output
}

fn task_output(
    graph: &Graph,
    task: &Task,
    human: String,
    warning: Option<String>,
) -> crate::output::CommandOutput {
    let mut output = crate::output::CommandOutput::new(graph::task_detail(graph, task), human);
    output.warnings.extend(warning);
    output
}

fn validate_priority(priority: u8) -> Result<()> {
    if priority > 3 {
        return Err(OrbitError::invalid_argument(
            "priority must be between 0 and 3",
        ));
    }
    Ok(())
}

fn normalize_labels(labels: &[String]) -> Result<Vec<String>> {
    let mut normalized = Vec::with_capacity(labels.len());
    for label in labels {
        normalized.push(task::normalize_label(label)?);
    }
    normalized.sort();
    normalized.dedup();
    Ok(normalized)
}

fn normalize_repo_filter(repo: Option<&str>) -> Result<Option<String>> {
    repo.map(|repo| {
        let repo = repo.trim();
        if repo.is_empty() {
            return Err(OrbitError::invalid_argument("--repo must not be empty"));
        }
        // Keep the same validation as legacy repo labels, without turning
        // the Repository field filter into a required label.
        task::normalize_label(&format!("repo:{repo}"))?;
        Ok(repo.to_string())
    })
    .transpose()
}

fn matches_repo(task: &Task, repo: Option<&str>) -> bool {
    repo.is_none_or(|repo| {
        task.front.repo.as_deref() == Some(repo)
            || task
                .front
                .labels
                .iter()
                .any(|label| label.eq_ignore_ascii_case(&format!("repo:{repo}")))
    })
}

fn normalize_waiting(waiting_on: Option<String>) -> Result<Option<String>> {
    match waiting_on {
        None => Ok(None),
        Some(value) => {
            let value = value.trim().to_string();
            if value.is_empty() {
                return Err(OrbitError::invalid_argument(
                    "--waiting-on must not be empty",
                ));
            }
            Ok(Some(value))
        }
    }
}

fn parse_area_option(board: &Board, area: Option<String>) -> Result<Option<String>> {
    match area {
        None => Ok(None),
        Some(value) => Ok(Some(parse_area_value(board, &value)?)),
    }
}

/// Normalizes one Area filter value and validates it against the Board's
/// `board.areas` list; `task::normalize_area` treats an empty list as
/// unrestricted.
fn parse_area_value(board: &Board, value: &str) -> Result<String> {
    task::normalize_area(value, &board.config.board.areas)
}

/// Normalizes repeatable Area filter flags the same way as `--area`.
fn normalize_areas(board: &Board, areas: &[String]) -> Result<Vec<String>> {
    areas
        .iter()
        .map(|area| parse_area_value(board, area))
        .collect()
}

fn resolve_body(body: Option<String>, file: Option<std::path::PathBuf>) -> Result<String> {
    match (body, file) {
        (Some(body), None) => {
            if body == "-" {
                read_stdin()
            } else {
                Ok(body)
            }
        }
        (None, Some(path)) => {
            if path == Path::new("-") {
                read_stdin()
            } else {
                std::fs::read_to_string(&path)
                    .map_err(|error| OrbitError::io(format!("read {}", path.display()), error))
            }
        }
        (None, None) => Ok(String::new()),
        _ => Err(OrbitError::invalid_argument(
            "--body and --body-file are mutually exclusive",
        )),
    }
}

fn read_stdin() -> Result<String> {
    let mut body = String::new();
    std::io::stdin()
        .read_to_string(&mut body)
        .map_err(|error| OrbitError::io("read stdin", error))?;
    Ok(body)
}

fn resolve_agent(by: Option<String>) -> Result<String> {
    optional_agent(by)
        .ok_or_else(|| OrbitError::config("set --by, ORBIT_AGENT, or USER to identify the agent"))
}

fn optional_agent(by: Option<String>) -> Option<String> {
    if let Some(by) = by {
        let by = by.trim().to_string();
        if !by.is_empty() {
            return Some(by);
        }
    }
    for key in ["ORBIT_AGENT", "USER", "USERNAME"] {
        if let Ok(value) = std::env::var(key) {
            let value = value.trim().to_string();
            if !value.is_empty() {
                return Some(value);
            }
        }
    }
    None
}

fn truncate(text: &str, max: usize) -> String {
    if max == 0 {
        return String::new();
    }
    if text.chars().count() <= max {
        return text.to_string();
    }
    let head: String = text.chars().take(max - 1).collect();
    format!("{head}…")
}

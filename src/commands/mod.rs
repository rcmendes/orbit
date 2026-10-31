mod crud;
mod init;
mod lifecycle;
mod link;
mod query;
mod relations;
mod render;
mod slug;

use clap::ValueEnum;
use serde_json::Value;
use std::io::Read;
use std::path::Path;

use crate::board::Board;
use crate::cli::{Cli, Command, CommentCommand, LabelCommand, SlugCommand, WorkspaceCommand};
use crate::error::{OrbitError, Result, code};
use crate::graph::{self, Graph};
use crate::task::{self, Task, TaskArea};

use crud::{cmd_add, cmd_delete, cmd_update};
use init::cmd_init;
use lifecycle::{
    cmd_cancel, cmd_checkpoint, cmd_claim, cmd_done, cmd_heartbeat, cmd_release, cmd_reopen,
};
use link::{cmd_link, cmd_unlink};
use query::{cmd_export, cmd_list, cmd_prime, cmd_ready, cmd_show, cmd_sync, cmd_tree};
use relations::{cmd_comment, cmd_depend, cmd_label, cmd_undepend};
use slug::cmd_slug;

pub fn run(cli: Cli) -> Result<crate::output::CommandOutput> {
    let Cli {
        json,
        dir,
        no_sync,
        command,
    } = cli;
    if no_sync {
        crate::github::auto_push::suppress();
    }

    if json && matches!(&command, Command::Prime(args) if args.hook_json) {
        return Err(OrbitError::invalid_argument(
            "--json and --hook-json are mutually exclusive",
        ));
    }

    match command {
        Command::Init(args) => cmd_init(args, dir.as_deref()),
        Command::Link(args) => cmd_link(args),
        Command::Unlink => cmd_unlink(),
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
        Command::Prime(args) => with_board(dir.as_deref(), false, |board| cmd_prime(board, args)),
        Command::Export => with_board(dir.as_deref(), false, cmd_export),
        Command::Setup(args) => crate::setup::run(args, dir.as_deref()),
        Command::Mcp => crate::mcp::serve(dir.as_deref())
            .map(|()| crate::output::CommandOutput::new(Value::Null, "").with_silent()),
        Command::Doctor(args) => crate::doctor::run(dir.as_deref(), args),
        Command::Sync => with_board(dir.as_deref(), true, cmd_sync),
        Command::GitHub(args) => {
            if matches!(
                &args.command,
                crate::cli::GitHubCommand::Status
                    | crate::cli::GitHubCommand::Create(crate::cli::GitHubCreateArgs {
                        apply: false,
                        ..
                    })
            ) {
                return with_board_read_only(dir.as_deref(), |board| {
                    crate::github::commands::run(board, args)
                });
            }
            with_board(dir.as_deref(), true, |board| {
                crate::github::commands::run(board, args)
            })
        }
        Command::Workspace(args) => match args.command {
            WorkspaceCommand::Migrate(args) => crate::workspace_migrate::run(dir.as_deref(), args),
            command => {
                let args = crate::cli::WorkspaceArgs { command };
                if matches!(
                    &args.command,
                    crate::cli::WorkspaceCommand::Status
                        | crate::cli::WorkspaceCommand::Issues(crate::cli::WorkspaceIssuesArgs {
                            command: crate::cli::WorkspaceIssuesCommand::List(_)
                                | crate::cli::WorkspaceIssuesCommand::Inspect(_)
                        })
                ) {
                    with_board_read_only(dir.as_deref(), |board| crate::workspace::run(board, args))
                } else {
                    with_board(dir.as_deref(), true, |board| {
                        crate::workspace::run(board, args)
                    })
                }
            }
        },
        Command::Triage(args) => with_task_board(dir.as_deref(), true, |board| {
            crate::triage::commands::run(board, args, json)
        }),
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
        && board.config.github.auto_push
        && !crate::github::auto_push::suppressed())
    .then(|| crate::github::auto_push::snapshot(board));
    let mut output = f(board)?;
    if let Some(before) = snapshot {
        if output
            .warnings
            .iter()
            .any(|warning| warning.contains("COMMIT_FAILED"))
        {
            output.warnings.push(
                "auto-push skipped because the local Board commit failed; retry after `orbit sync`"
                    .to_string(),
            );
        } else {
            output
                .warnings
                .extend(crate::github::auto_push::after_command(board, &before));
        }
    }
    output.warnings.extend(board.warnings.iter().cloned());
    Ok(output)
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

fn normalize_label_filters(labels: &[String], repo: Option<&str>) -> Result<Vec<String>> {
    let mut normalized = normalize_labels(labels)?;
    if let Some(repo) = repo {
        let repo = repo.trim();
        if repo.is_empty() {
            return Err(OrbitError::invalid_argument("--repo must not be empty"));
        }
        normalized.push(task::normalize_label(&format!("repo:{repo}"))?);
    }
    normalized.sort();
    normalized.dedup();
    Ok(normalized)
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

fn parse_area_option(area: Option<String>) -> Result<Option<TaskArea>> {
    match area {
        None => Ok(None),
        Some(value) => Ok(Some(parse_area_value(&value)?)),
    }
}

fn parse_area_value(value: &str) -> Result<TaskArea> {
    let value = value.trim();
    TaskArea::from_str(value, true).map_err(|_| {
        let allowed = TaskArea::all()
            .iter()
            .map(|area| area.as_str())
            .collect::<Vec<_>>()
            .join(", ");
        OrbitError::new(
            code::INVALID_AREA,
            format!("unknown area `{value}`; expected one of: {allowed}"),
        )
    })
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

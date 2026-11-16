mod batch;
mod board;
mod crud;
mod init;
mod lifecycle;
mod property;
mod query;
mod relations;
mod render;
mod repo;
mod slug;

use serde_json::Value;
use std::io::Read;
use std::path::Path;

use crate::board::Board;
use crate::cli::{
    Cli, Command, CommentCommand, LabelCommand, LinkCommand, PropertyCommand, RepoCommand,
    SlugCommand,
};
use crate::error::{OrbitError, Result, code};
use crate::graph::{self, Graph};
use crate::task::{self, Task};

use batch::cmd_batch;
use crud::{cmd_add, cmd_delete, cmd_update};
use init::cmd_init;
use lifecycle::{
    cmd_assign, cmd_cancel, cmd_checkpoint, cmd_claim, cmd_done, cmd_heartbeat, cmd_release,
    cmd_reopen, cmd_transition,
};
use property::cmd_property;
use query::{cmd_export, cmd_list, cmd_prime, cmd_ready, cmd_show, cmd_sync, cmd_tree};
use relations::{cmd_comment, cmd_depend, cmd_label, cmd_link, cmd_undepend};
use repo::run as cmd_repo;
use slug::cmd_slug;

/// The Task an `--if-version` guards.
#[derive(Clone)]
enum VersionTarget {
    /// A Task reference resolved under the exclusive Board lock before the
    /// mutation runs.
    Task(String),
    /// `orbit ready --claim`: the picked Task is checked inside `cmd_ready`,
    /// after selection and inside the same lock.
    ReadyClaim,
}

/// Optimistic-concurrency (`--if-version`) and idempotency
/// (`--idempotency-key`) inputs for one invocation, threaded into every
/// Board access the command takes.
#[derive(Clone, Default)]
struct Concurrency {
    if_version: Option<String>,
    target: Option<VersionTarget>,
    idempotency_key: Option<String>,
}

impl Concurrency {
    /// Commands that bypass `with_board_access` (they open the Board or run
    /// network I/O themselves) cannot honor `--idempotency-key`; reject it
    /// loudly instead of silently dropping the guarantee. `--if-version` is
    /// already rejected for them by [`version_target`].
    fn reject_idempotency(&self, command: &str) -> Result<()> {
        if self.idempotency_key.is_some() {
            return Err(OrbitError::invalid_argument(format!(
                "--idempotency-key is not supported by `{command}`"
            )));
        }
        Ok(())
    }
}

/// The Task an `--if-version` guards, or an error when the command cannot
/// honor it. Only Task mutations take a version; `orbit batch` carries a
/// per-operation `if_version` field instead.
fn version_target(command: &Command) -> Result<VersionTarget> {
    fn task(id: &str) -> VersionTarget {
        VersionTarget::Task(id.to_string())
    }
    fn unsupported(command: &str) -> OrbitError {
        OrbitError::invalid_argument(format!(
            "--if-version does not apply to `{command}`; it guards a Task mutation — pass the Task's `updated_at` from `orbit show <id>`"
        ))
    }
    fn name(command: &Command) -> &'static str {
        match command {
            Command::Migrate(_) => "orbit migrate",
            Command::Init(_) => "orbit init",
            Command::Repo(_) => "orbit repo",
            Command::Slug(_) => "orbit slug",
            Command::Add(_) => "orbit add",
            Command::List(_) => "orbit list",
            Command::Ready(_) => "orbit ready",
            Command::Show(_) => "orbit show",
            Command::Tree(_) => "orbit tree",
            Command::Prime(_) => "orbit prime",
            Command::Export => "orbit export",
            Command::Setup(_) => "orbit setup",
            Command::Mcp => "orbit mcp",
            Command::Doctor(_) => "orbit doctor",
            Command::Sync => "orbit sync",
            Command::Capabilities => "orbit capabilities",
            Command::Schema(_) => "orbit schema",
            Command::Board(_) => "orbit board",
            Command::Remote(_) => "orbit remote",
            Command::Triage(_) => "orbit triage",
            Command::Claim(_)
            | Command::Assign(_)
            | Command::Release(_)
            | Command::Heartbeat(_)
            | Command::Checkpoint(_)
            | Command::Done(_)
            | Command::Cancel(_)
            | Command::Reopen(_)
            | Command::Transition(_)
            | Command::Update(_)
            | Command::Delete(_)
            | Command::Depend(_)
            | Command::Undepend(_)
            | Command::Label(_)
            | Command::Batch(_)
            | Command::Property(_)
            | Command::Comment(_)
            | Command::Link(_) => unreachable!("handled above"),
        }
    }
    Ok(match command {
        Command::Claim(args) => task(&args.id),
        Command::Assign(args) => task(&args.id),
        Command::Release(args) => task(&args.id),
        Command::Heartbeat(args) => task(&args.id),
        Command::Checkpoint(args) => task(&args.id),
        Command::Done(args) => task(&args.id),
        Command::Cancel(args) => task(&args.id),
        Command::Reopen(args) => task(&args.id),
        Command::Transition(args) => task(&args.id),
        Command::Update(args) => task(&args.id),
        Command::Delete(args) => task(&args.id),
        Command::Depend(args) => task(&args.id),
        Command::Undepend(args) => task(&args.id),
        Command::Label(args) => match &args.command {
            LabelCommand::Add(inner) | LabelCommand::Remove(inner) => task(&inner.id),
            LabelCommand::List => return Err(unsupported("orbit label list")),
        },
        Command::Property(args) => match &args.command {
            PropertyCommand::Set(inner) => task(&inner.id),
            PropertyCommand::Remove(inner) => task(&inner.id),
        },
        Command::Comment(args) => match &args.command {
            CommentCommand::Add(inner) => task(&inner.id),
            CommentCommand::List(_) => return Err(unsupported("orbit comment list")),
        },
        Command::Link(args) => match &args.command {
            LinkCommand::Add(inner) => task(&inner.id),
            LinkCommand::Remove(inner) => task(&inner.id),
            LinkCommand::List(_) => return Err(unsupported("orbit link list")),
        },
        Command::Ready(args) => {
            if !args.claim {
                return Err(OrbitError::invalid_argument(
                    "--if-version requires --claim; the version guards the Task `ready` picks",
                ));
            }
            VersionTarget::ReadyClaim
        }
        Command::Batch(_) => {
            return Err(OrbitError::invalid_argument(
                "--if-version is not supported by `orbit batch`; set `if_version` on each operation instead",
            ));
        }
        Command::Triage(args) => match &args.command {
            crate::cli::TriageCommand::Apply(inner) => task(&inner.task),
            _ => return Err(unsupported("orbit triage context|review")),
        },
        _ => return Err(unsupported(name(command))),
    })
}

pub fn run(cli: Cli) -> Result<crate::output::CommandOutput> {
    let Cli {
        json,
        dir,
        no_sync,
        if_version,
        idempotency_key,
        command,
    } = cli;
    if no_sync {
        crate::remote::auto_push::suppress();
        crate::board_host::suppress();
    }

    if json && matches!(&command, Command::Prime(args) if args.hook_json) {
        return Err(OrbitError::invalid_argument(
            "--json and --hook-json are mutually exclusive",
        ));
    }

    let concurrency = Concurrency {
        target: if_version
            .as_deref()
            .map(|_| version_target(&command))
            .transpose()?,
        if_version,
        idempotency_key,
    };

    match command {
        Command::Migrate(args) => {
            concurrency.reject_idempotency("orbit migrate")?;
            match &args.command {
                Some(crate::cli::MigrateCommand::Model(model_args)) => {
                    crate::model_migrate::cmd_model(model_args, dir.as_deref())
                }
                None => crate::board_migrate::cmd_migrate(&args, dir.as_deref()),
            }
        }
        Command::Init(args) => {
            concurrency.reject_idempotency("orbit init")?;
            cmd_init(args, dir.as_deref())
        }
        Command::Repo(args) => {
            let exclusive = !matches!(args.command, RepoCommand::List);
            with_board(dir.as_deref(), exclusive, &concurrency, |board| {
                cmd_repo(board, args)
            })
        }
        Command::Slug(args) => {
            let exclusive = matches!(args.command, SlugCommand::Migrate(_));
            with_board(dir.as_deref(), exclusive, &concurrency, |board| {
                cmd_slug(board, args)
            })
        }
        Command::Add(args) => with_task_board(dir.as_deref(), true, &concurrency, |board| {
            cmd_add(board, args)
        }),
        Command::List(args) => with_board(dir.as_deref(), false, &concurrency, |board| {
            cmd_list(board, args)
        }),
        Command::Ready(args) => {
            let exclusive = args.claim;
            let if_version = match &concurrency.target {
                Some(VersionTarget::ReadyClaim) => concurrency.if_version.as_deref(),
                _ => None,
            };
            with_task_board(dir.as_deref(), exclusive, &concurrency, |board| {
                cmd_ready(board, args, if_version)
            })
        }
        Command::Show(args) => with_board(dir.as_deref(), false, &concurrency, |board| {
            cmd_show(board, args)
        }),
        Command::Tree(args) => with_board(dir.as_deref(), false, &concurrency, |board| {
            cmd_tree(board, args)
        }),
        Command::Claim(args) => with_task_board(dir.as_deref(), true, &concurrency, |board| {
            cmd_claim(board, args)
        }),
        Command::Assign(args) => with_task_board(dir.as_deref(), true, &concurrency, |board| {
            cmd_assign(board, args)
        }),
        Command::Release(args) => with_task_board(dir.as_deref(), true, &concurrency, |board| {
            cmd_release(board, args)
        }),
        Command::Heartbeat(args) => with_board(dir.as_deref(), true, &concurrency, |board| {
            cmd_heartbeat(board, args)
        }),
        Command::Checkpoint(args) => with_board(dir.as_deref(), true, &concurrency, |board| {
            cmd_checkpoint(board, args)
        }),
        Command::Done(args) => with_task_board(dir.as_deref(), true, &concurrency, |board| {
            cmd_done(board, args)
        }),
        Command::Cancel(args) => with_task_board(dir.as_deref(), true, &concurrency, |board| {
            cmd_cancel(board, args)
        }),
        Command::Reopen(args) => with_task_board(dir.as_deref(), true, &concurrency, |board| {
            cmd_reopen(board, args)
        }),
        Command::Transition(args) => with_task_board(dir.as_deref(), true, &concurrency, |board| {
            cmd_transition(board, args)
        }),
        Command::Update(args) => with_task_board(dir.as_deref(), true, &concurrency, |board| {
            cmd_update(board, args)
        }),
        Command::Label(args) => {
            let exclusive = !matches!(args.command, LabelCommand::List);
            with_task_board(dir.as_deref(), exclusive, &concurrency, |board| {
                cmd_label(board, args)
            })
        }
        Command::Batch(args) => {
            let exclusive = !args.dry_run;
            with_task_board(dir.as_deref(), exclusive, &concurrency, |board| {
                cmd_batch(board, args)
            })
        }
        Command::Property(args) => with_task_board(dir.as_deref(), true, &concurrency, |board| {
            cmd_property(board, args)
        }),
        Command::Comment(args) => {
            let exclusive = matches!(args.command, CommentCommand::Add(_));
            with_board(dir.as_deref(), exclusive, &concurrency, |board| {
                cmd_comment(board, args)
            })
        }
        Command::Depend(args) => with_board(dir.as_deref(), true, &concurrency, |board| {
            cmd_depend(board, args)
        }),
        Command::Undepend(args) => with_board(dir.as_deref(), true, &concurrency, |board| {
            cmd_undepend(board, args)
        }),
        Command::Link(args) => {
            let exclusive = !matches!(args.command, LinkCommand::List(_));
            with_board(dir.as_deref(), exclusive, &concurrency, |board| {
                cmd_link(board, args)
            })
        }
        Command::Delete(args) => with_board(dir.as_deref(), true, &concurrency, |board| {
            cmd_delete(board, args)
        }),
        Command::Prime(args) => prime_with_host_sync(dir.as_deref(), args, &concurrency),
        Command::Export => with_board(dir.as_deref(), false, &concurrency, cmd_export),
        Command::Setup(args) => {
            concurrency.reject_idempotency("orbit setup")?;
            crate::setup::run(args, dir.as_deref())
        }
        Command::Mcp => {
            concurrency.reject_idempotency("orbit mcp")?;
            crate::mcp::serve(dir.as_deref())
                .map(|()| crate::output::CommandOutput::new(Value::Null, "").with_silent())
        }
        Command::Doctor(args) => {
            concurrency.reject_idempotency("orbit doctor")?;
            crate::doctor::run(dir.as_deref(), args)
        }
        Command::Sync => with_board(dir.as_deref(), true, &concurrency, cmd_sync),
        Command::Capabilities => {
            concurrency.reject_idempotency("orbit capabilities")?;
            crate::capabilities::run(dir.as_deref())
        }
        Command::Schema(args) => {
            concurrency.reject_idempotency("orbit schema")?;
            crate::registry::cmd_schema(&args)
        }
        Command::Board(args) => match &args.command {
            crate::cli::BoardCommand::Host(host_args) => {
                if host_args.target.is_some() {
                    concurrency.reject_idempotency("orbit board host")?;
                    let board = crate::board_host::unlocked_for_network(dir.as_deref())?;
                    let mut output = board::cmd_host(&board, host_args)?;
                    output
                        .warnings
                        .extend(crate::board_host::auto_after_board(&board));
                    output.warnings.extend(board.warnings.iter().cloned());
                    Ok(output)
                } else if host_args.none {
                    with_board(dir.as_deref(), true, &concurrency, |board| {
                        board::cmd_host(board, host_args)
                    })
                } else {
                    with_board(dir.as_deref(), false, &concurrency, |board| {
                        board::cmd_host(board, host_args)
                    })
                }
            }
            crate::cli::BoardCommand::Sync(sync_args) => {
                concurrency.reject_idempotency("orbit board sync")?;
                // The command performs the sync itself; do not auto-sync again.
                crate::board_host::suppress();
                let board = crate::board_host::unlocked_for_network(dir.as_deref())?;
                let mut output = board::cmd_sync(&board, sync_args)?;
                output.warnings.extend(board.warnings.iter().cloned());
                Ok(output)
            }
            crate::cli::BoardCommand::Move(move_args) => {
                concurrency.reject_idempotency("orbit board move")?;
                // The old root is gone after the move; never auto-sync from it.
                crate::board_host::suppress();
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
                return with_board_read_only(dir.as_deref(), &concurrency, |board| {
                    crate::remote::commands::run(board, args)
                });
            }
            with_board(dir.as_deref(), true, &concurrency, |board| {
                crate::remote::commands::run(board, args)
            })
        }
        Command::Triage(args) => {
            // Only `triage apply` writes; context and review stay read-only so
            // `--idempotency-key` cannot replay a stale read as a write.
            let exclusive = matches!(args.command, crate::cli::TriageCommand::Apply(_));
            with_task_board(dir.as_deref(), exclusive, &concurrency, |board| {
                crate::triage::commands::run(board, args, json)
            })
        }
    }
}

/// `orbit remote` previews and read-only views run without the exclusive
/// Board lock; anything that writes configuration, the catalog, or Tasks
/// keeps the normal write path.
fn remote_is_read_only(args: &crate::cli::RemoteArgs) -> bool {
    use crate::cli::{RemoteCommand, RemoteIssuesCommand};
    match &args.command {
        RemoteCommand::Status(_) | RemoteCommand::List => true,
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

fn with_board<F>(
    dir: Option<&Path>,
    exclusive: bool,
    concurrency: &Concurrency,
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
    with_board_access(dir, mode, AutoPush::Off, concurrency, f)
}

/// Task commands that may push mapped changes after a successful mutation.
fn with_task_board<F>(
    dir: Option<&Path>,
    exclusive: bool,
    concurrency: &Concurrency,
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
    with_board_access(dir, mode, AutoPush::On, concurrency, f)
}

fn with_board_read_only<F>(
    dir: Option<&Path>,
    concurrency: &Concurrency,
    f: F,
) -> Result<crate::output::CommandOutput>
where
    F: FnOnce(&Board) -> Result<crate::output::CommandOutput>,
{
    with_board_access(
        dir,
        crate::board::AccessMode::Inspect,
        AutoPush::Off,
        concurrency,
        f,
    )
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
    concurrency: &Concurrency,
    f: F,
) -> Result<crate::output::CommandOutput>
where
    F: FnOnce(&Board) -> Result<crate::output::CommandOutput>,
{
    let write = matches!(mode, crate::board::AccessMode::Write);
    if !write && concurrency.idempotency_key.is_some() {
        return Err(OrbitError::invalid_argument(
            "--idempotency-key only applies to commands that write",
        ));
    }
    let access = crate::board::BoardAccess::open(dir, mode)?;
    let board = access.board();
    if write && let Some(key) = concurrency.idempotency_key.as_deref() {
        // A replay wins over --if-version: the recorded first result is the
        // answer the caller already saw for this key.
        if let Some(replay) = crate::concurrency::lookup(board, key)? {
            return Ok(replay);
        }
    }
    if write
        && let Some(expected) = concurrency.if_version.as_deref()
        && let Some(VersionTarget::Task(reference)) = &concurrency.target
    {
        crate::concurrency::check_version(board, reference, expected)?;
    }
    let snapshot = (auto_push == AutoPush::On
        && write
        && crate::remote::auto_push::enabled(board)
        && !crate::remote::auto_push::suppressed())
    .then(|| crate::remote::auto_push::snapshot(board));
    let mut output = f(board)?;
    if write && let Some(key) = concurrency.idempotency_key.as_deref() {
        // The mutation already committed; a record failure warns rather than
        // fails, or a retry would see an error for work that succeeded.
        if let Err(error) = crate::concurrency::store(board, key, &output) {
            output
                .warnings
                .push(format!("IDEMPOTENCY_NOT_STORED: {}", error.message));
        }
    }
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
    if write {
        let (board, warnings) = crate::board_host::auto_after_write(access);
        output.warnings.extend(warnings);
        output.warnings.extend(board.warnings.iter().cloned());
        return Ok(output);
    }
    let board = access.into_board();
    output.warnings.extend(board.warnings.iter().cloned());
    Ok(output)
}

/// `orbit prime` syncs the Board host first when `board.host_sync = auto`,
/// so the primed view already includes the host's changes. Failures surface
/// as warnings; prime itself never fails because of the host.
fn prime_with_host_sync(
    dir: Option<&Path>,
    args: crate::cli::PrimeArgs,
    concurrency: &Concurrency,
) -> Result<crate::output::CommandOutput> {
    let mut warnings = crate::board_host::auto_before_prime(dir)?;
    let mut output = with_board(dir, false, concurrency, |board| cmd_prime(board, args))?;
    warnings.append(&mut output.warnings);
    output.warnings = warnings;
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

/// Normalizes a Priority name against the Board's `board.priorities` list and
/// returns the configured spelling. Names are matched case-insensitively.
pub(crate) fn validate_priority(board: &Board, priority: &str) -> Result<String> {
    let name = priority.trim().to_lowercase();
    match board
        .config
        .board
        .priorities
        .iter()
        .find(|candidate| candidate.as_str() == name)
    {
        Some(configured) => Ok(configured.clone()),
        None => Err(OrbitError::invalid_argument(format!(
            "unknown priority `{priority}`; expected one of: {}",
            board.config.board.priorities.join(", ")
        ))),
    }
}

pub(crate) fn normalize_labels(labels: &[String]) -> Result<Vec<String>> {
    let mut normalized = Vec::with_capacity(labels.len());
    for label in labels {
        normalized.push(task::normalize_label(label)?);
    }
    normalized.sort();
    normalized.dedup();
    Ok(normalized)
}

fn normalize_repository_filter(repository: Option<&str>) -> Result<Option<String>> {
    repository
        .map(|repository| {
            let repository = repository.trim();
            if repository.is_empty() {
                return Err(OrbitError::invalid_argument(
                    "--repository must not be empty",
                ));
            }
            if !task::is_valid_repository(repository) {
                return Err(OrbitError::invalid_argument(format!(
                    "repository `{repository}` is not a valid Repository name"
                )));
            }
            Ok(repository.to_string())
        })
        .transpose()
}

fn matches_repository(task: &Task, repository: Option<&str>) -> bool {
    repository.is_none_or(|repository| {
        task.front
            .repository
            .as_deref()
            .is_some_and(|name| name.eq_ignore_ascii_case(repository))
    })
}

pub(crate) fn normalize_waiting(waiting_on: Option<String>) -> Result<Option<String>> {
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

pub(crate) fn parse_component_option(
    board: &Board,
    component: Option<String>,
) -> Result<Option<String>> {
    match component {
        None => Ok(None),
        Some(value) => Ok(Some(parse_component_value(board, &value)?)),
    }
}

/// Normalizes one Component filter value and validates it against the
/// Board's `board.components` list; `task::normalize_component` treats an
/// empty list as unrestricted.
fn parse_component_value(board: &Board, value: &str) -> Result<String> {
    task::normalize_component(value, &board.config.board.components)
}

/// Normalizes repeatable Component filter flags the same way as `--component`.
fn normalize_components(board: &Board, components: &[String]) -> Result<Vec<String>> {
    components
        .iter()
        .map(|component| parse_component_value(board, component))
        .collect()
}

pub(crate) fn resolve_body(
    body: Option<String>,
    file: Option<std::path::PathBuf>,
) -> Result<String> {
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

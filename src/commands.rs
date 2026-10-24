use chrono::{Duration as ChronoDuration, Utc};
use serde_json::{Value, json};
use std::collections::BTreeMap;
use std::io::Read;
use std::path::Path;

use crate::atomic;
use crate::board::Board;
use crate::cli::{
    AddArgs, CancelArgs, ClaimArgs, Cli, Command, CommentAddArgs, CommentArgs, CommentCommand,
    CommentListArgs, DeleteArgs, DependArgs, DoctorArgs, DoneArgs, HeartbeatArgs, InitArgs,
    LabelArgs, LabelCommand, LabelMutationArgs, LinkArgs, ListArgs, PrimeArgs, ReadyArgs,
    ReleaseArgs, ReopenArgs, ShowArgs, UndependArgs, UpdateArgs,
};
use crate::comment::{self, Comment};
use crate::error::{OrbitError, Result, code};
use crate::git::CommitOutcome;
use crate::graph::{self, Graph, Issue, LoadMode};
use crate::task::{self, Status, Task, TaskFrontmatter};

pub fn run(cli: Cli) -> Result<crate::output::CommandOutput> {
    let Cli { json, dir, command } = cli;

    if json && matches!(&command, Command::Prime(args) if args.hook_json) {
        return Err(OrbitError::invalid_argument(
            "--json and --hook-json are mutually exclusive",
        ));
    }

    match command {
        Command::Init(args) => cmd_init(args, dir.as_deref()),
        Command::Link(args) => cmd_link(args),
        Command::Unlink => cmd_unlink(),
        Command::Add(args) => with_board(dir.as_deref(), true, |board| cmd_add(board, args)),
        Command::List(args) => with_board(dir.as_deref(), false, |board| cmd_list(board, args)),
        Command::Ready(args) => {
            let exclusive = args.claim;
            with_board(dir.as_deref(), exclusive, |board| cmd_ready(board, args))
        }
        Command::Show(args) => with_board(dir.as_deref(), false, |board| cmd_show(board, args)),
        Command::Claim(args) => with_board(dir.as_deref(), true, |board| cmd_claim(board, args)),
        Command::Release(args) => {
            with_board(dir.as_deref(), true, |board| cmd_release(board, args))
        }
        Command::Heartbeat(args) => {
            with_board(dir.as_deref(), true, |board| cmd_heartbeat(board, args))
        }
        Command::Done(args) => with_board(dir.as_deref(), true, |board| cmd_done(board, args)),
        Command::Cancel(args) => with_board(dir.as_deref(), true, |board| cmd_cancel(board, args)),
        Command::Reopen(args) => with_board(dir.as_deref(), true, |board| cmd_reopen(board, args)),
        Command::Update(args) => with_board(dir.as_deref(), true, |board| cmd_update(board, args)),
        Command::Label(args) => {
            let exclusive = !matches!(args.command, LabelCommand::List);
            with_board(dir.as_deref(), exclusive, |board| cmd_label(board, args))
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
        Command::Setup(args) => crate::setup::run(args, dir.as_deref()),
        Command::Mcp => crate::mcp::serve(dir.as_deref())
            .map(|()| crate::output::CommandOutput::new(Value::Null, "").with_silent()),
        Command::Doctor(args) => {
            let exclusive = args.fix;
            with_board(dir.as_deref(), exclusive, |board| cmd_doctor(board, args))
        }
        Command::Sync => with_board(dir.as_deref(), true, cmd_sync),
    }
}

fn with_board<F>(dir: Option<&Path>, exclusive: bool, f: F) -> Result<crate::output::CommandOutput>
where
    F: FnOnce(&Board) -> Result<crate::output::CommandOutput>,
{
    let board = Board::discover(dir)?;
    let _lock = if exclusive {
        board.lock_exclusive()?
    } else {
        board.lock_shared()?
    };
    f(&board)
}

fn cmd_init(args: InitArgs, dir: Option<&Path>) -> Result<crate::output::CommandOutput> {
    let cwd = std::env::current_dir()
        .map_err(|error| OrbitError::io("resolve current directory", error))?;
    let in_repo = crate::git::is_git_repo(&cwd);
    if !in_repo && args.board.is_none() && dir.is_none() {
        return Err(OrbitError::new(
            code::NOT_A_REPOSITORY,
            "`orbit init` must run inside a Git repository; use `orbit init --board <dir>` to create a shared Board elsewhere",
        ));
    }
    let root = if in_repo {
        crate::git::repo_root()?
    } else {
        cwd.clone()
    };

    let existing_link = if in_repo { Board::read_link()? } else { None };

    if let Some(linked) = &existing_link
        && args.board.is_none()
        && dir.is_none()
        && !args.local
    {
        return Err(OrbitError::new(
            code::ALREADY_LINKED,
            format!(
                "this repository is linked to the shared Board at {}; run `orbit init --local` to create its own Board",
                linked.display()
            ),
        ));
    }

    let clients = crate::setup::selected_agents(&args, &root)?;

    let (board_root, created, external) = if let Some(board) = args.board.as_deref() {
        init_board_at(Some(board), args.print)?
    } else if let Some(dir) = dir {
        init_board_at(Some(dir), args.print)?
    } else {
        let local = Board::local_root()?;
        let created = !Board::is_board(&local);
        if !args.print {
            if created {
                Board::init(None)?;
            } else {
                Board::discover(Some(&local))?;
            }
            if existing_link.is_some() {
                Board::clear_link()?;
            }
        }
        (local, created, false)
    };

    let target_given = args.board.is_some() || dir.is_some();
    if target_given && !args.print && in_repo {
        Board::write_link(&board_root)?;
    }
    let linked = target_given && in_repo;
    let branch = crate::board::Config::load(&board_root)
        .map(|config| config.board.branch)
        .unwrap_or_else(|_| "orbit".to_string());

    let mut files = json!([]);
    let mut warnings = Vec::new();
    let mut setup_human = String::new();
    if !clients.is_empty() {
        let bin = crate::setup::resolve_mcp_bin(args.mcp_bin.as_deref())?;
        let output = crate::setup::install(
            &root,
            crate::setup::InstallOptions {
                clients: clients.clone(),
                hooks: args.hooks,
                hook_bin: if args.hooks { Some(bin.clone()) } else { None },
                mcp_bin: Some(bin),
                mcp_dir: if external {
                    Some(board_root.clone())
                } else {
                    None
                },
                print: args.print,
                remove: false,
            },
        )?;
        files = output.data["files"].clone();
        warnings = output.warnings;
        setup_human = output.human;
    }

    let mut human = if args.print {
        format!(
            "would initialize Board at {} (branch {branch}{})\n",
            board_root.display(),
            if external { ", external" } else { "" }
        )
    } else if created {
        format!(
            "initialized Board at {} (branch {branch}{})\n",
            board_root.display(),
            if external { ", external" } else { "" }
        )
    } else {
        format!(
            "Board already initialized at {}; reconciled agent integration\n",
            board_root.display()
        )
    };
    human.push_str(&setup_human);

    let mut output = crate::output::CommandOutput::new(
        json!({
            "board": board_root,
            "branch": branch,
            "external": external,
            "created": created,
            "preview": args.print,
            "linked": linked,
            "agents": clients.iter().map(|client| client.name()).collect::<Vec<_>>(),
            "files": files,
        }),
        human,
    );
    output.warnings = warnings;
    Ok(output)
}

fn init_board_at(dir: Option<&Path>, print: bool) -> Result<(std::path::PathBuf, bool, bool)> {
    let board_root = Board::resolve_root(dir)?;
    let created = !Board::is_board(&board_root);
    if !print {
        if created {
            Board::init(dir)?;
        } else {
            Board::discover(Some(&board_root))?;
        }
    }
    Ok((board_root, created, true))
}

fn cmd_link(args: LinkArgs) -> Result<crate::output::CommandOutput> {
    let cwd = std::env::current_dir()
        .map_err(|error| OrbitError::io("resolve current directory", error))?;
    if !crate::git::is_git_repo(&cwd) {
        return Err(OrbitError::new(
            code::NOT_A_REPOSITORY,
            "`orbit link` must run inside a Git repository",
        ));
    }

    let target = Board::resolve_root(Some(&args.path))?;
    let board = Board::discover(Some(&target))?;
    let local = Board::local_root()?;
    let shadowed = Board::is_board(&local) && local != board.root;

    Board::write_link(&board.root)?;

    let root = crate::git::repo_root()?;
    let mut warnings = Vec::new();
    if shadowed {
        warnings.push(format!(
            "the local Board at {} is now shadowed but was not deleted",
            local.display()
        ));
    }

    let mut refreshed = Vec::new();
    if !args.no_refresh {
        match crate::setup::refresh(&root, Some(&board.root)) {
            Ok(output) => {
                refreshed = output.data["files"]
                    .as_array()
                    .map(|files| {
                        files
                            .iter()
                            .filter(|file| {
                                matches!(
                                    file["action"].as_str(),
                                    Some("created" | "updated" | "removed")
                                )
                            })
                            .filter_map(|file| file["path"].as_str().map(str::to_string))
                            .collect::<Vec<_>>()
                    })
                    .unwrap_or_default();
                warnings.extend(output.warnings);
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
            "refreshed": refreshed,
        }),
        human,
    );
    output.warnings = warnings;
    Ok(output)
}

fn cmd_unlink() -> Result<crate::output::CommandOutput> {
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
        if local_exists {
            match crate::setup::refresh(&root, None) {
                Ok(output) => warnings.extend(output.warnings),
                Err(error) => warnings.push(format!(
                    "could not refresh agent configuration: {}; run `orbit setup <client>` to update MCP entries",
                    error.message
                )),
            }
        } else {
            warnings.push(
                "this repository has no local Board; run `orbit init` to create one".to_string(),
            );
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

fn cmd_add(board: &Board, args: AddArgs) -> Result<crate::output::CommandOutput> {
    let (mut graph, _) = Graph::load(board, LoadMode::Strict)?;

    let title = args.title.trim().to_string();
    if title.is_empty() {
        return Err(OrbitError::invalid_argument("title must not be empty"));
    }

    let priority = args.priority.unwrap_or(board.config.board.default_priority);
    validate_priority(priority)?;
    let labels = normalize_labels(&args.label)?;

    let mut depends_on = Vec::new();
    for dep in args.depends_on {
        if !graph.tasks.contains_key(&dep) {
            return Err(OrbitError::not_found(&dep));
        }
        if !depends_on.contains(&dep) {
            depends_on.push(dep);
        }
    }

    let waiting_on = normalize_waiting(args.waiting_on)?;
    let body = resolve_body(args.body, args.body_file)?;
    let id = generate_unique_id(board)?;
    let now = Utc::now();

    let task = Task {
        id: id.clone(),
        front: TaskFrontmatter {
            title: title.clone(),
            status: Status::Todo,
            priority,
            labels,
            depends_on,
            waiting_on,
            claimed_by: None,
            claimed_at: None,
            lease_until: None,
            created_at: now,
            updated_at: now,
            extra: Default::default(),
        },
        body,
    };

    let rendered = task::render_task(&task)?;
    atomic::write_atomic(&board.task_path(&id), rendered.as_bytes(), false)?;
    graph.tasks.insert(id.clone(), task.clone());

    task_output(
        board,
        &graph,
        &task,
        format!("created {id}  P{priority}  {title}"),
        &format!("orbit: add {id}"),
    )
}

fn cmd_list(board: &Board, args: ListArgs) -> Result<crate::output::CommandOutput> {
    let (graph, _) = Graph::load(board, LoadMode::Strict)?;
    if let Some(priority) = args.priority {
        validate_priority(priority)?;
    }
    let label_filter = args
        .label
        .as_deref()
        .map(task::normalize_label)
        .transpose()?;

    let mut tasks: Vec<&Task> = graph
        .tasks
        .values()
        .filter(|task| {
            if let Some(status) = args.status {
                if task.front.status != status {
                    return false;
                }
            } else if !args.all && task.front.status.is_terminal() {
                return false;
            }

            if let Some(priority) = args.priority
                && task.front.priority != priority
            {
                return false;
            }
            if let Some(claimed_by) = &args.claimed_by
                && task.front.claimed_by.as_deref() != Some(claimed_by.as_str())
            {
                return false;
            }
            if let Some(label) = &label_filter
                && !task.front.labels.iter().any(|candidate| candidate == label)
            {
                return false;
            }
            if args.unlabeled && !task.front.labels.is_empty() {
                return false;
            }
            !args.waiting || task.front.waiting_on.is_some()
        })
        .collect();

    tasks.sort_by(|left, right| {
        (
            left.front.priority,
            left.front.created_at.timestamp_millis(),
            left.id.as_str(),
        )
            .cmp(&(
                right.front.priority,
                right.front.created_at.timestamp_millis(),
                right.id.as_str(),
            ))
    });

    let summaries: Vec<Value> = tasks
        .iter()
        .map(|task| graph::task_summary(&graph, task))
        .collect();
    let count = summaries.len();
    let human = if tasks.is_empty() {
        "no tasks".to_string()
    } else {
        tasks
            .iter()
            .copied()
            .map(format_summary)
            .collect::<Vec<_>>()
            .join("\n")
    };

    Ok(
        crate::output::CommandOutput::new(json!({ "tasks": summaries, "count": count }), human)
            .with_meta(json!({ "count": count })),
    )
}

fn cmd_ready(board: &Board, args: ReadyArgs) -> Result<crate::output::CommandOutput> {
    let (mut graph, _) = Graph::load(board, LoadMode::Strict)?;

    if args.claim {
        let agent = resolve_agent(args.by)?;
        let ready = graph.ready_tasks();
        if ready.is_empty() {
            return Ok(
                crate::output::CommandOutput::new(Value::Null, "no ready tasks")
                    .with_meta(json!({ "reason": "NO_READY_TASKS" })),
            );
        }

        let id = ready[0].id.clone();
        claim_task(
            &mut graph,
            &id,
            &agent,
            false,
            board.config.board.lease_minutes,
        )?;
        let task = graph.get(&id)?.clone();
        write_task(board, &task)?;

        let lease = task
            .front
            .lease_until
            .as_ref()
            .map(|until| until.to_rfc3339())
            .unwrap_or_default();
        let mut output = task_output(
            board,
            &graph,
            &task,
            format!("claimed {id} by {agent} until {lease}"),
            &format!("orbit: claim {id}"),
        )?;
        output.meta = Some(json!({ "claimed": true }));
        return Ok(output);
    }

    let mut ready = graph.ready_tasks();
    if let Some(limit) = args.limit {
        ready.truncate(limit);
    }

    let summaries: Vec<Value> = ready
        .iter()
        .map(|task| graph::task_summary(&graph, task))
        .collect();
    let count = summaries.len();
    let human = if ready.is_empty() {
        "no ready tasks".to_string()
    } else {
        ready
            .iter()
            .map(format_summary)
            .collect::<Vec<_>>()
            .join("\n")
    };

    Ok(
        crate::output::CommandOutput::new(json!({ "tasks": summaries, "count": count }), human)
            .with_meta(json!({ "count": count })),
    )
}

fn cmd_show(board: &Board, args: ShowArgs) -> Result<crate::output::CommandOutput> {
    let (graph, _) = Graph::load(board, LoadMode::Strict)?;
    let task = graph.get(&args.id)?;
    let comments = board.read_comments(&args.id)?;
    Ok(crate::output::CommandOutput::new(
        graph::task_detail_with_comments(&graph, task, &comments),
        format_detail(&graph, task, &comments),
    ))
}

fn cmd_claim(board: &Board, args: ClaimArgs) -> Result<crate::output::CommandOutput> {
    let (mut graph, _) = Graph::load(board, LoadMode::Strict)?;
    let agent = resolve_agent(args.by)?;

    claim_task(
        &mut graph,
        &args.id,
        &agent,
        args.steal,
        board.config.board.lease_minutes,
    )?;
    let task = graph.get(&args.id)?.clone();
    write_task(board, &task)?;

    let lease = task
        .front
        .lease_until
        .as_ref()
        .map(|until| until.to_rfc3339())
        .unwrap_or_default();
    task_output(
        board,
        &graph,
        &task,
        format!("claimed {} by {agent} until {lease}", args.id),
        &format!("orbit: claim {}", args.id),
    )
}

fn cmd_release(board: &Board, args: ReleaseArgs) -> Result<crate::output::CommandOutput> {
    let (mut graph, _) = Graph::load(board, LoadMode::Strict)?;
    let caller = optional_agent(args.by);

    {
        let task = graph.get(&args.id)?;
        if task.front.status.is_terminal() {
            return Err(invalid_transition(format!(
                "{} is {}; release only applies to active tasks",
                args.id,
                task.front.status.as_str()
            )));
        }
        if let Some(holder) = &task.front.claimed_by
            && !args.force
        {
            let caller = caller
                .as_deref()
                .ok_or_else(|| not_claim_holder(&args.id, holder))?;
            if caller != holder {
                return Err(not_claim_holder(&args.id, holder));
            }
        }
    }

    let changed = {
        let task = graph.get(&args.id)?;
        task.front.claimed_by.is_some() || task.front.status == Status::InProgress
    };

    if !changed {
        return Ok(crate::output::CommandOutput::new(
            json!({ "id": args.id, "changed": false }),
            format!("{} was not claimed", args.id),
        ));
    }

    let task = {
        let now = Utc::now();
        let task = graph.get_mut(&args.id)?;
        task.front.status = Status::Todo;
        task.front.claimed_by = None;
        task.front.claimed_at = None;
        task.front.lease_until = None;
        task.front.updated_at = now;
        task.clone()
    };
    write_task(board, &task)?;

    task_output(
        board,
        &graph,
        &task,
        format!("released {}", args.id),
        &format!("orbit: release {}", args.id),
    )
}

fn cmd_heartbeat(board: &Board, args: HeartbeatArgs) -> Result<crate::output::CommandOutput> {
    let (mut graph, _) = Graph::load(board, LoadMode::Strict)?;
    let agent = resolve_agent(args.by)?;
    let now = Utc::now();

    {
        let task = graph.get(&args.id)?;
        if task.front.status != Status::InProgress {
            return Err(invalid_transition(format!(
                "{} is {}; only in_progress tasks can be heartbeated",
                args.id,
                task.front.status.as_str()
            )));
        }
        let holder = task
            .front
            .claimed_by
            .as_deref()
            .ok_or_else(|| invalid_transition(format!("{} has no claim", args.id)))?;
        if holder != agent {
            return Err(not_claim_holder(&args.id, holder));
        }
    }

    let lease_until = {
        let task = graph.get_mut(&args.id)?;
        let lease_until = now + ChronoDuration::minutes(board.config.board.lease_minutes);
        task.front.lease_until = Some(lease_until);
        task.front.updated_at = now;
        lease_until
    };
    let task = graph.get(&args.id)?.clone();
    write_task(board, &task)?;

    Ok(crate::output::CommandOutput::new(
        json!({ "id": args.id, "lease_until": lease_until }),
        format!(
            "heartbeat {} extended to {}",
            args.id,
            lease_until.to_rfc3339()
        ),
    )
    .with_meta(json!({ "committed": false })))
}

fn cmd_done(board: &Board, args: DoneArgs) -> Result<crate::output::CommandOutput> {
    let (mut graph, _) = Graph::load(board, LoadMode::Strict)?;
    let caller = optional_agent(args.by.clone());
    let comment = resolve_optional_comment(args.comment, args.comment_file)?;
    let author = match &comment {
        Some(_) => Some(resolve_agent(args.by.clone())?),
        None => None,
    };
    let now = Utc::now();

    {
        let task = graph.get(&args.id)?;
        if task.front.status == Status::Done {
            if comment.is_none() {
                return Ok(crate::output::CommandOutput::new(
                    graph::task_detail(&graph, task),
                    format!("{} is already done", args.id),
                )
                .with_meta(json!({ "changed": false })));
            }

            let author = author.expect("a completion comment has an author");
            let comments = add_comment(
                board,
                &args.id,
                &author,
                comment.expect("a completion comment has a body"),
                now,
            )?;
            let task = {
                let task = graph.get_mut(&args.id)?;
                task.front.updated_at = now;
                task.clone()
            };
            write_task(board, &task)?;

            return task_output_with_comments(
                board,
                &graph,
                &task,
                &comments,
                format!("done {} (comment added)", args.id),
                &format!("orbit: done {}", args.id),
            );
        }
        if task.front.status == Status::Cancelled {
            return Err(invalid_transition(format!(
                "{} is cancelled; reopen it before completing it",
                args.id
            )));
        }

        let blocking = graph.blocking_ids(task);
        if !blocking.is_empty() {
            return Err(dependency_blocked(&args.id, &blocking));
        }

        if let Some(holder) = &task.front.claimed_by {
            let live = task
                .front
                .lease_until
                .as_ref()
                .is_some_and(|until| *until > now);
            if live && !args.force {
                let caller = caller
                    .as_deref()
                    .ok_or_else(|| not_claim_holder(&args.id, holder))?;
                if caller != holder {
                    return Err(not_claim_holder(&args.id, holder));
                }
            }
        }
    }

    let task = {
        let task = graph.get_mut(&args.id)?;
        task.front.status = Status::Done;
        task.front.claimed_by = None;
        task.front.claimed_at = None;
        task.front.lease_until = None;
        task.front.waiting_on = None;
        task.front.updated_at = now;
        task.clone()
    };
    write_task(board, &task)?;

    if let (Some(author), Some(body)) = (author.as_deref(), comment) {
        match add_comment(board, &args.id, author, body, now) {
            Ok(comments) => {
                return task_output_with_comments(
                    board,
                    &graph,
                    &task,
                    &comments,
                    format!("done {} (comment added)", args.id),
                    &format!("orbit: done {}", args.id),
                );
            }
            Err(error) => {
                let mut output = task_output(
                    board,
                    &graph,
                    &task,
                    format!("done {}", args.id),
                    &format!("orbit: done {}", args.id),
                )?;
                output = output.with_warning(format!("COMMENT_FAILED: {}", error.message));
                return Ok(output.with_exit_code(1));
            }
        }
    }

    task_output(
        board,
        &graph,
        &task,
        format!("done {}", args.id),
        &format!("orbit: done {}", args.id),
    )
}

fn cmd_cancel(board: &Board, args: CancelArgs) -> Result<crate::output::CommandOutput> {
    let (mut graph, _) = Graph::load(board, LoadMode::Strict)?;
    let caller = optional_agent(args.by.clone());
    let comment = resolve_optional_comment(args.comment, args.comment_file)?;
    let author = match &comment {
        Some(_) => Some(resolve_agent(args.by.clone())?),
        None => None,
    };
    let now = Utc::now();

    {
        let task = graph.get(&args.id)?;
        if task.front.status == Status::Cancelled {
            if comment.is_none() {
                return Ok(crate::output::CommandOutput::new(
                    graph::task_detail(&graph, task),
                    format!("{} is already cancelled", args.id),
                )
                .with_meta(json!({ "changed": false })));
            }

            let author = author.expect("a cancellation comment has an author");
            let comments = add_comment(
                board,
                &args.id,
                &author,
                comment.expect("a cancellation comment has a body"),
                now,
            )?;
            let task = {
                let task = graph.get_mut(&args.id)?;
                task.front.updated_at = now;
                task.clone()
            };
            write_task(board, &task)?;

            return task_output_with_comments(
                board,
                &graph,
                &task,
                &comments,
                format!("cancelled {} (comment added)", args.id),
                &format!("orbit: cancel {}", args.id),
            );
        }
        if task.front.status == Status::Done {
            return Err(invalid_transition(format!(
                "{} is done; reopen it before cancelling it",
                args.id
            )));
        }

        if let Some(holder) = &task.front.claimed_by {
            let live = task
                .front
                .lease_until
                .as_ref()
                .is_some_and(|until| *until > now);
            if live && !args.force {
                let caller = caller
                    .as_deref()
                    .ok_or_else(|| not_claim_holder(&args.id, holder))?;
                if caller != holder {
                    return Err(not_claim_holder(&args.id, holder));
                }
            }
        }
    }

    let task = {
        let task = graph.get_mut(&args.id)?;
        task.front.status = Status::Cancelled;
        task.front.claimed_by = None;
        task.front.claimed_at = None;
        task.front.lease_until = None;
        task.front.waiting_on = None;
        task.front.updated_at = now;
        task.clone()
    };
    write_task(board, &task)?;

    if let (Some(author), Some(body)) = (author.as_deref(), comment) {
        match add_comment(board, &args.id, author, body, now) {
            Ok(comments) => {
                return task_output_with_comments(
                    board,
                    &graph,
                    &task,
                    &comments,
                    format!("cancelled {} (comment added)", args.id),
                    &format!("orbit: cancel {}", args.id),
                );
            }
            Err(error) => {
                let mut output = task_output(
                    board,
                    &graph,
                    &task,
                    format!("cancelled {}", args.id),
                    &format!("orbit: cancel {}", args.id),
                )?;
                output = output.with_warning(format!("COMMENT_FAILED: {}", error.message));
                return Ok(output.with_exit_code(1));
            }
        }
    }

    task_output(
        board,
        &graph,
        &task,
        format!("cancelled {}", args.id),
        &format!("orbit: cancel {}", args.id),
    )
}

fn cmd_reopen(board: &Board, args: ReopenArgs) -> Result<crate::output::CommandOutput> {
    let (mut graph, _) = Graph::load(board, LoadMode::Strict)?;

    {
        let task = graph.get(&args.id)?;
        if !task.front.status.is_terminal() {
            return Err(invalid_transition(format!(
                "{} is {}; only terminal tasks can be reopened",
                args.id,
                task.front.status.as_str()
            )));
        }

        let done_dependents: Vec<String> = graph
            .dependents(&args.id)
            .iter()
            .filter(|dependent| dependent.front.status == Status::Done)
            .map(|dependent| dependent.id.clone())
            .collect();
        if !done_dependents.is_empty() && !args.force {
            return Err(invalid_transition(format!(
                "{} has done dependents: {}",
                args.id,
                done_dependents.join(", ")
            ))
            .with_details(json!({ "done_dependents": done_dependents })));
        }
    }

    let task = {
        let task = graph.get_mut(&args.id)?;
        task.front.status = Status::Todo;
        task.front.claimed_by = None;
        task.front.claimed_at = None;
        task.front.lease_until = None;
        task.front.waiting_on = None;
        task.front.updated_at = Utc::now();
        task.clone()
    };
    write_task(board, &task)?;

    task_output(
        board,
        &graph,
        &task,
        format!("reopened {}", args.id),
        &format!("orbit: reopen {}", args.id),
    )
}

fn cmd_update(board: &Board, args: UpdateArgs) -> Result<crate::output::CommandOutput> {
    let (mut graph, _) = Graph::load(board, LoadMode::Strict)?;

    let has_changes = args.title.is_some()
        || args.body.is_some()
        || args.body_file.is_some()
        || args.priority.is_some()
        || args.waiting_on.is_some()
        || args.clear_waiting;
    if !has_changes {
        return Err(OrbitError::invalid_argument(
            "nothing to update; pass --title, --body, --priority, or --waiting-on",
        ));
    }

    if let Some(priority) = args.priority {
        validate_priority(priority)?;
    }
    let body = if args.body.is_some() || args.body_file.is_some() {
        Some(resolve_body(args.body, args.body_file)?)
    } else {
        None
    };
    let waiting_on = normalize_waiting(args.waiting_on)?;

    let task = {
        let task = graph.get_mut(&args.id)?;
        if let Some(title) = args.title {
            let title = title.trim().to_string();
            if title.is_empty() {
                return Err(OrbitError::invalid_argument("title must not be empty"));
            }
            task.front.title = title;
        }
        if let Some(body) = body {
            task.body = body;
        }
        if let Some(priority) = args.priority {
            task.front.priority = priority;
        }
        if args.clear_waiting {
            task.front.waiting_on = None;
        } else if waiting_on.is_some() {
            task.front.waiting_on = waiting_on;
        }
        task.front.updated_at = Utc::now();
        task.clone()
    };

    write_task(board, &task)?;
    task_output(
        board,
        &graph,
        &task,
        format!("updated {}", args.id),
        &format!("orbit: update {}", args.id),
    )
}

fn cmd_label(board: &Board, args: LabelArgs) -> Result<crate::output::CommandOutput> {
    match args.command {
        LabelCommand::Add(args) => mutate_labels(board, args, true),
        LabelCommand::Remove(args) => mutate_labels(board, args, false),
        LabelCommand::List => list_labels(board),
    }
}

fn mutate_labels(
    board: &Board,
    args: LabelMutationArgs,
    add: bool,
) -> Result<crate::output::CommandOutput> {
    let labels = normalize_labels(&args.labels)?;
    let (mut graph, _) = Graph::load(board, LoadMode::Strict)?;
    let now = Utc::now();

    let (task, changed) = {
        let task = graph.get_mut(&args.id)?;
        let before = task.front.labels.clone();
        if add {
            for label in &labels {
                if !task.front.labels.contains(label) {
                    task.front.labels.push(label.clone());
                }
            }
        } else {
            task.front.labels.retain(|label| !labels.contains(label));
        }
        task.front.labels.sort();
        task.front.labels.dedup();
        let changed = task.front.labels != before;
        if changed {
            task.front.updated_at = now;
        }
        (task.clone(), changed)
    };

    if !changed {
        return Ok(crate::output::CommandOutput::new(
            json!({ "id": args.id, "labels": task.front.labels, "changed": false }),
            format!("{} labels unchanged", args.id),
        ));
    }

    write_task(board, &task)?;
    let outcome = board.commit(
        &format!(
            "orbit: label {} {}",
            if add { "add" } else { "remove" },
            args.id
        ),
        false,
    )?;
    let mut output = crate::output::CommandOutput::new(
        json!({ "id": args.id, "labels": task.front.labels, "changed": true }),
        format!(
            "{} {}: {}",
            if add { "added to" } else { "removed from" },
            args.id,
            labels.join(", ")
        ),
    );
    if let CommitOutcome::Failed(message) = outcome {
        output = output.with_warning(format!("COMMIT_FAILED: {message}"));
    }
    Ok(output)
}

fn list_labels(board: &Board) -> Result<crate::output::CommandOutput> {
    let (graph, _) = Graph::load(board, LoadMode::Strict)?;
    let mut counts: BTreeMap<String, usize> = BTreeMap::new();
    for task in graph.tasks.values() {
        for label in &task.front.labels {
            *counts.entry(label.clone()).or_default() += 1;
        }
    }

    let labels: Vec<Value> = counts
        .iter()
        .map(|(label, count)| json!({ "label": label, "count": count }))
        .collect();
    let human = if labels.is_empty() {
        "no labels".to_string()
    } else {
        labels
            .iter()
            .map(|value| {
                format!(
                    "{}  {}",
                    value["label"].as_str().unwrap_or("?"),
                    value["count"].as_u64().unwrap_or(0)
                )
            })
            .collect::<Vec<_>>()
            .join("\n")
    };

    Ok(
        crate::output::CommandOutput::new(json!({ "labels": labels }), human)
            .with_meta(json!({ "count": counts.len() })),
    )
}

fn cmd_comment(board: &Board, args: CommentArgs) -> Result<crate::output::CommandOutput> {
    match args.command {
        CommentCommand::Add(args) => cmd_comment_add(board, args),
        CommentCommand::List(args) => cmd_comment_list(board, args),
    }
}

fn cmd_comment_add(board: &Board, args: CommentAddArgs) -> Result<crate::output::CommandOutput> {
    let (mut graph, _) = Graph::load(board, LoadMode::Strict)?;
    graph.get(&args.id)?;
    let author = resolve_agent(args.by)?;
    let body = resolve_body(args.body, args.body_file)?;

    let now = Utc::now();
    let comments = add_comment(board, &args.id, &author, body, now)?;
    let comment = comments.last().expect("a comment was appended").clone();
    let task = {
        let task = graph.get_mut(&args.id)?;
        task.front.updated_at = now;
        task.clone()
    };
    write_task(board, &task)?;

    let outcome = board.commit(&format!("orbit: comment {}", args.id), false)?;
    let mut output = crate::output::CommandOutput::new(
        json!({
            "id": args.id,
            "comment": comment,
            "count": comments.len()
        }),
        format!("commented {} as {author}", args.id),
    );
    if let CommitOutcome::Failed(message) = outcome {
        output = output.with_warning(format!("COMMIT_FAILED: {message}"));
    }
    Ok(output)
}

fn cmd_comment_list(board: &Board, args: CommentListArgs) -> Result<crate::output::CommandOutput> {
    let (graph, _) = Graph::load(board, LoadMode::Strict)?;
    graph.get(&args.id)?;
    let comments = board.read_comments(&args.id)?;
    let human = if comments.is_empty() {
        format!("{} has no comments", args.id)
    } else {
        comments
            .iter()
            .map(format_comment)
            .collect::<Vec<_>>()
            .join("\n\n")
    };

    Ok(
        crate::output::CommandOutput::new(json!({ "id": args.id, "comments": comments }), human)
            .with_meta(json!({ "count": comments.len() })),
    )
}

fn format_comment(comment: &Comment) -> String {
    format!(
        "{}  {}\n{}",
        comment.created_at.to_rfc3339(),
        comment.author,
        comment.body
    )
}

fn cmd_depend(board: &Board, args: DependArgs) -> Result<crate::output::CommandOutput> {
    let (mut graph, _) = Graph::load(board, LoadMode::Strict)?;

    {
        let task = graph.get(&args.id)?;
        if task.front.status.is_terminal() {
            return Err(invalid_transition(format!(
                "{} is {}; cannot add a dependency to a terminal task",
                args.id,
                task.front.status.as_str()
            )));
        }
        graph.get(&args.dep_id)?;
    }

    if args.id == args.dep_id {
        return Err(cycle_error(&[args.id.clone(), args.dep_id.clone()]));
    }

    if graph
        .get(&args.id)?
        .front
        .depends_on
        .iter()
        .any(|dep| dep == &args.dep_id)
    {
        let task = graph.get(&args.id)?;
        return Ok(crate::output::CommandOutput::new(
            graph::task_detail(&graph, task),
            format!("{} already depends on {}", args.id, args.dep_id),
        )
        .with_meta(json!({ "changed": false })));
    }

    if let Some(cycle) = graph.would_create_cycle(&args.id, &args.dep_id) {
        return Err(cycle_error(&cycle));
    }

    let task = {
        let task = graph.get_mut(&args.id)?;
        task.front.depends_on.push(args.dep_id.clone());
        task.front.updated_at = Utc::now();
        task.clone()
    };
    write_task(board, &task)?;

    task_output(
        board,
        &graph,
        &task,
        format!("{} now depends on {}", args.id, args.dep_id),
        &format!("orbit: depend {}", args.id),
    )
}

fn cmd_undepend(board: &Board, args: UndependArgs) -> Result<crate::output::CommandOutput> {
    let (mut graph, _) = Graph::load(board, LoadMode::Lenient)?;

    let changed = {
        let task = graph.get(&args.id)?;
        task.front.depends_on.iter().any(|dep| dep == &args.dep_id)
    };
    if !changed {
        let task = graph.get(&args.id)?;
        return Ok(crate::output::CommandOutput::new(
            graph::task_detail(&graph, task),
            format!("{} does not depend on {}", args.id, args.dep_id),
        )
        .with_meta(json!({ "changed": false })));
    }

    let task = {
        let task = graph.get_mut(&args.id)?;
        task.front.depends_on.retain(|dep| dep != &args.dep_id);
        task.front.updated_at = Utc::now();
        task.clone()
    };
    write_task(board, &task)?;

    task_output(
        board,
        &graph,
        &task,
        format!("{} no longer depends on {}", args.id, args.dep_id),
        &format!("orbit: undepend {}", args.id),
    )
}

fn cmd_delete(board: &Board, args: DeleteArgs) -> Result<crate::output::CommandOutput> {
    let (mut graph, _) = Graph::load(board, LoadMode::Lenient)?;
    let caller = optional_agent(args.by);
    let now = Utc::now();

    {
        let task = graph.get(&args.id)?;
        if let Some(holder) = &task.front.claimed_by {
            let live = task
                .front
                .lease_until
                .as_ref()
                .is_some_and(|until| *until > now);
            if live && !args.force {
                let caller = caller
                    .as_deref()
                    .ok_or_else(|| not_claim_holder(&args.id, holder))?;
                if caller != holder {
                    return Err(not_claim_holder(&args.id, holder));
                }
            }
        }
    }

    let dependent_ids: Vec<String> = graph
        .dependents(&args.id)
        .iter()
        .map(|dependent| dependent.id.clone())
        .collect();
    if !dependent_ids.is_empty() && !args.force {
        return Err(OrbitError::new(
            code::DEPENDENCY_BLOCKED,
            format!("{} has dependents: {}", args.id, dependent_ids.join(", ")),
        )
        .with_details(json!({ "task": args.id, "dependents": dependent_ids })));
    }

    if args.force {
        for dependent in &dependent_ids {
            let updated = {
                let task = graph.get_mut(dependent)?;
                task.front.depends_on.retain(|dep| dep != &args.id);
                task.front.updated_at = now;
                task.clone()
            };
            write_task(board, &updated)?;
        }
    }

    let path = board.task_path(&args.id);
    match std::fs::remove_file(&path) {
        Ok(()) => {}
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => {
            return Err(OrbitError::io(format!("delete {}", path.display()), error));
        }
    }

    let comments_path = board.comment_path(&args.id);
    match std::fs::remove_file(&comments_path) {
        Ok(()) => {}
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => {
            return Err(OrbitError::io(
                format!("delete {}", comments_path.display()),
                error,
            ));
        }
    }

    let outcome = board.commit(&format!("orbit: delete {}", args.id), false)?;
    let mut output = crate::output::CommandOutput::new(
        json!({ "id": args.id, "deleted": true, "cleaned": dependent_ids }),
        format!("deleted {}", args.id),
    );
    if let CommitOutcome::Failed(message) = outcome {
        output.warnings.push(format!("COMMIT_FAILED: {message}"));
    }
    Ok(output)
}

fn cmd_prime(board: &Board, args: PrimeArgs) -> Result<crate::output::CommandOutput> {
    let (graph, _) = Graph::load(board, LoadMode::Strict)?;
    let counts = graph.counts();
    let agent = optional_agent(args.by).unwrap_or_else(|| "unknown (set ORBIT_AGENT)".to_string());
    let instructions = "Orbit is the task tracker for this repository.\n\
Use the CLI; never edit task files directly.\n\
- `orbit ready --claim --by <agent>` takes one ready task.\n\
- `orbit heartbeat <id> --by <agent>` extends the lease while working.\n\
- `orbit done <id> --by <agent> --comment \"note\"` completes it and records the note with the status change; `orbit cancel <id>` abandons it.\n\
- `orbit add \"title\" -p N --depends-on <id>` creates work.\n\
- `orbit depend <id> <dep-id>` links work; cycles and unfinished dependencies are rejected.\n\
- `orbit show <id>` reads the full body.";

    let ready: Vec<Value> = graph
        .ready_tasks()
        .iter()
        .take(10)
        .map(|task| graph::task_summary(&graph, task))
        .collect();
    let stale: Vec<Value> = graph
        .stale_tasks()
        .iter()
        .take(10)
        .map(|task| graph::task_summary(&graph, task))
        .collect();

    let mut human = String::new();
    human.push_str(instructions);
    human.push_str("\n\n");
    human.push_str(&format!("agent: {agent}\n"));
    human.push_str(&format!(
        "counts: total={} ready={} stale={} in_progress={} waiting={} blocked={}\n",
        counts.total,
        counts.ready,
        counts.stale,
        counts.in_progress,
        counts.waiting,
        counts.blocked
    ));
    human.push_str("ready:\n");
    if ready.is_empty() {
        human.push_str("  (none)\n");
    } else {
        for task in &ready {
            human.push_str(&format!("  {}\n", format_summary_value(task)));
        }
    }
    human.push_str("stale claims:\n");
    if stale.is_empty() {
        human.push_str("  (none)\n");
    } else {
        for task in &stale {
            human.push_str(&format!("  {}\n", format_summary_value(task)));
        }
    }

    let mut output = crate::output::CommandOutput::new(
        json!({
            "agent": agent,
            "instructions": instructions,
            "counts": counts,
            "ready": ready,
            "stale": stale,
        }),
        human,
    );

    if args.hook_json {
        let hook = json!({
            "hookSpecificOutput": {
                "hookEventName": "SessionStart",
                "additionalContext": output.human,
            }
        });
        output.raw_output = Some(serde_json::to_string(&hook).map_err(|error| {
            OrbitError::config(format!("serialize prime hook output: {error}"))
        })?);
    }

    Ok(output)
}

fn cmd_doctor(board: &Board, args: DoctorArgs) -> Result<crate::output::CommandOutput> {
    let (graph, mut issues) = Graph::load(board, LoadMode::Lenient)?;
    let mut warnings = Vec::new();

    match board.comment_ids() {
        Ok(ids) => {
            for id in ids {
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
                }
            }
        }
        Err(error) => {
            issues.push(Issue::new(code::BOARD_CORRUPT, None, error.message));
        }
    }

    for task in graph.stale_tasks() {
        warnings.push(format!(
            "{}: stale claim by {}",
            task.id,
            task.front.claimed_by.as_deref().unwrap_or("unknown")
        ));
    }

    for task in graph.tasks.values() {
        let has_holder = task.front.claimed_by.is_some();
        let has_times = task.front.claimed_at.is_some() || task.front.lease_until.is_some();
        if has_holder != has_times {
            warnings.push(format!("{}: claim metadata is incomplete", task.id));
        }
        if task.front.status == Status::Done {
            let unfinished = graph.blocking_ids(task);
            if !unfinished.is_empty() {
                warnings.push(format!(
                    "{} is done but has unfinished dependencies: {}",
                    task.id,
                    unfinished.join(", ")
                ));
            }
        }
    }

    match crate::git::status_porcelain(&board.root) {
        Ok(status) if !status.trim().is_empty() => {
            warnings.push("the board has uncommitted changes; run `orbit sync`".to_string());
        }
        Ok(_) => {}
        Err(error) => issues.push(Issue::new(code::BOARD_CORRUPT, None, error.message)),
    }

    match crate::git::current_branch(&board.root) {
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

    let mut stale_temps = atomic::stale_temps(&board.tasks_dir(), std::time::Duration::ZERO);
    stale_temps.extend(atomic::stale_temps(
        &board.comments_dir(),
        std::time::Duration::ZERO,
    ));
    if !stale_temps.is_empty() && !args.fix {
        warnings.push(format!(
            "{} stale temp file(s) in the board; run `orbit doctor --fix`",
            stale_temps.len()
        ));
    }

    let fixed = if args.fix {
        let mut fixed = atomic::clean_stale_temps(&board.tasks_dir(), std::time::Duration::ZERO)?;
        fixed.extend(atomic::clean_stale_temps(
            &board.comments_dir(),
            std::time::Duration::ZERO,
        )?);
        fixed
            .into_iter()
            .map(|path| path.display().to_string())
            .collect::<Vec<_>>()
    } else {
        Vec::new()
    };

    let orbit_name = if cfg!(windows) { "orbit.exe" } else { "orbit" };
    let running = std::env::current_exe()
        .ok()
        .and_then(|path| dunce::canonicalize(path).ok());
    let found =
        crate::setup::find_on_path(orbit_name).and_then(|path| dunce::canonicalize(path).ok());
    let on_path = match (&found, &running) {
        (Some(found), Some(running)) => found == running,
        (Some(_), None) => true,
        _ => false,
    };
    if !on_path {
        let repo_root =
            crate::git::repo_root().unwrap_or_else(|_| std::env::current_dir().unwrap_or_default());
        if crate::setup::has_orbit_hook(&repo_root) {
            warnings.push(
                "`orbit` is not the binary on PATH; the Claude SessionStart hook will not find it"
                    .to_string(),
            );
        }
    }

    let healthy = issues.is_empty();
    let mut human = String::new();
    if healthy {
        human.push_str("orbit doctor: healthy\n");
    } else {
        human.push_str(&format!("orbit doctor: {} issue(s)\n", issues.len()));
        for issue in &issues {
            human.push_str(&format!(
                "- [{}] {}{}\n",
                issue.code,
                issue
                    .task
                    .as_ref()
                    .map(|task| format!("{task}: "))
                    .unwrap_or_default(),
                issue.message
            ));
        }
    }
    human.push_str(&format!("tasks: {}\n", graph.tasks.len()));
    for warning in &warnings {
        human.push_str(&format!("warning: {warning}\n"));
    }
    for path in &fixed {
        human.push_str(&format!("fixed: {path}\n"));
    }

    let output = crate::output::CommandOutput::new(
        json!({
            "healthy": healthy,
            "tasks": graph.tasks.len(),
            "issues": issues,
            "warnings": warnings,
            "path_on_path": on_path,
            "fixed": fixed,
        }),
        human,
    )
    .with_exit_code(if healthy { 0 } else { 1 });
    Ok(output)
}

fn cmd_sync(board: &Board) -> Result<crate::output::CommandOutput> {
    match board.commit("orbit: sync", true)? {
        CommitOutcome::Committed => Ok(crate::output::CommandOutput::new(
            json!({ "committed": true }),
            "committed pending board changes",
        )),
        CommitOutcome::NoChanges => Ok(crate::output::CommandOutput::new(
            json!({ "committed": false }),
            "no board changes to commit",
        )),
        CommitOutcome::Skipped => Ok(crate::output::CommandOutput::new(
            json!({ "committed": false }),
            "auto-commit is disabled",
        )),
        CommitOutcome::Failed(message) => Err(OrbitError::git(message)),
    }
}

fn claim_task(
    graph: &mut Graph,
    id: &str,
    agent: &str,
    steal: bool,
    lease_minutes: i64,
) -> Result<()> {
    let now = Utc::now();

    {
        let task = graph.get(id)?;
        if task.front.status.is_terminal() {
            return Err(invalid_transition(format!(
                "{id} is {}; it cannot be claimed",
                task.front.status.as_str()
            )));
        }
        if task.front.waiting_on.is_some() {
            return Err(OrbitError::new(
                code::WAITING_EXTERNAL,
                format!("{id} is waiting on an external blocker"),
            )
            .with_details(json!({ "task": id, "waiting_on": task.front.waiting_on })));
        }

        let holds = graph.hold_labels(task);
        if !holds.is_empty() {
            return Err(OrbitError::new(
                code::LABEL_BLOCKED,
                format!("{id} has hold labels: {}", holds.join(", ")),
            )
            .with_details(json!({ "task": id, "labels": holds })));
        }

        let blocking = graph.blocking_ids(task);
        if !blocking.is_empty() {
            return Err(dependency_blocked(id, &blocking));
        }

        if let Some(holder) = &task.front.claimed_by {
            let live = task
                .front
                .lease_until
                .as_ref()
                .is_some_and(|until| *until > now);
            if !steal {
                if live {
                    return Err(OrbitError::new(
                        code::ALREADY_CLAIMED,
                        format!("{id} is claimed by {holder}"),
                    )
                    .with_details(json!({
                        "task": id,
                        "holder": holder,
                        "lease_until": task.front.lease_until,
                    })));
                }
                return Err(OrbitError::new(
                    code::CLAIM_STALE,
                    format!("{id} has a stale claim by {holder}; use `orbit claim {id} --steal`"),
                )
                .with_details(json!({
                    "task": id,
                    "holder": holder,
                    "lease_until": task.front.lease_until,
                })));
            }
        }
    }

    let task = graph.get_mut(id)?;
    task.front.status = Status::InProgress;
    task.front.claimed_by = Some(agent.to_string());
    task.front.claimed_at = Some(now);
    task.front.lease_until = Some(now + ChronoDuration::minutes(lease_minutes));
    task.front.updated_at = now;
    Ok(())
}

fn write_task(board: &Board, task: &Task) -> Result<()> {
    let rendered = task::render_task(task)?;
    atomic::write_atomic(&board.task_path(&task.id), rendered.as_bytes(), true)
}

fn task_output(
    board: &Board,
    graph: &Graph,
    task: &Task,
    human: String,
    message: &str,
) -> Result<crate::output::CommandOutput> {
    let outcome = board.commit(message, false)?;
    let mut output = crate::output::CommandOutput::new(graph::task_detail(graph, task), human);
    if let CommitOutcome::Failed(message) = outcome {
        output = output.with_warning(format!("COMMIT_FAILED: {message}"));
    }
    Ok(output)
}

fn task_output_with_comments(
    board: &Board,
    graph: &Graph,
    task: &Task,
    comments: &[Comment],
    human: String,
    message: &str,
) -> Result<crate::output::CommandOutput> {
    let outcome = board.commit(message, false)?;
    let mut output = crate::output::CommandOutput::new(
        graph::task_detail_with_comments(graph, task, comments),
        human,
    );
    if let CommitOutcome::Failed(message) = outcome {
        output = output.with_warning(format!("COMMIT_FAILED: {message}"));
    }
    Ok(output)
}

fn add_comment(
    board: &Board,
    id: &str,
    author: &str,
    body: String,
    created_at: chrono::DateTime<Utc>,
) -> Result<Vec<Comment>> {
    comment::validate_body(&body)?;
    let comment = Comment {
        author: author.to_string(),
        created_at,
        body: body.trim_end_matches(['\r', '\n']).to_string(),
    };
    let mut comments = board.read_comments(id)?;
    comments.push(comment);
    board.write_comments(id, &comments)?;
    Ok(comments)
}

fn resolve_optional_comment(
    comment: Option<String>,
    comment_file: Option<std::path::PathBuf>,
) -> Result<Option<String>> {
    if comment.is_none() && comment_file.is_none() {
        return Ok(None);
    }
    let body = resolve_body(comment, comment_file)?;
    comment::validate_body(&body)?;
    Ok(Some(body))
}

fn generate_unique_id(board: &Board) -> Result<String> {
    let config = &board.config.id;
    for attempt in 0..50 {
        let length = if attempt < 40 {
            config.length
        } else {
            (config.length + 2).min(8)
        };
        let id = task::generate_id(&config.prefix, length);
        if !board.task_path(&id).exists() {
            return Ok(id);
        }
    }
    Err(OrbitError::new(
        code::ID_COLLISION,
        "could not generate a unique task id",
    ))
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

fn format_summary(task: &Task) -> String {
    let claim = task
        .front
        .claimed_by
        .as_ref()
        .map(|holder| format!("  claimed by {holder}"))
        .unwrap_or_default();
    let labels = if task.front.labels.is_empty() {
        String::new()
    } else {
        format!("  [{}]", task.front.labels.join(", "))
    };
    format!(
        "{}  {:<11}  P{}  {}{}{}",
        task.id,
        task.front.status.as_str(),
        task.front.priority,
        task.front.title,
        labels,
        claim
    )
}

fn format_summary_value(value: &Value) -> String {
    let id = value.get("id").and_then(Value::as_str).unwrap_or("?");
    let status = value.get("status").and_then(Value::as_str).unwrap_or("?");
    let priority = value.get("priority").and_then(Value::as_u64).unwrap_or(0);
    let title = value.get("title").and_then(Value::as_str).unwrap_or("?");
    let claim = value
        .get("claimed_by")
        .and_then(Value::as_str)
        .map(|holder| format!("  claimed by {holder}"))
        .unwrap_or_default();
    let labels = value
        .get("labels")
        .and_then(Value::as_array)
        .map(|labels| {
            let labels = labels.iter().filter_map(Value::as_str).collect::<Vec<_>>();
            if labels.is_empty() {
                String::new()
            } else {
                format!("  [{}]", labels.join(", "))
            }
        })
        .unwrap_or_default();
    format!("{id}  {status:<11}  P{priority}  {title}{labels}{claim}")
}

fn format_detail(graph: &Graph, task: &Task, comments: &[Comment]) -> String {
    let mut lines = Vec::new();
    lines.push(format!("{}  {}", task.id, task.front.title));
    lines.push(format!("status: {}", task.front.status.as_str()));
    lines.push(format!("priority: P{}", task.front.priority));
    if task.front.depends_on.is_empty() {
        lines.push("depends_on: -".to_string());
    } else {
        lines.push(format!("depends_on: {}", task.front.depends_on.join(", ")));
    }
    if let Some(waiting) = &task.front.waiting_on {
        lines.push(format!("waiting_on: {waiting}"));
    }
    if let Some(holder) = &task.front.claimed_by {
        let until = task
            .front
            .lease_until
            .as_ref()
            .map(|until| until.to_rfc3339())
            .unwrap_or_else(|| "unknown".to_string());
        lines.push(format!("claimed_by: {holder} (until {until})"));
    }
    lines.push(format!("ready: {}", graph.is_ready(task)));
    let blocking = graph.blocked_by(task);
    if !blocking.is_empty() {
        lines.push(format!("blocking: {}", blocking.join(", ")));
    }
    lines.push(format!(
        "created_at: {}",
        task.front.created_at.to_rfc3339()
    ));
    lines.push(format!(
        "updated_at: {}",
        task.front.updated_at.to_rfc3339()
    ));
    let dependents: Vec<&str> = graph
        .dependents(&task.id)
        .iter()
        .map(|dependent| dependent.id.as_str())
        .collect();
    if !dependents.is_empty() {
        lines.push(format!("dependents: {}", dependents.join(", ")));
    }
    if !comments.is_empty() {
        lines.push(format!("comments: {}", comments.len()));
        for comment in comments {
            lines.push(format!(
                "  [{}] {}",
                comment.created_at.to_rfc3339(),
                comment.author
            ));
            for line in comment.body.lines() {
                lines.push(format!("    {line}"));
            }
        }
    }
    lines.push(String::new());
    lines.push(task.body.clone());
    lines.join("\n")
}

fn invalid_transition(message: impl Into<String>) -> OrbitError {
    OrbitError::new(code::INVALID_TRANSITION, message)
}

fn dependency_blocked(id: &str, blocking: &[String]) -> OrbitError {
    OrbitError::new(
        code::DEPENDENCY_BLOCKED,
        format!("{id} has unfinished dependencies"),
    )
    .with_details(json!({ "task": id, "blocking": blocking }))
}

fn cycle_error(path: &[String]) -> OrbitError {
    OrbitError::new(
        code::CYCLE_DETECTED,
        format!("dependency cycle: {}", path.join(" -> ")),
    )
    .with_details(json!({ "cycle": path }))
}

fn not_claim_holder(id: &str, holder: &str) -> OrbitError {
    OrbitError::new(
        code::NOT_CLAIM_HOLDER,
        format!("{id} is claimed by {holder}"),
    )
    .with_details(json!({ "task": id, "holder": holder }))
}

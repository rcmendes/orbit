use chrono::{Duration as ChronoDuration, Utc};
use serde_json::{Value, json};
use std::collections::BTreeMap;
use std::io::{IsTerminal, Read};
use std::path::Path;

use crate::atomic;
use crate::board::Board;
use crate::cli::{
    AddArgs, CancelArgs, ClaimArgs, Cli, Command, CommentAddArgs, CommentArgs, CommentCommand,
    CommentListArgs, DeleteArgs, DependArgs, DoneArgs, HeartbeatArgs, InitArgs, LabelArgs,
    LabelCommand, LabelMutationArgs, LinkArgs, ListArgs, PrimeArgs, ReadyArgs, ReleaseArgs,
    ReopenArgs, ShowArgs, SlugArgs, SlugCommand, SlugMigrateArgs, TreeArgs, UndependArgs,
    UpdateArgs,
};
use crate::comment::{self, Comment};
use crate::error::{OrbitError, Result, code};
use crate::git::CommitOutcome;
use crate::graph::{self, Graph, LoadMode};
use crate::migrate::{self, Plan};
use crate::setup::COMPLETION_LINE;
use crate::slug;
use crate::task::{self, Status, Task, TaskFrontmatter, TaskType};
use crate::transition::{ClaimGate, Outcome, TransitionError};

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
        Command::Slug(args) => {
            let exclusive = matches!(args.command, SlugCommand::Migrate(_));
            with_board(dir.as_deref(), exclusive, |board| cmd_slug(board, args))
        }
        Command::Add(args) => with_board(dir.as_deref(), true, |board| cmd_add(board, args)),
        Command::List(args) => with_board(dir.as_deref(), false, |board| cmd_list(board, args)),
        Command::Ready(args) => {
            let exclusive = args.claim;
            with_board(dir.as_deref(), exclusive, |board| cmd_ready(board, args))
        }
        Command::Show(args) => with_board(dir.as_deref(), false, |board| cmd_show(board, args)),
        Command::Tree(args) => with_board(dir.as_deref(), false, |board| cmd_tree(board, args)),
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
        Command::Export => with_board(dir.as_deref(), false, cmd_export),
        Command::Setup(args) => crate::setup::run(args, dir.as_deref()),
        Command::Mcp => crate::mcp::serve(dir.as_deref())
            .map(|()| crate::output::CommandOutput::new(Value::Null, "").with_silent()),
        Command::Doctor(args) => crate::doctor::run(dir.as_deref(), args),
        Command::Sync => with_board(dir.as_deref(), true, cmd_sync),
        Command::GitHub(args) => {
            with_board(dir.as_deref(), true, |board| crate::github::commands::run(board, args))
        }
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
    let mut output = f(&board)?;
    output.warnings.extend(board.warnings.iter().cloned());
    Ok(output)
}

struct InitTarget {
    board_root: std::path::PathBuf,
    created: bool,
    prefix: String,
    storage: crate::board::StorageMode,
    branch: String,
    exclude: Option<String>,
    warnings: Vec<String>,
}

fn cmd_init(args: InitArgs, dir: Option<&Path>) -> Result<crate::output::CommandOutput> {
    let cwd = std::env::current_dir()
        .map_err(|error| OrbitError::io("resolve current directory", error))?;
    let cwd = dunce::canonicalize(&cwd).unwrap_or(cwd);
    let in_repo = crate::git::is_git_repo(&cwd);
    let setup_root = if in_repo {
        crate::git::repo_root()?
    } else {
        cwd.clone()
    };

    if args.global && dir.is_some() {
        return Err(OrbitError::invalid_argument(
            "--global cannot be combined with --dir or ORBIT_DIR",
        ));
    }

    let home = home_dir().ok();
    if args.global && home.is_none() {
        return Err(OrbitError::config(
            "HOME or USERPROFILE must be set for `orbit init --global`",
        ));
    }
    let global_dir = home.as_ref().map(|home| home.join(".orbit"));
    let (board_root, explicit_global) = if args.global {
        (global_dir.clone().expect("global home was checked"), true)
    } else if let Some(target) = args.target.as_deref() {
        (crate::board::resolve_external(target)?, false)
    } else if let Some(dir) = dir {
        (crate::board::resolve_external(dir)?, false)
    } else {
        (default_init_dir(&cwd)?, false)
    };
    let global = explicit_global
        || global_dir
            .as_ref()
            .is_some_and(|global| global == &board_root);

    let interactive = std::io::stdin().is_terminal();
    let clients = crate::setup::selected_agents(&args, &setup_root)?;
    let selected_portable_mcp = crate::setup::selected_portable_mcp(&args, &clients, &setup_root)?;
    let ignored_portable_mcp = args.portable_mcp && !clients.iter().any(|client| client.uses_mcp());

    let InitTarget {
        board_root,
        created,
        prefix,
        storage,
        branch,
        mut exclude,
        mut warnings,
    } = run_init(&board_root, args.slug.as_deref(), args.print, global)?;

    if !args.print && storage == crate::board::StorageMode::Worktree {
        exclude = Board::ensure_excluded(&board_root)?;
    }

    let mut files = json!([]);
    let mut mcp_command = None;
    let mut reported_portable_mcp = false;
    if !clients.is_empty() {
        let mcp_bin =
            crate::setup::configured_mcp_bin(selected_portable_mcp, args.mcp_bin.as_deref())?;
        reported_portable_mcp = crate::setup::is_portable_mcp_bin(&mcp_bin);
        if clients.iter().any(|client| client.uses_mcp()) {
            mcp_command = Some(format!("{} mcp", mcp_bin.display()));
        }
        let hook_bin = if args.hooks {
            Some(crate::setup::configured_hook_bin(args.mcp_bin.as_deref())?)
        } else {
            None
        };
        let output = crate::setup::install(
            &setup_root,
            crate::setup::InstallOptions {
                scope: crate::setup::SetupScope::Project,
                clients: clients.clone(),
                hooks: args.hooks,
                hook_bin,
                mcp_bin: Some(mcp_bin),
                print: args.print,
                remove: false,
            },
        )?;
        files = output.data["files"].clone();
        warnings.extend(output.warnings);
    }
    if ignored_portable_mcp {
        warnings.push("no selected harness uses MCP; ignoring --portable-mcp".to_string());
    }

    let human = render_init_human(&InitReport {
        args: &args,
        interactive,
        board_root: &board_root,
        created,
        prefix: &prefix,
        storage,
        branch: &branch,
        exclude: exclude.as_deref(),
        global,
        clients: &clients,
        files: &files,
        mcp_command: mcp_command.as_deref(),
        portable_mcp: reported_portable_mcp,
    });

    let mut output = crate::output::CommandOutput::new(
        json!({
            "board": board_root,
            "branch": branch,
            "prefix": prefix,
            "storage": storage.as_str(),
            "global": global,
            "exclude": exclude,
            "created": created,
            "preview": args.print,
            "agents": clients.iter().map(|client| client.name()).collect::<Vec<_>>(),
            "portable_mcp": reported_portable_mcp,
            "files": files,
        }),
        human,
    );
    output.warnings = warnings;
    Ok(output)
}

struct InitReport<'a> {
    args: &'a InitArgs,
    interactive: bool,
    board_root: &'a Path,
    created: bool,
    prefix: &'a str,
    storage: crate::board::StorageMode,
    branch: &'a str,
    exclude: Option<&'a str>,
    global: bool,
    clients: &'a [crate::setup::SetupClient],
    files: &'a Value,
    mcp_command: Option<&'a str>,
    portable_mcp: bool,
}

/// Render the human onboarding report. Every section states what happened (or
/// what a preview would do), so a first run does not require reading the docs.
fn render_init_human(report: &InitReport<'_>) -> String {
    let preview = report.args.print;
    let mut human = String::new();

    if preview {
        human.push_str("Orbit setup preview (nothing was written).\n\n");
    } else if report.created {
        human.push_str("Orbit setup complete.\n\n");
    } else if report.clients.is_empty() {
        human.push_str("Orbit is already initialized; the Board was reused.\n\n");
    } else {
        human.push_str("Orbit is already initialized; agent integration was reconciled.\n\n");
    }

    let board_state = match (preview, report.created) {
        (true, true) => "would create",
        (true, false) => "existing; would reuse",
        (false, true) => "created",
        (false, false) => "existing; reused",
    };
    human.push_str(&format!("1. Board ({board_state})\n"));
    human.push_str(&format!(
        "   {:<9}{}\n",
        "path",
        report.board_root.display()
    ));
    human.push_str(&format!("   {:<9}{}\n", "branch", report.branch));
    let storage = match (report.storage, report.global) {
        (crate::board::StorageMode::Worktree, _) => {
            "worktree (shared by every Git worktree of this repository)"
        }
        (crate::board::StorageMode::Repository, true) => "repository (global fallback at ~/.orbit)",
        (crate::board::StorageMode::Repository, false) => {
            "repository (standalone Board folder; link repositories with `orbit link`)"
        }
    };
    human.push_str(&format!("   {:<9}{storage}\n", "storage"));
    let prefix = report.prefix;
    let prefix_detail = match prefix.strip_suffix('-') {
        Some(slug) if !slug.is_empty() => {
            format!("{prefix} (Task IDs look like {slug}-a7b2c9)")
        }
        _ => format!("{prefix} (Task IDs start with {prefix})"),
    };
    human.push_str(&format!("   {:<9}{prefix_detail}\n", "prefix"));
    if let Some(exclude) = report.exclude {
        let detail = if preview {
            "would be listed in .git/info/exclude"
        } else {
            "listed in .git/info/exclude"
        };
        human.push_str(&format!("   {:<9}{exclude} ({detail})\n", "exclude"));
    }
    human.push('\n');

    if report.clients.is_empty() {
        human.push_str("2. Coding agents (skipped)\n");
        human.push_str(&format!(
            "   {:<9}{}\n",
            "reason",
            init_agent_skip_reason(report.args, report.interactive)
        ));
    } else {
        human.push_str("2. Coding agents\n");
        let wired = report
            .clients
            .iter()
            .map(|client| client.name())
            .collect::<Vec<_>>()
            .join(", ");
        human.push_str(&format!("   {:<9}{wired}\n", "wired"));
        if let Some(command) = report.mcp_command {
            let detail = if report.portable_mcp {
                "portable; every editor that launches it must find `orbit` on PATH"
            } else {
                "absolute path; GUI-launched editors need no PATH setup"
            };
            human.push_str(&format!("   {:<9}{command} ({detail})\n", "mcp"));
        }
        if report.args.hooks {
            human.push_str(&format!("   {:<9}Claude Code SessionStart\n", "hook"));
        }
        human.push_str("   files\n");
        let files = report.files.as_array();
        if files.is_none_or(|files| files.is_empty()) {
            human.push_str("     already up to date\n");
        } else {
            for file in files.into_iter().flatten() {
                let action = file["action"].as_str().unwrap_or("unknown");
                let path = file["path"].as_str().unwrap_or("?");
                human.push_str(&format!(
                    "     {:<13}{path}\n",
                    init_action_label(preview, action)
                ));
            }
        }
    }
    human.push('\n');

    human.push_str(if preview {
        "3. Verify after applying\n"
    } else {
        "3. Verify\n"
    });
    human.push_str("   orbit doctor   # check the Board, worktree link, and PATH\n\n");

    if preview {
        human.push_str("Re-run the same command without --print to apply these changes.\n");
        if let Some(files) = report.files.as_array() {
            let previews = files
                .iter()
                .filter_map(|file| Some((file["path"].as_str()?, file["content"].as_str()?)))
                .collect::<Vec<_>>();
            if !previews.is_empty() {
                human.push_str("\nGenerated file previews:\n");
                for (path, content) in previews {
                    human.push_str(&format!("\n--- {path} ---\n{content}"));
                    if !content.ends_with('\n') {
                        human.push('\n');
                    }
                }
            }
        }
        return human;
    }

    human.push_str("Next steps:\n");
    let external = report.storage == crate::board::StorageMode::Repository && !report.global;
    if external {
        human.push_str(&format!(
            "  orbit link {}   # join this Board from each repository\n",
            report.board_root.display()
        ));
    }
    if report.clients.is_empty() {
        human.push_str(&format!(
            "  {:<33}# wire a coding agent\n",
            "orbit setup <client>"
        ));
    }
    human.push_str(&format!(
        "  {:<33}# create work on the Board\n",
        "orbit add \"First task\" -p 1"
    ));
    if !report.clients.is_empty() {
        let agent = report
            .clients
            .first()
            .map(|client| client.name())
            .unwrap_or("claude");
        human.push_str(&format!(
            "  {:<33}# claim one ready task\n",
            format!("orbit ready --claim --by {agent}")
        ));
    }
    human.push_str(&format!("  {:<33}# see the Board\n", "orbit list"));
    human
}

fn init_action_label(preview: bool, action: &str) -> &str {
    match (preview, action) {
        (true, "created") => "would create",
        (true, "updated") => "would update",
        (true, "removed") => "would remove",
        _ => action,
    }
}

fn init_agent_skip_reason(args: &InitArgs, interactive: bool) -> String {
    if args.no_agents {
        return "disabled by --no-agents".to_string();
    }
    if let Some(selector) = args.agents.as_deref() {
        if selector.trim() == "none" {
            return "disabled by --agents none".to_string();
        }
        return "no supported harness detected; pass --agents all or a client list".to_string();
    }
    if args.print {
        return "no supported harness detected; pass --agents all or a client list".to_string();
    }
    if !interactive {
        return "no terminal; pass --agents auto or run `orbit setup <client>`".to_string();
    }
    "none selected".to_string()
}

fn run_init(
    board_root: &Path,
    slug_arg: Option<&str>,
    print: bool,
    global: bool,
) -> Result<InitTarget> {
    if Board::is_board(board_root) {
        let board = Board::discover(Some(board_root))?;
        let prefix = reconcile_prefix(&board, slug_arg, print)?;
        let storage = board.storage_mode();
        let exclude = if storage == crate::board::StorageMode::Worktree {
            Board::exclude_entry(board_root)?
        } else {
            None
        };
        return Ok(InitTarget {
            board_root: board_root.to_path_buf(),
            created: false,
            prefix,
            storage,
            branch: board.config.board.branch,
            exclude,
            warnings: board.warnings,
        });
    }

    if print && let Some(existing) = Board::conflicting_board_for(board_root, global)? {
        return Err(crate::board::already_initialized(&existing));
    }

    let (slug, warning) = match slug_arg {
        Some(slug) => {
            slug::explicit_prefix(slug)?;
            (slug.to_string(), None)
        }
        None => slug::auto_slug_for_dir(board_root),
    };
    let prefix = slug::prefix(&slug);
    let storage = if global {
        crate::board::StorageMode::Repository
    } else {
        crate::board::StorageMode::for_new_dir(board_root)
    };
    let exclude = if storage == crate::board::StorageMode::Worktree {
        Board::exclude_entry(board_root)?
    } else {
        None
    };

    if print {
        return Ok(InitTarget {
            board_root: board_root.to_path_buf(),
            created: true,
            prefix,
            storage,
            branch: "orbit".to_string(),
            exclude,
            warnings: warning.into_iter().collect(),
        });
    }

    let result = if global {
        Board::init_repository(board_root, Some(&slug))?
    } else {
        Board::init_dir(board_root, Some(&slug))?
    };
    Ok(InitTarget {
        board_root: result.board,
        created: true,
        prefix: result.prefix,
        storage: result.storage,
        branch: result.branch,
        exclude,
        warnings: warning.into_iter().collect(),
    })
}

fn default_init_dir(cwd: &Path) -> Result<std::path::PathBuf> {
    let Ok(repo_root) = crate::git::repo_root() else {
        return Ok(cwd.join(".orbit"));
    };
    let repo_root = dunce::canonicalize(&repo_root).unwrap_or(repo_root);
    let main = slug::main_checkout_anchor().unwrap_or_else(|_| repo_root.clone());
    if repo_root != main {
        return Ok(main.join(".orbit"));
    }
    Ok(cwd.join(".orbit"))
}

fn home_dir() -> Result<std::path::PathBuf> {
    std::env::var_os("HOME")
        .or_else(|| std::env::var_os("USERPROFILE"))
        .map(std::path::PathBuf::from)
        .ok_or_else(|| OrbitError::config("HOME or USERPROFILE is not set"))
        .and_then(|home| {
            dunce::canonicalize(&home)
                .map_err(|error| OrbitError::io(format!("resolve {}", home.display()), error))
        })
}

fn reconcile_prefix(board: &Board, slug: Option<&str>, print: bool) -> Result<String> {
    let Some(slug) = slug else {
        return Ok(board.config.id.prefix.clone());
    };

    let requested = slug::explicit_prefix(slug)?;
    if slug::slug_of_prefix(&board.config.id.prefix) == slug {
        return Ok(board.config.id.prefix.clone());
    }

    let tasks = board.task_ids()?.len();
    if tasks > 0 {
        return Err(crate::board::slug_locked_error(
            &board.config.id.prefix,
            &requested,
            tasks,
        ));
    }

    if !print {
        board.set_prefix(&requested)?;
    }
    Ok(requested)
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
            "shadowed_boards": shadowed_boards,
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
        match crate::setup::refresh(&root) {
            Ok(output) => warnings.extend(output.warnings),
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

fn cmd_slug(board: &Board, args: SlugArgs) -> Result<crate::output::CommandOutput> {
    match args.command {
        SlugCommand::Show => cmd_slug_show(board),
        SlugCommand::Migrate(args) => cmd_slug_migrate(board, args),
    }
}

fn cmd_slug_show(board: &Board) -> Result<crate::output::CommandOutput> {
    let ids = board.task_ids()?;
    let subtasks = ids.iter().filter(|id| id.contains('.')).count();
    let comment_files = board.comment_ids()?.len();
    let slug = slug::slug_of_prefix(&board.config.id.prefix);

    let human = format!(
        "slug     {slug}\nprefix   {}\ntasks    {} ({})\ncomments {}\n",
        board.config.id.prefix,
        ids.len(),
        plural(subtasks, "SubTask", "SubTasks"),
        plural(comment_files, "file", "files"),
    );
    Ok(crate::output::CommandOutput::new(
        json!({
            "slug": slug,
            "prefix": board.config.id.prefix,
            "tasks": ids.len(),
            "subtasks": subtasks,
            "comment_files": comment_files,
        }),
        human,
    ))
}

fn cmd_slug_migrate(board: &Board, args: SlugMigrateArgs) -> Result<crate::output::CommandOutput> {
    let (graph, _) = Graph::load(board, LoadMode::Strict)?;
    let issues = migrate::health_issues(board, &graph)?;
    if !issues.is_empty() {
        return Err(migrate::blocked_by_issues(issues));
    }

    let plan = migrate::plan(board, &graph, &args.slug)?;
    if !plan.collisions.is_empty() {
        return Err(migrate::blocked_by_collisions(
            &plan.collisions,
            &plan.new_prefix,
        ));
    }
    if !args.dry_run && plan.changed && !plan.claims.is_empty() && !args.force {
        return Err(migrate::blocked_by_claims(&plan.claims));
    }

    if args.dry_run || !plan.changed {
        let mut output = crate::output::CommandOutput::new(
            slug_plan_json(&plan, args.dry_run, false, None),
            slug_plan_human(&plan, args.dry_run),
        );
        if !plan.claims.is_empty() {
            output.warnings.push(format!(
                "{} live claim(s) would change ID; apply requires --force",
                plan.claims.len()
            ));
        }
        return Ok(output);
    }

    let applied = migrate::apply(board, &graph, &plan)?;
    let mut output = crate::output::CommandOutput::new(
        slug_plan_json(&plan, false, true, Some(&applied.commit)),
        slug_applied_human(&plan, &applied.commit),
    );
    output.warnings = applied.warnings;
    if !plan.claims.is_empty() {
        output.warnings.push(format!(
            "{} live claim(s) changed ID; holders must re-read the Board",
            plan.claims.len()
        ));
    }
    Ok(output)
}

fn slug_plan_json(plan: &Plan, dry_run: bool, applied: bool, commit: Option<&str>) -> Value {
    json!({
        "current": {
            "slug": &plan.old_slug,
            "prefix": &plan.old_prefix,
        },
        "target": {
            "slug": &plan.new_slug,
            "prefix": &plan.new_prefix,
        },
        "changed": plan.changed,
        "tasks": plan.task_count,
        "subtasks": plan.subtask_count,
        "comment_files": plan.comment_files.len(),
        "references": plan.references,
        "mappings": &plan.mappings,
        "claims": &plan.claims,
        "collisions": &plan.collisions,
        "dry_run": dry_run,
        "applied": applied,
        "commit": commit,
    })
}

fn slug_plan_human(plan: &Plan, dry_run: bool) -> String {
    if !plan.changed {
        return format!(
            "board already uses slug `{}` (prefix `{}`); nothing to migrate\n",
            plan.old_slug, plan.old_prefix
        );
    }

    let mut human = String::from(if dry_run {
        "slug migration plan\n"
    } else {
        "slug migration\n"
    });
    human.push_str(&format!(
        "prefix     {} -> {}\n",
        plan.old_prefix, plan.new_prefix
    ));
    human.push_str(&format!(
        "tasks      {} ({})\n",
        plan.task_count,
        plural(plan.subtask_count, "SubTask", "SubTasks")
    ));
    human.push_str(&format!(
        "comments   {}\n",
        plural(plan.comment_files.len(), "file", "files")
    ));
    human.push_str(&format!("references {}\n", plan.references));
    append_claim_lines(&mut human, plan);
    if dry_run {
        human.push_str("dry run: no files written\n");
    }
    human
}

fn slug_applied_human(plan: &Plan, commit: &str) -> String {
    let mut human = format!(
        "migrated {}, {}, and {}\n",
        plural(plan.task_count, "task", "tasks"),
        plural(plan.subtask_count, "SubTask", "SubTasks"),
        plural(plan.comment_files.len(), "comment file", "comment files"),
    );
    human.push_str(&format!(
        "prefix     {} -> {}\n",
        plan.old_prefix, plan.new_prefix
    ));
    human.push_str(&format!("commit     {}\n", short_commit(commit)));
    append_claim_lines(&mut human, plan);
    human
}

fn append_claim_lines(human: &mut String, plan: &Plan) {
    if plan.claims.is_empty() {
        return;
    }
    human.push_str("claims:\n");
    for claim in &plan.claims {
        let lease = claim
            .lease_until
            .map(|until| format!(", lease until {}", until.to_rfc3339()))
            .unwrap_or_default();
        human.push_str(&format!(
            "  {} -> {} ({}{})\n",
            claim.from, claim.to, claim.holder, lease
        ));
    }
}

fn plural(count: usize, singular: &str, plural: &str) -> String {
    format!("{count} {}", if count == 1 { singular } else { plural })
}

fn short_commit(commit: &str) -> &str {
    commit.get(..7).unwrap_or(commit)
}

fn cmd_add(board: &Board, args: AddArgs) -> Result<crate::output::CommandOutput> {
    let (mut graph, _) = Graph::load(board, LoadMode::Strict)?;

    let title = args.title.trim().to_string();
    if title.is_empty() {
        return Err(OrbitError::invalid_argument("title must not be empty"));
    }

    let priority = args.priority.unwrap_or(board.config.board.default_priority);
    validate_priority(priority)?;
    let task_type = args.task_type.unwrap_or(TaskType::Task);
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

    let parent = match args.parent {
        Some(parent_id) => {
            let parent_task = graph.get(&parent_id)?;
            if parent_task.front.status.is_terminal() {
                return Err(OrbitError::invalid_argument(format!(
                    "parent {parent_id} is {}; a SubTask needs an active Parent Task",
                    parent_task.front.status.as_str()
                )));
            }
            if parent_task.front.parent.is_some() {
                return Err(OrbitError::invalid_argument(format!(
                    "parent {parent_id} is a SubTask; SubTasks are one level deep"
                )));
            }
            Some(parent_id)
        }
        None => None,
    };

    if parent.is_some() && task_type == TaskType::Epic {
        return Err(OrbitError::new(
            code::INVALID_TYPE,
            "epic type is only allowed on a top-level Task",
        ));
    }

    if let Some(parent_id) = &parent
        && depends_on.iter().any(|dep| dep == parent_id)
    {
        return Err(OrbitError::invalid_argument(
            "a SubTask cannot depend on its Parent Task; parentage already makes the parent wait",
        ));
    }

    let waiting_on = normalize_waiting(args.waiting_on)?;
    let body = resolve_body(args.body, args.body_file)?;
    let id = generate_unique_id(board, parent.as_deref())?;
    let now = Utc::now();

    let task = Task {
        id: id.clone(),
        front: TaskFrontmatter {
            title: title.clone(),
            status: Status::Todo,
            task_type,
            priority,
            labels,
            depends_on,
            parent,
            waiting_on,
            area: None,
            verification: None,
            github: None,
            claim: None,
            stray_claim_times: None,
            created_at: now,
            updated_at: now,
            extra: Default::default(),
        },
        body,
    };

    let rendered = task::render_task(&task)?;
    atomic::write_atomic(&board.task_path(&id), rendered.as_bytes(), false)?;
    graph.tasks.insert(id.clone(), task.clone());

    let type_prefix = if task_type == TaskType::Task {
        String::new()
    } else {
        format!("{}  ", task_type.as_str())
    };
    let human = match &task.front.parent {
        Some(parent) => {
            format!("created {id}  P{priority}  {type_prefix}{title}  (subtask of {parent})")
        }
        None => format!("created {id}  P{priority}  {type_prefix}{title}"),
    };
    task_output(board, &graph, &task, human, &format!("orbit: add {id}"))
}

fn cmd_list(board: &Board, args: ListArgs) -> Result<crate::output::CommandOutput> {
    let (graph, _) = Graph::load(board, LoadMode::Strict)?;
    if let Some(priority) = args.priority {
        validate_priority(priority)?;
    }
    let label_filters = normalize_label_filters(&args.label, args.repo.as_deref())?;
    let excluded_labels = normalize_labels(&args.exclude_label)?;
    if let Some(parent) = args.parent.as_deref() {
        let parent_task = graph.get(parent)?;
        if parent_task.front.parent.is_some() {
            return Err(OrbitError::invalid_argument(format!(
                "{parent} is a SubTask; `--parent` expects a Parent Task"
            )));
        }
    }

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
                && task.holder() != Some(claimed_by.as_str())
            {
                return false;
            }
            if !args.task_type.is_empty() && !args.task_type.contains(&task.front.task_type) {
                return false;
            }
            if args.exclude_type.contains(&task.front.task_type) {
                return false;
            }
            if !task.has_labels(&label_filters) {
                return false;
            }
            if task.has_any_label(&excluded_labels) {
                return false;
            }
            if args.unlabeled && !task.front.labels.is_empty() {
                return false;
            }
            if let Some(parent) = args.parent.as_deref()
                && task.front.parent.as_deref() != Some(parent)
            {
                return false;
            }
            !args.waiting || task.front.waiting_on.is_some()
        })
        .collect();

    tasks.sort_by(|left, right| list_sort_key(&graph, left).cmp(&list_sort_key(&graph, right)));

    let summaries: Vec<Value> = tasks
        .iter()
        .map(|task| graph::task_summary(&graph, task))
        .collect();
    let count = summaries.len();
    let human = if tasks.is_empty() {
        "no tasks".to_string()
    } else if args.group {
        format_grouped_table(&group_tasks(&graph, &tasks)?)
    } else {
        format_table(&tasks)
    };

    Ok(
        crate::output::CommandOutput::new(json!({ "tasks": summaries, "count": count }), human)
            .with_meta(json!({ "count": count })),
    )
}

fn cmd_ready(board: &Board, args: ReadyArgs) -> Result<crate::output::CommandOutput> {
    let (mut graph, _) = Graph::load(board, LoadMode::Strict)?;
    let label_filters = normalize_label_filters(&args.label, args.repo.as_deref())?;
    let excluded_labels = normalize_labels(&args.exclude_label)?;
    if let Some(parent) = args.parent.as_deref() {
        let parent_task = graph.get(parent)?;
        if parent_task.front.parent.is_some() {
            return Err(OrbitError::invalid_argument(format!(
                "{parent} is a SubTask; `--parent` expects a Parent Task"
            )));
        }
    }

    if args.claim {
        let agent = resolve_agent(args.by)?;
        let ready = ready_for_filters(
            graph.ready_tasks(),
            args.parent.as_deref(),
            &label_filters,
            &excluded_labels,
            &args.task_type,
            &args.exclude_type,
        );
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
            .lease_until()
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

    let mut ready = ready_for_filters(
        graph.ready_tasks(),
        args.parent.as_deref(),
        &label_filters,
        &excluded_labels,
        &args.task_type,
        &args.exclude_type,
    );
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

fn cmd_tree(board: &Board, args: TreeArgs) -> Result<crate::output::CommandOutput> {
    let (graph, _) = Graph::load(board, LoadMode::Strict)?;
    let root = graph.get(&args.id)?;
    if root.front.parent.is_some() {
        return Err(OrbitError::invalid_argument(format!(
            "{} is a SubTask; `orbit tree` expects a Parent Task",
            root.id
        )));
    }

    let children = graph.children(&root.id);
    let mut tasks = Vec::with_capacity(children.len() + 1);
    tasks.push(root.clone());
    tasks.extend(children.into_iter().cloned());

    let summaries: Vec<Value> = tasks
        .iter()
        .map(|task| graph::task_summary(&graph, task))
        .collect();
    let count = summaries.len();

    let mut human = format_tree_line(root, &graph, true);
    for child in &tasks[1..] {
        human.push('\n');
        human.push_str(&format_tree_line(child, &graph, false));
    }

    Ok(crate::output::CommandOutput::new(
        json!({ "root": root.id, "tasks": summaries, "count": count }),
        human,
    )
    .with_meta(json!({ "count": count })))
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
        .lease_until()
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

/// Applies one Task transition and returns its result with the updated Task.
fn transition<T>(
    graph: &mut Graph,
    id: &str,
    apply: impl FnOnce(&mut Task) -> std::result::Result<T, TransitionError>,
) -> Result<(T, Task)> {
    let task = graph.get_mut(id)?;
    let result = apply(task)?;
    Ok((result, task.clone()))
}

fn cmd_release(board: &Board, args: ReleaseArgs) -> Result<crate::output::CommandOutput> {
    let (mut graph, _) = Graph::load(board, LoadMode::Strict)?;
    let caller = optional_agent(args.by);

    let (outcome, task) = transition(&mut graph, &args.id, |task| {
        task.release(caller.as_deref(), args.force, Utc::now())
    })?;
    if outcome == Outcome::Unchanged {
        return Ok(crate::output::CommandOutput::new(
            json!({ "id": args.id, "changed": false }),
            format!("{} was not claimed", args.id),
        ));
    }

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

    let (lease_until, task) = transition(&mut graph, &args.id, |task| {
        task.heartbeat(
            &agent,
            ChronoDuration::minutes(board.config.board.lease_minutes),
            Utc::now(),
        )
    })?;
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

    let open_subtasks = graph.open_children(&args.id);
    if !open_subtasks.is_empty() && !args.force {
        return Err(subtasks_open_error(&args.id, &open_subtasks));
    }
    let blocking = graph.blocking_ids(graph.get(&args.id)?);
    if !blocking.is_empty() {
        return Err(TransitionError::DependencyBlocked {
            id: args.id.clone(),
            blocking,
        }
        .into());
    }
    let cancelled_subtasks = if open_subtasks.is_empty() {
        Vec::new()
    } else {
        cancel_open_subtasks(board, &mut graph, &args.id, caller.as_deref(), now)?
    };

    let outcome =
        graph
            .get_mut(&args.id)?
            .complete(caller.as_deref(), args.force, &blocking, &[], now)?;

    let comment = author.zip(comment);
    let mut output = settle_terminal(
        board,
        &mut graph,
        &args.id,
        outcome,
        comment,
        now,
        TerminalKind::Done,
    )?;
    if !cancelled_subtasks.is_empty() {
        output.warnings.push(format!(
            "cancelled open SubTasks: {}",
            cancelled_subtasks.join(", ")
        ));
    }
    Ok(output)
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

    let open_subtasks = graph.open_children(&args.id);
    if !open_subtasks.is_empty() && !args.force {
        return Err(subtasks_open_error(&args.id, &open_subtasks));
    }
    let cancelled_subtasks = if open_subtasks.is_empty() {
        Vec::new()
    } else {
        cancel_open_subtasks(board, &mut graph, &args.id, caller.as_deref(), now)?
    };

    let outcome = graph
        .get_mut(&args.id)?
        .cancel(caller.as_deref(), args.force, now)?;

    let comment = author.zip(comment);
    let mut output = settle_terminal(
        board,
        &mut graph,
        &args.id,
        outcome,
        comment,
        now,
        TerminalKind::Cancelled,
    )?;
    if !cancelled_subtasks.is_empty() {
        output.warnings.push(format!(
            "cancelled open SubTasks: {}",
            cancelled_subtasks.join(", ")
        ));
    }
    Ok(output)
}

#[derive(Clone, Copy)]
enum TerminalKind {
    Done,
    Cancelled,
}

impl TerminalKind {
    /// Human wording, e.g. "cancelled".
    fn past(self) -> &'static str {
        match self {
            Self::Done => "done",
            Self::Cancelled => "cancelled",
        }
    }

    /// Commit verb, e.g. "cancel".
    fn verb(self) -> &'static str {
        match self {
            Self::Done => "done",
            Self::Cancelled => "cancel",
        }
    }
}

/// Persists a done/cancel transition and its optional `(author, body)` comment.
fn settle_terminal(
    board: &Board,
    graph: &mut Graph,
    id: &str,
    outcome: Outcome,
    comment: Option<(String, String)>,
    now: chrono::DateTime<Utc>,
    kind: TerminalKind,
) -> Result<crate::output::CommandOutput> {
    let (past, verb) = (kind.past(), kind.verb());
    let message = format!("orbit: {verb} {id}");
    match (outcome, comment) {
        (Outcome::Unchanged, None) => Ok(crate::output::CommandOutput::new(
            graph::task_detail(graph, graph.get(id)?),
            format!("{id} is already {past}"),
        )
        .with_meta(json!({ "changed": false }))),
        (Outcome::Unchanged, Some((author, body))) => {
            let comments = add_comment(board, id, &author, body, now)?;
            let task = {
                let task = graph.get_mut(id)?;
                task.touch(now);
                task.clone()
            };
            write_task(board, &task)?;
            task_output_with_comments(
                board,
                graph,
                &task,
                &comments,
                format!("{past} {id} (comment added)"),
                &message,
            )
        }
        (Outcome::Changed, comment) => {
            let task = graph.get(id)?.clone();
            write_task(board, &task)?;
            let Some((author, body)) = comment else {
                return task_output(board, graph, &task, format!("{past} {id}"), &message);
            };
            match add_comment(board, id, &author, body, now) {
                Ok(comments) => task_output_with_comments(
                    board,
                    graph,
                    &task,
                    &comments,
                    format!("{past} {id} (comment added)"),
                    &message,
                ),
                Err(error) => {
                    let output =
                        task_output(board, graph, &task, format!("{past} {id}"), &message)?;
                    Ok(output
                        .with_warning(format!("COMMENT_FAILED: {}", error.message))
                        .with_exit_code(1))
                }
            }
        }
    }
}

fn cancel_open_subtasks(
    board: &Board,
    graph: &mut Graph,
    parent_id: &str,
    caller: Option<&str>,
    now: chrono::DateTime<Utc>,
) -> Result<Vec<String>> {
    let ids = graph.open_children(parent_id);
    for id in &ids {
        let task = graph.get_mut(id)?;
        task.cancel(caller, true, now)?;
        let task = task.clone();
        write_task(board, &task)?;
    }
    Ok(ids)
}

fn subtasks_open_error(id: &str, subtasks: &[String]) -> OrbitError {
    OrbitError::new(
        code::SUBTASKS_OPEN,
        format!("{id} has open SubTasks: {}", subtasks.join(", ")),
    )
    .with_details(json!({ "task": id, "subtasks": subtasks }))
}

fn subtasks_exist_error(id: &str, subtasks: &[String]) -> OrbitError {
    OrbitError::new(
        code::SUBTASKS_EXIST,
        format!("{id} has SubTasks: {}", subtasks.join(", ")),
    )
    .with_details(json!({ "task": id, "subtasks": subtasks }))
}

fn cmd_reopen(board: &Board, args: ReopenArgs) -> Result<crate::output::CommandOutput> {
    let (mut graph, _) = Graph::load(board, LoadMode::Strict)?;
    let now = Utc::now();

    let task = graph.get(&args.id)?.clone();
    let child_dependents: Vec<String> = graph
        .dependents(&args.id)
        .iter()
        .filter(|dependent| dependent.front.status == Status::Done)
        .map(|dependent| dependent.id.clone())
        .collect();
    let mut probe = task.clone();
    probe.reopen(args.force, &child_dependents, now)?;

    let mut reopened_parent = None;
    if let Some(parent_id) = task.front.parent.clone() {
        let parent_status = graph.get(&parent_id)?.front.status;
        match parent_status {
            Status::Done => {
                let done_dependents: Vec<String> = graph
                    .dependents(&parent_id)
                    .iter()
                    .filter(|dependent| dependent.front.status == Status::Done)
                    .map(|dependent| dependent.id.clone())
                    .collect();
                graph
                    .get_mut(&parent_id)?
                    .reopen(args.force, &done_dependents, now)?;
                let parent = graph.get(&parent_id)?.clone();
                write_task(board, &parent)?;
                reopened_parent = Some(parent_id);
            }
            Status::Cancelled => {
                return Err(invalid_transition(format!(
                    "{} is a SubTask of cancelled {parent_id}; reopen the parent first",
                    args.id
                )));
            }
            _ => {}
        }
    }

    let task = {
        let task = graph.get_mut(&args.id)?;
        task.reopen(args.force, &child_dependents, now)?;
        task.clone()
    };
    write_task(board, &task)?;

    let mut output = task_output(
        board,
        &graph,
        &task,
        format!("reopened {}", args.id),
        &format!("orbit: reopen {}", args.id),
    )?;
    if let Some(parent) = reopened_parent {
        output.warnings.push(format!("reopened parent {parent}"));
    }
    Ok(output)
}

fn cmd_update(board: &Board, args: UpdateArgs) -> Result<crate::output::CommandOutput> {
    let (mut graph, _) = Graph::load(board, LoadMode::Strict)?;

    let has_changes = args.title.is_some()
        || args.body.is_some()
        || args.body_file.is_some()
        || args.priority.is_some()
        || args.task_type.is_some()
        || args.waiting_on.is_some()
        || args.clear_waiting;
    if !has_changes {
        return Err(OrbitError::invalid_argument(
            "nothing to update; pass --title, --body, --priority, --type, or --waiting-on",
        ));
    }

    if let Some(priority) = args.priority {
        validate_priority(priority)?;
    }
    let current = graph.get(&args.id)?;
    let next_type = args.task_type.unwrap_or(current.front.task_type);
    if next_type == TaskType::Epic && current.front.parent.is_some() {
        return Err(OrbitError::new(
            code::INVALID_TYPE,
            "epic type is only allowed on a top-level Task",
        ));
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
        if let Some(task_type) = args.task_type {
            task.front.task_type = task_type;
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
        let dep = graph.get(&args.dep_id)?;
        if task.front.parent.as_deref() == Some(args.dep_id.as_str())
            || dep.front.parent.as_deref() == Some(args.id.as_str())
        {
            return Err(OrbitError::invalid_argument(
                "parentage is not a dependency; use `orbit add --parent` to create a SubTask",
            ));
        }
        if task.front.status.is_terminal() {
            return Err(invalid_transition(format!(
                "{} is {}; cannot add a dependency to a terminal task",
                args.id,
                task.front.status.as_str()
            )));
        }
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

    graph
        .get(&args.id)?
        .authorize_live(caller.as_deref(), args.force, now)?;

    let child_ids: Vec<String> = graph
        .children(&args.id)
        .iter()
        .map(|task| task.id.clone())
        .collect();
    if !child_ids.is_empty() && !args.force {
        return Err(subtasks_exist_error(&args.id, &child_ids));
    }

    let mut deleted_ids = Vec::new();
    let mut cleaned_ids = Vec::new();
    delete_subtree(
        board,
        &mut graph,
        &args.id,
        args.force,
        now,
        &mut deleted_ids,
        &mut cleaned_ids,
    )?;

    let outcome = board.commit(&format!("orbit: delete {}", args.id), false)?;
    let subtasks_deleted = deleted_ids.len().saturating_sub(1);
    let human = if subtasks_deleted == 0 {
        format!("deleted {}", args.id)
    } else {
        format!("deleted {} and {} SubTask(s)", args.id, subtasks_deleted)
    };
    let mut output = crate::output::CommandOutput::new(
        json!({
            "id": args.id,
            "deleted": true,
            "deleted_ids": deleted_ids,
            "cleaned": cleaned_ids,
        }),
        human,
    );
    if let CommitOutcome::Failed(message) = outcome {
        output.warnings.push(format!("COMMIT_FAILED: {message}"));
    }
    Ok(output)
}

fn delete_subtree(
    board: &Board,
    graph: &mut Graph,
    id: &str,
    force: bool,
    now: chrono::DateTime<Utc>,
    deleted: &mut Vec<String>,
    cleaned: &mut Vec<String>,
) -> Result<()> {
    let dependent_ids: Vec<String> = graph
        .dependents(id)
        .iter()
        .map(|dependent| dependent.id.clone())
        .collect();
    if !dependent_ids.is_empty() && !force {
        return Err(OrbitError::new(
            code::DEPENDENCY_BLOCKED,
            format!("{id} has dependents: {}", dependent_ids.join(", ")),
        )
        .with_details(json!({ "task": id, "dependents": dependent_ids })));
    }

    if force {
        for dependent in &dependent_ids {
            let updated = {
                let task = graph.get_mut(dependent)?;
                task.front.depends_on.retain(|dep| dep != id);
                task.front.updated_at = now;
                task.clone()
            };
            write_task(board, &updated)?;
            if !cleaned.iter().any(|seen| seen == dependent) {
                cleaned.push(dependent.clone());
            }
        }
    }

    let child_ids: Vec<String> = graph
        .children(id)
        .iter()
        .map(|task| task.id.clone())
        .collect();
    for child in child_ids {
        delete_subtree(board, graph, &child, force, now, deleted, cleaned)?;
    }

    let path = board.task_path(id);
    match std::fs::remove_file(&path) {
        Ok(()) => {}
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => {
            return Err(OrbitError::io(format!("delete {}", path.display()), error));
        }
    }

    let comments_path = board.comment_path(id);
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

    graph.tasks.remove(id);
    deleted.push(id.to_string());
    Ok(())
}

fn cmd_prime(board: &Board, args: PrimeArgs) -> Result<crate::output::CommandOutput> {
    let (graph, _) = Graph::load(board, LoadMode::Strict)?;
    let label_filters = normalize_label_filters(&args.label, args.repo.as_deref())?;
    let counts = graph.counts_matching(&label_filters);
    let agent = optional_agent(args.by).unwrap_or_else(|| "unknown (set ORBIT_AGENT)".to_string());
    let instructions = format!(
        "Orbit is the task tracker for this repository.\n\
Use the CLI; never edit task files directly.\n\
- `orbit ready --claim --by <agent>` takes one ready task.\n\
- `orbit heartbeat <id> --by <agent>` extends the lease while working.\n\
{COMPLETION_LINE}\n\
- `orbit add \"title\" -p N --depends-on <id>` creates work.\n\
- `orbit depend <id> <dep-id>` links work; cycles and unfinished dependencies are rejected.\n\
- A Parent Task whose SubTasks are all resolved appears under close candidates; close it explicitly when the family is done.\n\
- `orbit show <id>` reads the full body."
    );

    let ready: Vec<Value> = graph
        .ready_tasks()
        .into_iter()
        .filter(|task| task.has_labels(&label_filters))
        .take(10)
        .map(|task| graph::task_summary(&graph, &task))
        .collect();
    let close_candidates: Vec<Value> = graph
        .close_candidates()
        .into_iter()
        .filter(|task| task.has_labels(&label_filters))
        .take(10)
        .map(|task| graph::task_summary(&graph, task))
        .collect();
    let stale: Vec<Value> = graph
        .stale_tasks()
        .into_iter()
        .filter(|task| task.has_labels(&label_filters))
        .take(10)
        .map(|task| graph::task_summary(&graph, task))
        .collect();

    let mut human = String::new();
    human.push_str(&instructions);
    human.push_str("\n\n");
    human.push_str(&format!("agent: {agent}\n"));
    if !label_filters.is_empty() {
        human.push_str(&format!("scope: {}\n", label_filters.join(", ")));
    }
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
    human.push_str("close candidates:\n");
    if close_candidates.is_empty() {
        human.push_str("  (none)\n");
    } else {
        for task in &close_candidates {
            human.push_str(&format!(
                "  {}  (ready to close)\n",
                format_summary_value(task)
            ));
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
            "labels": label_filters,
            "counts": counts,
            "ready": ready,
            "close_candidates": close_candidates,
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

fn cmd_export(board: &Board) -> Result<crate::output::CommandOutput> {
    let (graph, _) = Graph::load(board, LoadMode::Strict)?;
    let mut ids: Vec<&String> = graph.tasks.keys().collect();
    ids.sort();

    let mut tasks = Vec::with_capacity(ids.len());
    let mut comment_count = 0;
    for id in ids {
        let comments = board.read_comments(id)?;
        comment_count += comments.len();
        let task = graph.get(id)?;
        tasks.push(graph::task_detail_with_comments(&graph, task, &comments));
    }

    let task_count = tasks.len();
    let snapshot = json!({
        "schema_version": 2,
        "board": {
            "branch": board.config.board.branch,
            "prefix": board.config.id.prefix,
            "task_count": task_count,
            "comment_count": comment_count,
        },
        "tasks": tasks,
    });
    Ok(crate::output::CommandOutput::new(
        snapshot,
        format!("exported {task_count} tasks and {comment_count} comments (schema 2)"),
    ))
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
    let task = graph.get(id)?;
    let gate = ClaimGate {
        hold_labels: graph.hold_labels(task),
        blocking: graph.blocking_ids(task),
        open_subtasks: graph.open_children(id),
    };
    graph.get_mut(id)?.claim(
        agent,
        steal,
        ChronoDuration::minutes(lease_minutes),
        &gate,
        Utc::now(),
    )?;
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

fn generate_unique_id(board: &Board, parent: Option<&str>) -> Result<String> {
    let config = &board.config.id;
    for attempt in 0..50 {
        let length = if attempt < 40 {
            config.length
        } else {
            (config.length + 2).min(8)
        };
        let id = match parent {
            Some(parent) => task::generate_subtask_id(parent, length),
            None => task::generate_id(&config.prefix, length),
        };
        if !board.task_path(&id).exists() {
            return Ok(id);
        }
    }
    Err(OrbitError::new(
        code::ID_COLLISION,
        "could not generate a unique task id",
    ))
}

fn ready_for_filters(
    mut ready: Vec<Task>,
    parent: Option<&str>,
    labels: &[String],
    excluded: &[String],
    types: &[TaskType],
    excluded_types: &[TaskType],
) -> Vec<Task> {
    if let Some(parent) = parent {
        ready.retain(|task| task.front.parent.as_deref() == Some(parent));
    }
    if !labels.is_empty() {
        ready.retain(|task| task.has_labels(labels));
    }
    if !excluded.is_empty() {
        ready.retain(|task| !task.has_any_label(excluded));
    }
    if !types.is_empty() {
        ready.retain(|task| types.contains(&task.front.task_type));
    }
    if !excluded_types.is_empty() {
        ready.retain(|task| !excluded_types.contains(&task.front.task_type));
    }
    ready
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

fn list_sort_key<'a>(
    graph: &'a Graph,
    task: &'a Task,
) -> (u8, i64, &'a str, bool, u8, i64, &'a str) {
    let root = task
        .front
        .parent
        .as_deref()
        .and_then(|parent_id| graph.tasks.get(parent_id))
        .unwrap_or(task);
    (
        root.front.priority,
        root.front.created_at.timestamp_millis(),
        root.id.as_str(),
        task.front.parent.is_some(),
        task.front.priority,
        task.front.created_at.timestamp_millis(),
        task.id.as_str(),
    )
}

struct ListGroup<'a> {
    parent: Option<&'a Task>,
    tasks: Vec<&'a Task>,
}

fn group_tasks<'a>(graph: &'a Graph, tasks: &[&'a Task]) -> Result<Vec<ListGroup<'a>>> {
    let mut groups: Vec<ListGroup<'a>> = Vec::new();
    let mut positions: BTreeMap<&'a str, usize> = BTreeMap::new();
    let mut standalone: Vec<&'a Task> = Vec::new();

    for task in tasks {
        if let Some(parent_id) = task.front.parent.as_deref() {
            if let Some(position) = positions.get(parent_id) {
                groups[*position].tasks.push(task);
            } else {
                let parent = graph.get(parent_id)?;
                positions.insert(parent_id, groups.len());
                groups.push(ListGroup {
                    parent: Some(parent),
                    tasks: vec![task],
                });
            }
        } else {
            standalone.push(task);
        }
    }

    let banner_ids: Vec<&str> = groups
        .iter()
        .filter_map(|group| group.parent.map(|parent| parent.id.as_str()))
        .collect();
    standalone.retain(|task| !banner_ids.contains(&task.id.as_str()));

    if !standalone.is_empty() {
        groups.push(ListGroup {
            parent: None,
            tasks: standalone,
        });
    }

    Ok(groups)
}

fn group_headers() -> [String; 7] {
    ["ID", "STATUS", "P", "TYPE", "TITLE", "LABELS", "CLAIMED BY"].map(String::from)
}

fn task_cells(task: &Task) -> [String; 7] {
    [
        task.id.to_string(),
        task.front.status.as_str().to_string(),
        format!("P{}", task.front.priority),
        task.front.task_type.as_str().to_string(),
        truncate(&task.front.title, 60),
        task.front.labels.join(", "),
        task.holder()
            .map(|holder| holder.to_string())
            .unwrap_or_default(),
    ]
}

fn column_widths(rows: &[[String; 7]]) -> Vec<usize> {
    (0..7)
        .map(|column| {
            rows.iter()
                .map(|row| row[column].chars().count())
                .max()
                .unwrap_or(0)
        })
        .collect()
}

fn format_table(tasks: &[&Task]) -> String {
    let mut rows = vec![group_headers()];
    rows.extend(tasks.iter().map(|task| task_cells(task)));
    let widths = column_widths(&rows);
    rows.iter()
        .map(|row| {
            row.iter()
                .zip(&widths)
                .map(|(cell, width)| format!("{cell:<width$}"))
                .collect::<Vec<_>>()
                .join("  ")
                .trim_end()
                .to_string()
        })
        .collect::<Vec<_>>()
        .join("\n")
}

fn format_grouped_table(groups: &[ListGroup]) -> String {
    let headers = group_headers();
    let mut rows = vec![headers.clone()];
    rows.extend(
        groups
            .iter()
            .flat_map(|group| group.tasks.iter().map(|task| task_cells(task))),
    );
    let widths = column_widths(&rows);
    let inner_width = widths.iter().map(|width| width + 2).sum::<usize>() + widths.len() - 1;

    let mut lines = Vec::new();
    lines.push(column_border("╭", "┬", "╮", &widths));
    lines.push(table_row(&headers, &widths));
    for group in groups {
        lines.push(full_border("├", "┤", inner_width));
        if let Some(parent) = group.parent {
            let left = format!("{}  {}", parent.id, truncate(&parent.front.title, 60));
            lines.push(banner_row(&left, &group_meta(parent), inner_width));
        } else {
            lines.push(banner_row("No parent", "", inner_width));
        }
        lines.push(full_border("├", "┤", inner_width));
        for task in &group.tasks {
            lines.push(table_row(&task_cells(task), &widths));
        }
    }
    lines.push(column_border("╰", "┴", "╯", &widths));
    lines.join("\n")
}

fn group_meta(task: &Task) -> String {
    let mut parts = vec![
        task.front.task_type.as_str().to_string(),
        task.front.status.as_str().to_string(),
        format!("P{}", task.front.priority),
    ];
    if !task.front.labels.is_empty() {
        parts.push(task.front.labels.join(", "));
    }
    if let Some(holder) = task.holder() {
        parts.push(format!("claimed by {holder}"));
    }
    parts.join(" · ")
}

fn column_border(left: &str, middle: &str, right: &str, widths: &[usize]) -> String {
    let segments: Vec<String> = widths.iter().map(|width| "─".repeat(width + 2)).collect();
    format!("{left}{}{right}", segments.join(middle))
}

fn full_border(left: &str, right: &str, inner_width: usize) -> String {
    format!("{left}{}{right}", "─".repeat(inner_width))
}

fn table_row(cells: &[String; 7], widths: &[usize]) -> String {
    let padded: Vec<String> = cells
        .iter()
        .zip(widths)
        .map(|(cell, width)| format!(" {cell:<width$} "))
        .collect();
    format!("│{}│", padded.join("│"))
}

fn banner_row(left: &str, right: &str, inner_width: usize) -> String {
    let available = inner_width.saturating_sub(2);
    let mut left = left.to_string();
    let mut right = right.to_string();

    if right.chars().count() + 1 > available {
        right = truncate(&right, available.saturating_sub(1));
    }

    let max_left = if right.is_empty() {
        available
    } else {
        available.saturating_sub(right.chars().count() + 1)
    };
    if left.chars().count() > max_left {
        left = truncate(&left, max_left);
    }

    let gap = if right.is_empty() {
        available.saturating_sub(left.chars().count())
    } else {
        available
            .saturating_sub(left.chars().count() + right.chars().count())
            .max(1)
    };
    format!("│ {left}{}{right} │", " ".repeat(gap))
}

fn format_summary(task: &Task) -> String {
    format_summary_with_parent(task, true)
}

fn format_summary_with_parent(task: &Task, include_parent: bool) -> String {
    let claim = task
        .holder()
        .map(|holder| format!("  claimed by {holder}"))
        .unwrap_or_default();
    let labels = if task.front.labels.is_empty() {
        String::new()
    } else {
        format!("  [{}]", task.front.labels.join(", "))
    };
    let type_prefix = if task.front.task_type == TaskType::Task {
        String::new()
    } else {
        format!("{}  ", task.front.task_type.as_str())
    };
    let parent = if include_parent {
        task.front
            .parent
            .as_deref()
            .map(|parent| format!("  (subtask of {parent})"))
            .unwrap_or_default()
    } else {
        String::new()
    };
    format!(
        "{}  {:<11}  P{}  {}{}{}{}{}",
        task.id,
        task.front.status.as_str(),
        task.front.priority,
        type_prefix,
        task.front.title,
        labels,
        claim,
        parent
    )
}

fn format_tree_line(task: &Task, graph: &Graph, root: bool) -> String {
    let mut line = format_summary_with_parent(task, false);
    if root {
        let subtask_count = graph.children(&task.id).len();
        if subtask_count > 0 {
            let open_count = graph.open_children(&task.id).len();
            let resolved = subtask_count.saturating_sub(open_count);
            line.push_str(&format!("  {resolved}/{subtask_count} resolved"));
            if open_count == 0 && !task.front.status.is_terminal() {
                line.push_str("  (ready to close)");
            }
        }
    }
    let blocking = graph.blocked_by(task);
    if !blocking.is_empty() {
        line.push_str(&format!("  blocked by: {}", blocking.join(", ")));
    }
    if root { line } else { format!("  {line}") }
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
    let parent = value
        .get("parent")
        .and_then(Value::as_str)
        .map(|parent| format!("  (subtask of {parent})"))
        .unwrap_or_default();
    let type_prefix = value
        .get("type")
        .and_then(Value::as_str)
        .filter(|task_type| *task_type != "task")
        .map(|task_type| format!("{task_type}  "))
        .unwrap_or_default();
    format!("{id}  {status:<11}  P{priority}  {type_prefix}{title}{labels}{claim}{parent}")
}

fn format_detail(graph: &Graph, task: &Task, comments: &[Comment]) -> String {
    let mut lines = Vec::new();
    lines.push(format!("{}  {}", task.id, task.front.title));
    lines.push(format!("status: {}", task.front.status.as_str()));
    lines.push(format!("kind: {}", task.kind()));
    lines.push(format!("type: {}", task.front.task_type.as_str()));
    lines.push(format!("priority: P{}", task.front.priority));
    if let Some(parent) = &task.front.parent {
        lines.push(format!("parent: {parent}"));
    }
    if task.front.depends_on.is_empty() {
        lines.push("depends_on: -".to_string());
    } else {
        lines.push(format!("depends_on: {}", task.front.depends_on.join(", ")));
    }
    if let Some(waiting) = &task.front.waiting_on {
        lines.push(format!("waiting_on: {waiting}"));
    }
    if let Some(claim) = &task.front.claim {
        let until = claim
            .lease_until
            .map(|until| until.to_rfc3339())
            .unwrap_or_else(|| "unknown".to_string());
        lines.push(format!("claimed_by: {} (until {until})", claim.holder));
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
    let children: Vec<&str> = graph
        .children(&task.id)
        .iter()
        .map(|child| child.id.as_str())
        .collect();
    if !children.is_empty() {
        lines.push(format!("subtasks: {}", children.join(", ")));
    }
    let open_subtasks = graph.open_children(&task.id);
    if !open_subtasks.is_empty() {
        lines.push(format!("open_subtasks: {}", open_subtasks.join(", ")));
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

fn cycle_error(path: &[String]) -> OrbitError {
    OrbitError::new(
        code::CYCLE_DETECTED,
        format!("dependency cycle: {}", path.join(" -> ")),
    )
    .with_details(json!({ "cycle": path }))
}

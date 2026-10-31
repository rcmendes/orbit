use std::io::IsTerminal;

use serde_json::{Value, json};

use crate::cli::InitArgs;
use crate::slug;

use super::*;

struct InitTarget {
    board_root: std::path::PathBuf,
    created: bool,
    prefix: String,
    storage: crate::board::StorageMode,
    branch: String,
    exclude: Option<String>,
    warnings: Vec<String>,
}

pub(super) fn cmd_init(args: InitArgs, dir: Option<&Path>) -> Result<crate::output::CommandOutput> {
    let cwd = std::env::current_dir()
        .map_err(|error| OrbitError::io("resolve current directory", error))?;
    let cwd = dunce::canonicalize(&cwd).unwrap_or(cwd);
    let in_repo = crate::git::is_git_repo(&cwd);

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
    let (requested_root, explicit_global) = if args.global {
        (global_dir.clone().expect("global home was checked"), true)
    } else if let Some(target) = args.target.as_deref() {
        (
            resolve_init_root(crate::board::resolve_external(target)?)?,
            false,
        )
    } else if let Some(dir) = dir {
        (
            resolve_init_root(crate::board::resolve_external(dir)?)?,
            false,
        )
    } else {
        (default_init_dir(&cwd)?, false)
    };
    let workspace_root = is_workspace_root(&requested_root);
    let setup_root = if workspace_root {
        requested_root.clone()
    } else if in_repo {
        crate::git::repo_root()?
    } else {
        cwd.clone()
    };
    let board_root = if workspace_root {
        requested_root.join(".orbit").join("board")
    } else {
        requested_root.clone()
    };
    let global = explicit_global
        || global_dir
            .as_ref()
            .is_some_and(|global| global == &board_root);

    if workspace_root {
        validate_workspace_layout(&requested_root)?;
        if !Board::is_board(&board_root)
            && let Some(existing) = crate::board::workspace_child_board(&requested_root)?
        {
            return Err(crate::board::already_initialized(&existing));
        }
    }

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
    } = run_init(
        &board_root,
        workspace_root.then_some(requested_root.as_path()),
        args.slug.as_deref(),
        args.print,
        global,
    )?;

    if workspace_root && !args.print {
        std::fs::create_dir_all(requested_root.join(".orbit/cache")).map_err(|error| {
            OrbitError::io(
                format!("create {}", requested_root.join(".orbit/cache").display()),
                error,
            )
        })?;
    }

    if !args.print && storage == crate::board::StorageMode::Worktree {
        exclude = Board::ensure_excluded(&board_root)?;
    }

    let mut file_outcomes: Vec<crate::setup::FileOutcome> = Vec::new();
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
        let outcome = crate::setup::install(
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
        file_outcomes = outcome.files;
        warnings.extend(outcome.output.warnings);
    }
    let files: Vec<Value> = file_outcomes
        .iter()
        .map(|file| file.to_json(args.print))
        .collect();
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
        files: &file_outcomes,
        mcp_command: mcp_command.as_deref(),
        portable_mcp: reported_portable_mcp,
        workspace: workspace_root,
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
    files: &'a [crate::setup::FileOutcome],
    mcp_command: Option<&'a str>,
    portable_mcp: bool,
    workspace: bool,
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
        (crate::board::StorageMode::Repository, false) if report.workspace => {
            "repository (Workspace Board at <workspace>/.orbit/board)"
        }
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
        if report.files.is_empty() {
            human.push_str("     already up to date\n");
        } else {
            for file in report.files {
                let path = file.relative.to_string_lossy();
                human.push_str(&format!(
                    "     {:<13}{path}\n",
                    init_action_label(preview, file.action.as_str())
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
        {
            let previews = report
                .files
                .iter()
                .filter_map(|file| {
                    Some((
                        file.relative.to_string_lossy().into_owned(),
                        file.content.as_deref()?,
                    ))
                })
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
    let external = report.storage == crate::board::StorageMode::Repository
        && !report.global
        && !report.workspace;
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
    slug_root: Option<&Path>,
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
        None => slug::auto_slug_for_dir(slug_root.unwrap_or(board_root)),
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
    let discovery = Board::discovery(None);
    if discovery.source == crate::board::DiscoverySource::Workspace
        && let Some(board) = discovery.root
        && let Some(workspace) = board.parent().and_then(Path::parent)
    {
        return Ok(workspace.to_path_buf());
    }
    let Ok(repo_root) = crate::git::repo_root() else {
        return Ok(cwd.to_path_buf());
    };
    let repo_root = dunce::canonicalize(&repo_root).unwrap_or(repo_root);
    let main = slug::main_checkout_anchor().unwrap_or_else(|_| repo_root.clone());
    if repo_root != main {
        return Ok(main.join(".orbit"));
    }
    Ok(cwd.join(".orbit"))
}

/// Git cannot add a Board worktree over a populated checkout, so a target that
/// is itself a working-tree root means "initialize here": use the repository's
/// `.orbit`, or the main checkout's `.orbit` from a linked worktree, exactly
/// like `orbit init` without a target. `orbit init .` therefore does not fail
/// with a raw `git worktree add` error.
fn resolve_init_root(root: std::path::PathBuf) -> Result<std::path::PathBuf> {
    let Some(toplevel) = crate::git::toplevel_containing(&root) else {
        return Ok(root);
    };
    let toplevel = dunce::canonicalize(&toplevel).unwrap_or(toplevel);
    if root != toplevel || Board::is_board(&root) {
        return Ok(root);
    }
    let common = crate::git::common_dir_at(&toplevel)?;
    let main = if common.file_name().and_then(|name| name.to_str()) == Some(".git") {
        common.parent().unwrap_or(&toplevel).to_path_buf()
    } else {
        toplevel
    };
    Ok(main.join(".orbit"))
}

/// A non-Git directory is the Workspace boundary. Existing direct Board paths
/// remain accepted when they already contain a Board or use a legacy Board
/// directory name; new workspaces keep their Board under `.orbit/board`.
fn is_workspace_root(path: &Path) -> bool {
    if crate::git::toplevel_containing(path).is_some() || Board::is_board(path) {
        return false;
    }
    !matches!(
        path.file_name().and_then(|name| name.to_str()),
        Some(".orbit" | "board")
    )
}

fn validate_workspace_layout(workspace: &Path) -> Result<()> {
    let orbit = workspace.join(".orbit");
    if Board::is_board(&orbit) {
        return Err(OrbitError::config(format!(
            "found a legacy Board at {}; it remains available with `--dir {}`. Back it up and migrate it to {}/.orbit/board before initializing this Workspace",
            orbit.display(),
            orbit.display(),
            workspace.display()
        )));
    }
    let entries = match std::fs::read_dir(&orbit) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(OrbitError::io(format!("read {}", orbit.display()), error)),
    };
    for entry in entries {
        let entry =
            entry.map_err(|error| OrbitError::io(format!("read {}", orbit.display()), error))?;
        let name = entry.file_name();
        if name != "board" && name != "cache" && name != "workspace-migration.lock" {
            return Err(crate::board::already_initialized(&entry.path()));
        }
    }
    let board = orbit.join("board");
    if board.exists() && !Board::is_board(&board) {
        return Err(crate::board::already_initialized(&board));
    }
    let cache = orbit.join("cache");
    if cache.exists() && !cache.is_dir() {
        return Err(crate::board::already_initialized(&cache));
    }
    Ok(())
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

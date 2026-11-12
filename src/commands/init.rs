use std::io::{BufRead, IsTerminal, Write};
use std::path::PathBuf;

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

    let home = resolve_home_dir().ok();
    if args.global && home.is_none() {
        return Err(OrbitError::config(
            "HOME or USERPROFILE must be set for `orbit init --global`",
        ));
    }
    let global_dir = home.as_ref().map(|home| home.join(".orbit"));
    let visible_choice = VisibleChoice::from_args(&args.visible)?;
    let (requested_root, explicit_global) = if args.global {
        (global_dir.clone().expect("global home was checked"), true)
    } else if let Some(target) = args.target.as_deref() {
        (crate::board::resolve_external(target)?, false)
    } else if let Some(dir) = dir {
        (crate::board::resolve_external(dir)?, false)
    } else {
        (default_init_dir(&cwd)?, false)
    };
    // The implicit target is the repository's `.orbit` anchor; `--visible`
    // means "the enclosing root", so resolve it one level up. An explicit
    // `.orbit` target stays an error inside `init_layout`.
    let requested_root = if matches!(visible_choice, VisibleChoice::Named(_))
        && args.target.is_none()
        && dir.is_none()
        && requested_root.file_name().and_then(|name| name.to_str()) == Some(".orbit")
    {
        requested_root
            .parent()
            .map(Path::to_path_buf)
            .unwrap_or(requested_root)
    } else {
        requested_root
    };
    let workspace_root = is_workspace_root(&requested_root);
    let setup_root = if workspace_root {
        requested_root.clone()
    } else if in_repo {
        crate::git::repo_root()?
    } else {
        cwd.clone()
    };
    // ADR 0019: the Board folder is a standalone repository. Hidden Boards
    // live at `<anchor>/.orbit/board`; a visible folder is anchored by the
    // `.orbit/board` pointer file instead.
    let (mut orbit_dir, mut board_root, mut visible_board) = init_layout(
        &requested_root,
        workspace_root,
        explicit_global,
        global_dir.as_deref(),
        &visible_choice,
    )?;
    // The Orbit folder may already anchor a Board: a hidden `.orbit/board`
    // directory or a `board` pointer file naming a visible folder. Reuse
    // whichever it resolves to instead of assuming the computed layout.
    if let Some(resolved) = crate::board::resolve_orbit_dir(&orbit_dir) {
        board_root = resolved.root().to_path_buf();
        visible_board = matches!(resolved, crate::board::OrbitBoard::Visible { .. });
    }
    let global = explicit_global
        || global_dir
            .as_ref()
            .is_some_and(|global| global.join("board") == board_root);

    // Interactive runs ask for the Board layout (when the default would be
    // hidden) and for a host. The stdin lock is scoped so `selected_agents`
    // can take its own lock afterwards.
    let interactive = std::io::stdin().is_terminal();
    let mut prompted_host = None;
    if interactive && !args.print && !Board::is_board(&board_root) {
        let stdin = std::io::stdin();
        let mut input = stdin.lock();
        let mut prompt = std::io::stderr();
        if !visible_board
            && matches!(visible_choice, VisibleChoice::Default)
            && prompt_board_layout(&mut input, &mut prompt)?
        {
            let name = prompt_board_name(&mut input, &mut prompt)?;
            let choice = VisibleChoice::Named(Some(name));
            let (orbit, root, visible) = init_layout(
                &requested_root,
                workspace_root,
                explicit_global,
                global_dir.as_deref(),
                &choice,
            )?;
            orbit_dir = orbit;
            board_root = root;
            visible_board = visible;
            if let Some(resolved) = crate::board::resolve_orbit_dir(&orbit_dir) {
                board_root = resolved.root().to_path_buf();
                visible_board = matches!(resolved, crate::board::OrbitBoard::Visible { .. });
            }
        }
        if args.host.is_none() {
            prompted_host = prompt_board_host(&mut input, &mut prompt)?;
        }
    }
    if let VisibleChoice::Named(Some(name)) = &visible_choice
        && visible_board
        && board_root.file_name().and_then(|n| n.to_str()) != Some(name.as_str())
    {
        return Err(OrbitError::invalid_argument(format!(
            "--visible={name} does not match the Board folder {}",
            board_root.display()
        )));
    }
    let host_spec = match &args.host {
        Some(Some(spec)) => Some(spec.clone()),
        Some(None) => {
            if interactive && !args.print {
                let stdin = std::io::stdin();
                let mut input = stdin.lock();
                let mut prompt = std::io::stderr();
                prompt_board_host(&mut input, &mut prompt)?
            } else {
                return Err(OrbitError::invalid_argument(
                    "--host requires a value when the command is not interactive; pass --host=<repo|url>",
                ));
            }
        }
        None => prompted_host,
    };

    if workspace_root {
        validate_workspace_layout(&requested_root)?;
        if !Board::is_board(&board_root)
            && let Some(existing) = crate::board::workspace_child_board(&requested_root)?
        {
            return Err(crate::board::already_initialized(&existing));
        }
    }

    // A Board that still uses a pre-ADR-0019 layout must go through
    // `orbit migrate`; `init` must not silently reuse or duplicate it.
    for candidate in [&orbit_dir, &board_root] {
        let discovery = Board::discovery(Some(candidate));
        if let Some(root) = discovery.root.clone()
            && let Ok(board) = Board::at_root(root, Vec::new())
            && board.needs_migration(Some(&discovery))
        {
            return Err(OrbitError::new(
                code::BOARD_NOT_MIGRATED,
                format!(
                    "the Board at {} uses a pre-ADR-0019 layout; run `orbit migrate` to convert it",
                    board.root.display()
                ),
            ));
        }
    }

    let clients = crate::setup::selected_agents(&args, &setup_root)?;
    let selected_portable_mcp = crate::setup::selected_portable_mcp(&args, &clients, &setup_root)?;
    let ignored_portable_mcp = args.portable_mcp && !clients.iter().any(|client| client.uses_mcp());

    let InitTarget {
        board_root,
        created,
        prefix,
        storage,
        branch,
        mut warnings,
    } = run_init(
        &board_root,
        // Hidden Boards derive the slug from their Orbit folder so `.orbit`
        // keeps meaning "the enclosing folder's name"; visible Board folders
        // derive it from the folder itself.
        Some(if visible_board {
            board_root.as_path()
        } else {
            orbit_dir.as_path()
        }),
        args.slug.as_deref(),
        args.print,
        global,
    )?;

    // `--host` / the host prompt: validate the host and, for a fresh or still
    // empty Board, adopt an existing same-Slug host branch instead of
    // publishing divergent history over it. `host_spec` is normalized to the
    // value `board.host` stores (canonical paths, verbatim URLs/names).
    let host_spec = host_spec
        .map(|spec| crate::board_host::normalize_host_spec(&spec))
        .transpose()?;
    let mut host_joined = false;
    if !args.print
        && let Some(spec) = &host_spec
    {
        let join = created
            || Board::discover(Some(&board_root))
                .and_then(|board| board.task_ids())
                .is_ok_and(|ids| ids.is_empty());
        let outcome = crate::board_host::set_host(&board_root, spec, join)?;
        host_joined = outcome.joined;
    }

    if !args.print {
        // The Orbit folder anchors the Board: `.orbit/board` hidden, or a
        // `board` pointer file naming a visible Board folder. `cache/` sits
        // beside the Board folder inside the Orbit folder.
        if visible_board {
            std::fs::create_dir_all(&orbit_dir).map_err(|error| {
                OrbitError::io(format!("create {}", orbit_dir.display()), error)
            })?;
            crate::board::write_pointer_file(&orbit_dir, &board_root)?;
        } else {
            std::fs::create_dir_all(orbit_dir.join("board")).map_err(|error| {
                OrbitError::io(
                    format!("create {}", orbit_dir.join("board").display()),
                    error,
                )
            })?;
        }
        let cache = orbit_dir.join("cache");
        std::fs::create_dir_all(&cache)
            .map_err(|error| OrbitError::io(format!("create {}", cache.display()), error))?;
    }

    // A Board folder inside a Git working tree is hidden only through
    // `.git/info/exclude`, never `.gitignore` (ADR 0019).
    let exclude = if !args.print {
        crate::board::ensure_orbit_excluded(
            &orbit_dir,
            visible_board.then_some(board_root.as_path()),
        )?
    } else {
        crate::board::orbit_exclude_entries(
            &orbit_dir,
            visible_board.then_some(board_root.as_path()),
        )?
    };

    let mut file_outcomes: Vec<crate::setup::FileOutcome> = Vec::new();
    let mut files: Vec<Value> = Vec::new();
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
        // `install` already serialized `files` with the same `args.print`
        // content gating, so reuse its wire shape instead of re-serializing.
        files = outcome.output.data["files"]
            .as_array()
            .cloned()
            .unwrap_or_default();
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
        exclude: exclude.as_slice(),
        global,
        host: host_spec.as_deref(),
        host_joined,
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
            "host": host_spec,
            "host_joined": host_joined,
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
    exclude: &'a [String],
    global: bool,
    host: Option<&'a str>,
    host_joined: bool,
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
    let storage = if report.workspace {
        "repository (Workspace Board at <workspace>/.orbit/board)"
    } else {
        "repository (standalone Board folder on branch `orbit`)"
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
    if let Some(host) = report.host {
        let detail = if report.host_joined {
            format!("{host} (joined the existing host `orbit` branch)")
        } else {
            host.to_string()
        };
        human.push_str(&format!("   {:<9}{detail}\n", "host"));
    }
    for exclude in report.exclude {
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
    human.push_str("   orbit doctor   # check the Board and agent setup\n\n");

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
        return Ok(InitTarget {
            board_root: board_root.to_path_buf(),
            created: false,
            prefix,
            storage: crate::board::StorageMode::Repository,
            branch: board.config.board.branch,
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

    if print {
        return Ok(InitTarget {
            board_root: board_root.to_path_buf(),
            created: true,
            prefix,
            storage: crate::board::StorageMode::Repository,
            branch: "orbit".to_string(),
            warnings: warning.into_iter().collect(),
        });
    }

    let result = Board::init_dir(board_root, Some(&slug))?;
    Ok(InitTarget {
        board_root: result.board,
        created: true,
        prefix: result.prefix,
        storage: result.storage,
        branch: result.branch,
        warnings: warning.into_iter().collect(),
    })
}

/// How `--visible` / the interactive layout prompt resolved the Board folder.
#[derive(Debug, Clone, PartialEq, Eq)]
enum VisibleChoice {
    /// The default layout for the target (hidden `.orbit/board` where it applies).
    Default,
    /// A visible Board folder; `None` is bare `--visible` (name `board`).
    Named(Option<String>),
}

impl VisibleChoice {
    fn from_args(flag: &Option<Option<String>>) -> Result<Self> {
        match flag {
            None => Ok(Self::Default),
            Some(None) => Ok(Self::Named(None)),
            Some(Some(name)) => Ok(Self::Named(Some(board_folder_name(name)?))),
        }
    }

    /// The folder name a visible choice implies, when it forces one.
    fn forced_name(&self) -> Option<String> {
        match self {
            Self::Default => None,
            Self::Named(name) => Some(name.clone().unwrap_or_else(|| "board".to_string())),
        }
    }
}

/// A visible Board folder name must be a single path component.
fn board_folder_name(name: &str) -> Result<String> {
    let name = name.trim();
    if name.is_empty() || name == "." || name == ".." || name.contains('/') || name.contains('\\') {
        return Err(OrbitError::invalid_argument(format!(
            "invalid Board folder name `{name}`; use a single directory name"
        )));
    }
    Ok(name.to_string())
}

/// Maps the requested init target to the ADR 0019 layout: the Orbit folder
/// and the Board folder it anchors. Returns `(orbit_dir, board_dir, visible)`
/// where `visible` means the Board folder is named explicitly and `.orbit`
/// holds a pointer file instead of a `board/` directory.
fn init_layout(
    requested: &Path,
    workspace_root: bool,
    global: bool,
    global_dir: Option<&Path>,
    visible: &VisibleChoice,
) -> Result<(PathBuf, PathBuf, bool)> {
    if global {
        let orbit = global_dir
            .ok_or_else(|| OrbitError::config("HOME or USERPROFILE is not set"))?
            .to_path_buf();
        return Ok((orbit.clone(), orbit.join("board"), false));
    }
    // An Orbit folder target anchors a hidden `.orbit/board` Board.
    if requested.file_name().and_then(|name| name.to_str()) == Some(".orbit") {
        if matches!(visible, VisibleChoice::Named(_)) {
            return Err(OrbitError::invalid_argument(
                "--visible cannot anchor a Board inside an Orbit folder; pass a directory or omit --visible",
            ));
        }
        return Ok((requested.to_path_buf(), requested.join("board"), false));
    }
    if workspace_root {
        let orbit = requested.join(".orbit");
        return match visible.forced_name() {
            Some(name) => Ok((orbit, requested.join(name), true)),
            None => Ok((orbit.clone(), orbit.join("board"), false)),
        };
    }
    // The target's own Git membership decides the layout, not the caller's
    // directory: `orbit init /elsewhere/board` from inside a repository still
    // creates a standalone Board outside it.
    if let Some(toplevel) = crate::git::toplevel_containing(requested) {
        let toplevel = dunce::canonicalize(&toplevel).unwrap_or(toplevel);
        let canonical_requested =
            dunce::canonicalize(requested).unwrap_or_else(|_| requested.to_path_buf());
        if toplevel == canonical_requested {
            // Re-initializing an existing Board folder: the target is itself
            // a standalone Board repository. Anchor it through the Orbit
            // folder that already names it — a hidden `.orbit/board` parent,
            // the enclosing repository's `.orbit`, or a sibling `.orbit`.
            if Board::is_board(requested) {
                let orbit = requested
                    .parent()
                    .filter(|parent| {
                        parent.file_name().and_then(|name| name.to_str()) == Some(".orbit")
                    })
                    .map(|parent| parent.to_path_buf())
                    .or_else(|| {
                        requested.parent().and_then(|parent| {
                            crate::git::toplevel_containing(parent).and_then(|toplevel| {
                                let common = crate::git::common_dir_at(&toplevel).ok()?;
                                Some(
                                    crate::board::main_checkout_for(&toplevel, &common)
                                        .join(".orbit"),
                                )
                            })
                        })
                    })
                    .or_else(|| requested.parent().map(|parent| parent.join(".orbit")))
                    .unwrap_or_else(|| requested.join(".orbit"));
                return Ok((orbit, requested.to_path_buf(), true));
            }
            let common = crate::git::common_dir_at(&toplevel)?;
            let main = crate::board::main_checkout_for(&toplevel, &common);
            let orbit = main.join(".orbit");
            return match visible.forced_name() {
                Some(name) => Ok((orbit, toplevel.join(name), true)),
                None => Ok((orbit.clone(), orbit.join("board"), false)),
            };
        }
        // Any other directory inside the repository is a visible Board
        // folder anchored by the repository's Orbit folder.
        let common = crate::git::common_dir_at(&toplevel)?;
        let main = crate::board::main_checkout_for(&toplevel, &common);
        let orbit = main.join(".orbit");
        return Ok((orbit, requested.to_path_buf(), true));
    }
    // Outside Git and not a Workspace root: a `board`-named or existing Board
    // directory becomes a visible Board folder anchored by a sibling `.orbit`.
    let orbit = requested
        .parent()
        .map(|parent| parent.join(".orbit"))
        .unwrap_or_else(|| requested.join(".orbit"));
    Ok((orbit, requested.to_path_buf(), true))
}

fn default_init_dir(cwd: &Path) -> Result<std::path::PathBuf> {
    let discovery = Board::discovery(None);
    if discovery.source == crate::board::DiscoverySource::Workspace
        && let Some(board) = discovery.root
    {
        // The Workspace root is the Orbit folder's parent.
        let workspace = discovery
            .orbit_dir
            .as_ref()
            .and_then(|orbit| orbit.parent())
            .or_else(|| board.parent().and_then(Path::parent))
            .map(|path| path.to_path_buf());
        if let Some(workspace) = workspace {
            return Ok(workspace);
        }
    }
    let Ok(repo_root) = crate::git::repo_root() else {
        return Ok(cwd.to_path_buf());
    };
    let repo_root = dunce::canonicalize(&repo_root).unwrap_or(repo_root);
    let main = slug::main_checkout_anchor().unwrap_or_else(|_| repo_root.clone());
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
    // `board` may be the hidden Board folder or the pointer file naming a
    // visible one; only a non-Board directory is a conflict.
    if board.is_dir() && !Board::is_board(&board) {
        return Err(crate::board::already_initialized(&board));
    }
    let cache = orbit.join("cache");
    if cache.exists() && !cache.is_dir() {
        return Err(crate::board::already_initialized(&cache));
    }
    Ok(())
}

fn resolve_home_dir() -> Result<std::path::PathBuf> {
    crate::git::home_dir()
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

fn prompt_board_layout(input: &mut impl BufRead, prompt: &mut impl Write) -> Result<bool> {
    writeln!(prompt, "Board layout:")
        .map_err(|error| OrbitError::io("write the layout prompt", error))?;
    writeln!(prompt, "  1. hidden .orbit/board (default)")
        .map_err(|error| OrbitError::io("write the layout prompt", error))?;
    writeln!(prompt, "  2. visible folder inside the repository")
        .map_err(|error| OrbitError::io("write the layout prompt", error))?;
    writeln!(prompt, "Choose 1 or 2 [Enter = 1 hidden]")
        .map_err(|error| OrbitError::io("write the layout prompt", error))?;
    loop {
        write!(prompt, "> ").map_err(|error| OrbitError::io("write the layout prompt", error))?;
        prompt
            .flush()
            .map_err(|error| OrbitError::io("flush the layout prompt", error))?;
        let mut line = String::new();
        if input
            .read_line(&mut line)
            .map_err(|error| OrbitError::io("read the layout choice", error))?
            == 0
        {
            return Ok(false);
        }
        match line.trim() {
            "" | "1" | "hidden" => return Ok(false),
            "2" | "visible" => return Ok(true),
            _ => {
                writeln!(prompt, "Enter 1 or 2.")
                    .map_err(|error| OrbitError::io("write the layout prompt", error))?;
            }
        }
    }
}

fn prompt_board_name(input: &mut impl BufRead, prompt: &mut impl Write) -> Result<String> {
    writeln!(prompt, "Visible Board folder name [board]:")
        .map_err(|error| OrbitError::io("write the folder prompt", error))?;
    write!(prompt, "> ").map_err(|error| OrbitError::io("write the folder prompt", error))?;
    prompt
        .flush()
        .map_err(|error| OrbitError::io("flush the folder prompt", error))?;
    let mut line = String::new();
    if input
        .read_line(&mut line)
        .map_err(|error| OrbitError::io("read the folder name", error))?
        == 0
    {
        return Ok("board".to_string());
    }
    let name = line.trim();
    if name.is_empty() {
        return Ok("board".to_string());
    }
    board_folder_name(name)
}

fn prompt_board_host(input: &mut impl BufRead, prompt: &mut impl Write) -> Result<Option<String>> {
    writeln!(
        prompt,
        "Board host — a member Repository name/path or git URL whose remote stores the Board's `orbit` branch [Enter = none]:"
    )
    .map_err(|error| OrbitError::io("write the host prompt", error))?;
    write!(prompt, "> ").map_err(|error| OrbitError::io("write the host prompt", error))?;
    prompt
        .flush()
        .map_err(|error| OrbitError::io("flush the host prompt", error))?;
    let mut line = String::new();
    if input
        .read_line(&mut line)
        .map_err(|error| OrbitError::io("read the host", error))?
        == 0
    {
        return Ok(None);
    }
    let answer = line.trim();
    if answer.is_empty() || answer.eq_ignore_ascii_case("none") {
        return Ok(None);
    }
    Ok(Some(answer.to_string()))
}

use std::path::{Path, PathBuf};
use std::time::Duration;

use serde_json::{Value, json};

use crate::atomic;
use crate::board::{
    ABSOLUTE_BOARD_PATH, AMBIGUOUS_BOARD, Board, Discovery, LEGACY_LAYOUT, ORPHAN_WORKTREE,
    RepoContext, STORAGE_MISMATCH, StorageMode,
};
use crate::cli::DoctorArgs;
use crate::error::{OrbitError, Result, code};
use crate::git;
use crate::graph::{Graph, Issue, LoadMode};

pub fn run(dir: Option<&Path>, args: DoctorArgs) -> Result<crate::output::CommandOutput> {
    let mut fixed = Vec::new();
    let mut migration = None;

    // A failed repair must still say where discovery looked, since `doctor` is
    // the command people run when resolution is already broken.
    let with_trace = |mut error: OrbitError| {
        error.message = format!(
            "{}\n{}",
            error.message,
            render_discovery(&Board::discovery(dir)).trim_end()
        );
        error
    };
    if args.fix {
        fixed = fix(dir).map_err(with_trace)?;
    }
    if args.migrate {
        migration = Some(crate::legacy::migrate(dir).map_err(with_trace)?);
    }

    diagnose(dir, &args, fixed, migration)
}

fn fix(dir: Option<&Path>) -> Result<Vec<String>> {
    let mut fixed = Vec::new();
    let discovery = Board::discovery(dir);
    if let Some(root) = &discovery.root
        && let Ok(board) = Board::at_root(root.clone(), Vec::new())
    {
        let _lock = board.lock_exclusive()?;
        let mut temps = atomic::clean_stale_temps(&board.tasks_dir(), Duration::ZERO)?;
        temps.extend(atomic::clean_stale_temps(
            &board.comments_dir(),
            Duration::ZERO,
        )?);
        fixed.extend(temps.into_iter().map(|path| path.display().to_string()));
    }

    if let Some(context) = repair_context(dir) {
        fixed.extend(reattach_orphans(&context)?);
    }
    Ok(fixed)
}

/// Repository whose `orbit` worktree should be repaired: the one holding
/// `--dir`/ORBIT_DIR when given (its nearest existing ancestor, since the
/// directory itself may be the missing worktree), else the current directory.
fn repair_context(dir: Option<&Path>) -> Option<RepoContext> {
    let Some(dir) = dir else {
        return Board::repo_context();
    };
    let dir = std::path::absolute(dir).unwrap_or_else(|_| dir.to_path_buf());
    let start = dir.ancestors().find(|path| path.exists())?;
    let start = dunce::canonicalize(start).unwrap_or_else(|_| start.to_path_buf());
    Board::repo_context_at(&start).ok().flatten()
}

/// Canonical form of a path that may not exist: the nearest existing ancestor
/// is resolved (`/var` vs `/private/var`) and the rest is appended.
fn canonical_lenient(path: &Path) -> PathBuf {
    let mut tail = Vec::new();
    let mut current = path;
    while !current.exists() {
        let (Some(parent), Some(name)) = (current.parent(), current.file_name()) else {
            return path.to_path_buf();
        };
        tail.push(name);
        current = parent;
    }
    let mut resolved = dunce::canonicalize(current).unwrap_or_else(|_| current.to_path_buf());
    resolved.extend(tail.into_iter().rev());
    resolved
}

/// Re-attaches an `orbit` worktree whose directory disappeared. The branch is
/// the source of truth, so this never rewrites a Task.
pub(crate) fn reattach_orphans(context: &RepoContext) -> Result<Vec<String>> {
    let records = git::worktree_records(&context.repo_root)?;
    let missing = records
        .into_iter()
        .filter(|record| record.branch.as_deref() == Some("orbit") && !record.path.exists())
        .collect::<Vec<_>>();
    if missing.is_empty() {
        return Ok(Vec::new());
    }

    git::worktree_prune(&context.repo_root)?;
    let mut fixed = Vec::new();
    for record in missing {
        if canonical_lenient(&record.path) == canonical_lenient(&context.legacy_local_board)
            && Board::is_board(&context.main_checkout.join(".orbit"))
        {
            continue;
        }
        let target = record.path.clone();
        if target.exists() {
            if Board::is_board(&target) {
                continue;
            }
            let empty = std::fs::read_dir(&target)
                .map(|mut entries| entries.next().is_none())
                .unwrap_or(false);
            if !empty {
                return Err(OrbitError::config(format!(
                    "cannot re-attach the orbit worktree at {} because the directory is not empty",
                    target.display()
                )));
            }
            std::fs::remove_dir(&target)
                .map_err(|error| OrbitError::io(format!("remove {}", target.display()), error))?;
        }
        if let Some(parent) = target.parent() {
            std::fs::create_dir_all(parent)
                .map_err(|error| OrbitError::io(format!("create {}", parent.display()), error))?;
        }
        git::worktree_add(&context.repo_root, &target, "orbit")?;
        if canonical_lenient(&target) != canonical_lenient(&context.legacy_local_board) {
            Board::ensure_excluded(&target)?;
        }
        fixed.push(format!("reattached {}", target.display()));
    }
    Ok(fixed)
}

fn diagnose(
    dir: Option<&Path>,
    args: &DoctorArgs,
    fixed: Vec<String>,
    migration: Option<crate::legacy::MigrationReport>,
) -> Result<crate::output::CommandOutput> {
    let discovery = Board::discovery(dir);
    let mut issues = Vec::new();
    let mut warnings = Vec::new();

    let board = match &discovery.root {
        Some(root) => match Board::at_root(root.clone(), discovery.warnings.clone()) {
            Ok(board) => Some(board),
            Err(error) => {
                issues.push(Issue::new(error.code, None, error.message));
                None
            }
        },
        None => None,
    };

    layout_issues(&discovery, board.as_ref(), &mut issues);

    let on_path = orbit_on_path();
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

    let task_count = match &board {
        Some(board) => {
            diagnose_tasks(board, &mut issues, &mut warnings, args.fix)?;
            board.task_ids().map(|ids| ids.len()).unwrap_or(0)
        }
        None => 0,
    };

    let healthy = issues.is_empty();
    let discovery_json = discovery_json(&discovery);
    let migration_json = migration
        .as_ref()
        .map(|report| serde_json::to_value(report).unwrap_or(Value::Null))
        .unwrap_or(Value::Null);

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
    human.push_str(&format!("tasks: {task_count}\n"));
    human.push_str(&render_discovery(&discovery));
    for warning in &warnings {
        human.push_str(&format!("warning: {warning}\n"));
    }
    for path in &fixed {
        human.push_str(&format!("fixed: {path}\n"));
    }
    if let Some(report) = &migration {
        human.push_str(&report.render_human());
    }

    Ok(crate::output::CommandOutput::new(
        json!({
            "healthy": healthy,
            "tasks": task_count,
            "issues": issues,
            "warnings": warnings,
            "path_on_path": on_path,
            "fixed": fixed,
            "discovery": discovery_json,
            "migration": migration_json,
        }),
        human,
    )
    .with_exit_code(if healthy { 0 } else { 1 }))
}

fn discovery_json(discovery: &Discovery) -> Value {
    json!({
        "root": discovery.root,
        "source": discovery.source.as_str(),
        "legacy": discovery.legacy.is_some(),
        "ambiguous": discovery.ambiguous.is_some(),
        "steps": discovery.steps,
    })
}

fn render_discovery(discovery: &Discovery) -> String {
    let mut human = String::from("discovery:\n");
    for step in &discovery.steps {
        let path = step
            .path
            .as_ref()
            .map(|path| format!(" {}", path.display()))
            .unwrap_or_default();
        let state = if step.found { "found" } else { "absent" };
        let selected = if step.selected { " (selected)" } else { "" };
        human.push_str(&format!(
            "- {}{path}: {state}{selected}\n",
            step.source.as_str()
        ));
        if let Some(note) = &step.note {
            human.push_str(&format!("  {note}\n"));
        }
    }
    if discovery.root.is_none() {
        human.push_str("no Board resolved; run `orbit init`\n");
    }
    human
}

fn layout_issues(discovery: &Discovery, board: Option<&Board>, issues: &mut Vec<Issue>) {
    if let Some(message) = &discovery.legacy {
        issues.push(Issue::new(LEGACY_LAYOUT, None, message.clone()));
    }
    if let Some((new, legacy)) = &discovery.ambiguous {
        issues.push(Issue::new(
            AMBIGUOUS_BOARD,
            None,
            format!(
                "both {} and legacy Board {} exist; run `orbit doctor --migrate` to finish or resume the move",
                new.display(),
                legacy.display()
            ),
        ));
    }
    if let Some(error) = &discovery.error
        && error.code != code::BOARD_NOT_FOUND
    {
        issues.push(Issue::new(error.code, None, error.message.clone()));
    }

    if let Some(board) = board {
        if board.config.board.storage.is_none() && discovery.legacy.is_none() {
            issues.push(Issue::new(
                LEGACY_LAYOUT,
                None,
                format!(
                    "Board at {} predates the Orbit folder and has no board.storage mode; run `orbit doctor --migrate`",
                    board.root.display()
                ),
            ));
        } else if let Some(configured) = board.config.board.storage {
            let actual = StorageMode::detect(&board.root);
            if configured != actual {
                let hint = if configured == StorageMode::Worktree {
                    "its .git link to the orbit worktree is missing or damaged; run `orbit doctor --fix`, or move the directory aside and re-run `orbit init`"
                } else {
                    "run `orbit doctor --migrate` or repair the worktree"
                };
                issues.push(Issue::new(
                    STORAGE_MISMATCH,
                    None,
                    format!(
                        "Board at {} records storage = {} but its Git shape is {}; {hint}",
                        board.root.display(),
                        configured.as_str(),
                        actual.as_str()
                    ),
                ));
            }
        }
    }

    if let Some(context) = Board::repo_context() {
        if let Ok(records) = git::worktree_records(&context.repo_root) {
            for record in records {
                if record.branch.as_deref() == Some("orbit") && !record.path.exists() {
                    issues.push(Issue::new(
                        ORPHAN_WORKTREE,
                        None,
                        format!(
                            "the orbit worktree at {} is missing; run `orbit doctor --fix` to re-attach it without task loss",
                            record.path.display()
                        ),
                    ));
                }
            }
        }

        let root = context.repo_root;
        let mut config_roots = vec![root];
        if let Some(home) = home_dir()
            && home != config_roots[0]
        {
            config_roots.push(home);
        }
        let mut seen = std::collections::BTreeSet::new();
        for path in config_roots
            .iter()
            .flat_map(|root| absolute_board_configs(root))
        {
            if seen.insert(path.clone()) {
                issues.push(Issue::new(
                    ABSOLUTE_BOARD_PATH,
                    None,
                    format!(
                        "{} records an absolute Board path; re-run `orbit setup` to rewrite it path-free",
                        path.display()
                    ),
                ));
            }
        }
    }
}

fn orbit_on_path() -> bool {
    let orbit_name = if cfg!(windows) { "orbit.exe" } else { "orbit" };
    let running = std::env::current_exe()
        .ok()
        .and_then(|path| dunce::canonicalize(path).ok());
    let found =
        crate::setup::find_on_path(orbit_name).and_then(|path| dunce::canonicalize(path).ok());
    match (&found, &running) {
        (Some(found), Some(running)) => found == running,
        (Some(_), None) => true,
        _ => false,
    }
}

fn diagnose_tasks(
    board: &Board,
    issues: &mut Vec<Issue>,
    warnings: &mut Vec<String>,
    fix: bool,
) -> Result<()> {
    let (graph, mut load_issues) = Graph::load(board, LoadMode::Lenient)?;
    issues.append(&mut load_issues);

    let prefix_mismatch = issues
        .iter()
        .any(|issue| issue.code == code::PREFIX_MISMATCH);

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
            task.holder().unwrap_or("unknown")
        ));
    }

    for task in graph.tasks.values() {
        let no_times = task
            .front
            .claim
            .as_ref()
            .is_some_and(|claim| !claim.has_times());
        if no_times || task.front.stray_claim_times.is_some() {
            warnings.push(format!("{}: claim metadata is incomplete", task.id));
        }

        // A prefix mismatch means the IDs use the observed prefix; comparing
        // them with the configured prefix would flag every SubTask.
        if !prefix_mismatch {
            let encoded_parent = crate::task::parent_id_from_id(&task.id, &board.config.id.prefix);
            match (&task.front.parent, encoded_parent) {
                (Some(parent), Some(encoded)) if parent != &encoded => {
                    issues.push(Issue::new(
                        code::PARENT_MISMATCH,
                        Some(&task.id),
                        format!(
                            "{} has parent {parent} but its ID encodes {encoded}",
                            task.id
                        ),
                    ));
                }
                (None, Some(encoded)) => {
                    issues.push(Issue::new(
                        code::PARENT_MISMATCH,
                        Some(&task.id),
                        format!(
                            "{} is a SubTask ID but has no parent; expected {encoded}",
                            task.id
                        ),
                    ));
                }
                (Some(parent), None) => {
                    issues.push(Issue::new(
                        code::PARENT_MISMATCH,
                        Some(&task.id),
                        format!(
                            "{} has parent {parent} but its ID is not a SubTask ID",
                            task.id
                        ),
                    ));
                }
                _ => {}
            }
        }

        if task.front.status == crate::task::Status::Done {
            let unfinished = graph.blocking_ids(task);
            if !unfinished.is_empty() {
                warnings.push(format!(
                    "{} is done but has unfinished dependencies: {}",
                    task.id,
                    unfinished.join(", ")
                ));
            }
            let open_subtasks = graph.open_children(&task.id);
            if !open_subtasks.is_empty() {
                warnings.push(format!(
                    "{} is done but has open SubTasks: {}",
                    task.id,
                    open_subtasks.join(", ")
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

    let mut stale_temps = atomic::stale_temps(&board.tasks_dir(), Duration::ZERO);
    stale_temps.extend(atomic::stale_temps(&board.comments_dir(), Duration::ZERO));
    if !stale_temps.is_empty() && !fix {
        warnings.push(format!(
            "{} stale temp file(s) in the board; run `orbit doctor --fix`",
            stale_temps.len()
        ));
    }

    Ok(())
}

/// Finds managed MCP entries that still carry `--dir <absolute path>` or an
/// absolute `ORBIT_DIR`. Generated configurations are path-free; a hit is an
/// old setup that needs to run again.
fn absolute_board_configs(root: &Path) -> Vec<PathBuf> {
    let mut files = Vec::new();
    for relative in [
        ".mcp.json",
        ".claude.json",
        ".cursor/mcp.json",
        "opencode.json",
        ".config/opencode/opencode.json",
    ] {
        let path = root.join(relative);
        let Ok(content) = std::fs::read_to_string(&path) else {
            continue;
        };
        let Ok(value) = serde_json::from_str::<Value>(&content) else {
            continue;
        };
        let containers: &[&[&str]] = &[&["mcpServers"], &["mcp", "servers"], &["mcp"]];
        if containers.iter().any(|container| {
            nested_value(&value, container)
                .and_then(|container| container.get("orbit"))
                .is_some_and(json_entry_records_absolute_board)
        }) {
            files.push(path);
        }
    }

    let path = root.join(".codex/config.toml");
    if let Ok(content) = std::fs::read_to_string(&path)
        && let Ok(value) = toml::from_str::<toml::Value>(&content)
        && value
            .get("mcp_servers")
            .and_then(|servers| servers.get("orbit"))
            .is_some_and(toml_entry_records_absolute_board)
    {
        files.push(path);
    }

    files
}

fn home_dir() -> Option<PathBuf> {
    std::env::var_os("HOME")
        .or_else(|| std::env::var_os("USERPROFILE"))
        .map(PathBuf::from)
        .and_then(|home| dunce::canonicalize(home).ok())
}

fn nested_value<'a>(value: &'a Value, path: &[&str]) -> Option<&'a Value> {
    let (head, rest) = path.split_first()?;
    let child = value.get(*head)?;
    if rest.is_empty() {
        Some(child)
    } else {
        nested_value(child, rest)
    }
}

fn json_entry_records_absolute_board(entry: &Value) -> bool {
    if is_orbit_entry(entry.get("command")) {
        let mut tokens = tokens_from_value(entry.get("command"));
        tokens.extend(tokens_from_value(entry.get("args")));
        if contains_absolute_board_path(&tokens) {
            return true;
        }
    }
    entry
        .get("env")
        .and_then(Value::as_object)
        .is_some_and(|env| {
            env.get("ORBIT_DIR")
                .and_then(Value::as_str)
                .is_some_and(|value| Path::new(value).is_absolute())
        })
}

fn toml_entry_records_absolute_board(entry: &toml::Value) -> bool {
    if !entry
        .get("command")
        .and_then(toml::Value::as_str)
        .is_some_and(is_orbit_command_text)
    {
        return false;
    }
    let mut tokens = entry
        .get("command")
        .and_then(toml::Value::as_str)
        .map(tokenize)
        .unwrap_or_default();
    tokens.extend(
        entry
            .get("args")
            .and_then(toml::Value::as_array)
            .into_iter()
            .flatten()
            .filter_map(toml::Value::as_str)
            .map(str::to_string),
    );
    contains_absolute_board_path(&tokens)
}

fn is_orbit_entry(command: Option<&Value>) -> bool {
    let Some(command) = command else {
        return false;
    };
    if let Some(command) = command.as_str() {
        return is_orbit_command_text(command);
    }
    command
        .as_array()
        .and_then(|command| command.first())
        .and_then(Value::as_str)
        .is_some_and(is_orbit_command_text)
}

fn is_orbit_command_text(command: &str) -> bool {
    Path::new(command)
        .file_name()
        .and_then(|name| name.to_str())
        .is_some_and(|name| name == "orbit" || name == "orbit.exe")
        || command == "orbit"
}

fn tokens_from_value(value: Option<&Value>) -> Vec<String> {
    match value {
        Some(Value::String(command)) => tokenize(command),
        Some(Value::Array(values)) => values
            .iter()
            .filter_map(Value::as_str)
            .map(str::to_string)
            .collect(),
        _ => Vec::new(),
    }
}

fn tokenize(command: &str) -> Vec<String> {
    command
        .split_whitespace()
        .map(|token| token.trim_matches('"').to_string())
        .collect()
}

fn contains_absolute_board_path(tokens: &[String]) -> bool {
    tokens
        .windows(2)
        .any(|pair| pair[0] == "--dir" && Path::new(pair[1].trim_matches('"')).is_absolute())
}

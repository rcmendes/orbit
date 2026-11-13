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

    diagnose(dir, &args, fixed)
}

fn fix(dir: Option<&Path>) -> Result<Vec<String>> {
    let mut fixed = Vec::new();
    let discovery = Board::discovery(dir);
    if let Some(root) = &discovery.root
        && let Ok(board) =
            Board::at_root_verdict(root.clone(), Vec::new(), discovery.needs_migration)
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
fn reattach_orphans(context: &RepoContext) -> Result<Vec<String>> {
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
) -> Result<crate::output::CommandOutput> {
    let discovery = Board::discovery(dir);
    let mut issues = Vec::new();
    let mut warnings = Vec::new();

    let board = match &discovery.root {
        Some(root) => match Board::at_root_verdict(
            root.clone(),
            discovery.warnings.clone(),
            discovery.needs_migration,
        ) {
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
                "`orbit` is not the binary on PATH; the SessionStart hook will not find it"
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
    if let Some(board) = &board {
        issues.extend(crate::remote::commands::doctor_issues(board));
    }

    let healthy = issues.is_empty();
    let discovery_json = discovery_json(&discovery);
    let mut human = String::new();
    if healthy {
        human.push_str("orbit doctor: healthy\n");
    } else {
        human.push_str(&format!("orbit doctor: {} issue(s)\n", issues.len()));
        for issue in &issues {
            human.push_str(&format!(
                "- [{}] [{}] {}{}\n",
                issue.severity.to_ascii_uppercase(),
                issue.code,
                issue
                    .task
                    .as_ref()
                    .map(|task| format!("{task}: "))
                    .unwrap_or_default(),
                issue.message
            ));
            if let Some(repository) = &issue.repository {
                human.push_str(&format!("  repository: {repository}\n"));
            }
            if let Some(identity) = &issue.issue {
                human.push_str(&format!("  issue: {identity}\n"));
            }
            if let Some(cause) = &issue.cause {
                human.push_str(&format!("  cause: {cause}\n"));
            }
            if let Some(next_action) = &issue.next_action {
                human.push_str(&format!("  next: {next_action}\n"));
            }
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

    Ok(crate::output::CommandOutput::new(
        json!({
            "healthy": healthy,
            "tasks": task_count,
            "issues": issues,
            "warnings": warnings,
            "path_on_path": on_path,
            "fixed": fixed,
            "discovery": discovery_json,
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
    // One LEGACY_LAYOUT issue from the authoritative verdict; the discovery
    // finding, when present, explains which legacy shape produced it.
    if discovery.needs_migration {
        let message = match &discovery.legacy {
            Some(detail) => {
                format!("{detail}; run `orbit migrate` to convert it to a standalone Board folder")
            }
            None => format!(
                "Board at {} uses a pre-ADR-0019 layout; run `orbit migrate` to convert it to a standalone Board folder",
                discovery
                    .root
                    .as_ref()
                    .map(|root| root.display().to_string())
                    .unwrap_or_default()
            ),
        };
        issues.push(Issue::new(LEGACY_LAYOUT, None, message));
    }
    if let Some((new, legacy)) = &discovery.ambiguous {
        issues.push(Issue::new(
            AMBIGUOUS_BOARD,
            None,
            format!(
                "both {} and legacy Board {} exist; resolve the duplicate Boards, then run `orbit migrate` to convert the legacy one",
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

    if let Some(board) = board
        && let Some(configured) = board.config.board.storage
    {
        let actual = StorageMode::detect(&board.root);
        if configured != actual {
            let hint = if configured == StorageMode::Worktree {
                "its .git link to the orbit worktree is missing or damaged; run `orbit doctor --fix`, or run `orbit migrate` to convert the Board"
            } else {
                "run `orbit migrate` to convert the Board"
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

    if let Some(context) = Board::repo_context() {
        if let Ok(records) = git::worktree_records(&context.repo_root) {
            for record in records {
                if record.branch.as_deref() == Some("orbit") && !record.path.exists() {
                    issues.push(Issue::new(
                        ORPHAN_WORKTREE,
                        None,
                        format!(
                            "the orbit worktree at {} is missing; run `orbit doctor --fix` to restore the legacy Board, then `orbit migrate` to convert it to a standalone Board folder",
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

    if let Err(error) = crate::health::comment_files(board, &graph, false, issues) {
        issues.push(Issue::new(code::BOARD_CORRUPT, None, error.message));
    }

    let mut keys: std::collections::BTreeMap<&str, Vec<&str>> = std::collections::BTreeMap::new();
    for task in graph.tasks.values() {
        if let Some(key) = task.front.source_key.as_deref() {
            keys.entry(key).or_default().push(task.id.as_str());
        }
    }
    for (key, ids) in keys.into_iter().filter(|(_, ids)| ids.len() > 1) {
        issues.push(Issue::new(
            code::DUPLICATE_SOURCE_KEY,
            ids.first().copied(),
            format!(
                "source key {key} is on more than one task: {}",
                ids.join(", ")
            ),
        ));
    }

    for task in graph.tasks.values() {
        for (owner, value) in &task.front.annotations {
            if let Err(error) = crate::task::validate_annotation(owner, value) {
                issues.push(Issue::new(error.code, Some(&task.id), error.message));
            }
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

    // A prefix mismatch means the IDs use the observed prefix; comparing them
    // with the configured prefix would flag every SubTask.
    if !prefix_mismatch {
        crate::health::parent_id_agreement(board, &graph, issues);
    }

    match crate::git::status_porcelain(&board.root) {
        Ok(status) if !status.trim().is_empty() => {
            warnings.push("the board has uncommitted changes; run `orbit sync`".to_string());
        }
        Ok(_) => {}
        Err(error) => issues.push(Issue::new(code::BOARD_CORRUPT, None, error.message)),
    }

    crate::health::branch_mismatch(board, issues);

    let mut stale_temps = atomic::stale_temps(&board.tasks_dir(), Duration::ZERO);
    stale_temps.extend(atomic::stale_temps(&board.comments_dir(), Duration::ZERO));
    if !stale_temps.is_empty() && !fix {
        warnings.push(format!(
            "{} stale temp file(s) in the board; run `orbit doctor --fix`",
            stale_temps.len()
        ));
    }

    // Links on a Board with no Remote Tracker configuration are inert
    // provenance: they stay untouched, nothing calls the tracker, and doctor
    // says so once instead of per Task.
    let linked = graph
        .tasks
        .values()
        .filter(|task| crate::remote::link::link_view(task).is_some())
        .count();
    if linked > 0 && !crate::remote::configured(board) {
        warnings.push(format!(
            "{linked} task(s) carry Remote Issue links but no Remote Tracker is configured; the links are kept untouched and nothing syncs — configure a Remote Project to resume syncing"
        ));
    }

    Ok(())
}

/// Finds managed MCP entries that still carry `--dir <absolute path>` or an
/// absolute `ORBIT_DIR`. Generated configurations are path-free; a hit is an
/// old setup that needs to run again.
pub(crate) fn absolute_board_configs(root: &Path) -> Vec<PathBuf> {
    let mut files = Vec::new();
    // Candidates come from setup's client layouts: adding a client there
    // automatically extends this check.
    for config in crate::setup::mcp_config_files() {
        match config {
            crate::setup::McpConfigFile::Json {
                relative,
                containers,
            } => {
                let path = root.join(relative);
                let Ok(content) = std::fs::read_to_string(&path) else {
                    continue;
                };
                let Ok(value) = serde_json::from_str::<Value>(&content) else {
                    continue;
                };
                if containers.iter().any(|container| {
                    nested_value(&value, container)
                        .and_then(|container| container.get("orbit"))
                        .is_some_and(json_entry_records_absolute_board)
                }) {
                    files.push(path);
                }
            }
            crate::setup::McpConfigFile::Toml { relative } => {
                let path = root.join(relative);
                if let Ok(content) = std::fs::read_to_string(&path)
                    && let Ok(value) = toml::from_str::<toml::Value>(&content)
                    && value
                        .get("mcp_servers")
                        .and_then(|servers| servers.get("orbit"))
                        .is_some_and(toml_entry_records_absolute_board)
                {
                    files.push(path);
                }
            }
        }
    }

    files
}

fn home_dir() -> Option<PathBuf> {
    crate::git::home_dir().and_then(|home| dunce::canonicalize(home).ok())
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
        .is_some_and(crate::setup::is_orbit_command)
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
        return crate::setup::is_orbit_command(command);
    }
    command
        .as_array()
        .and_then(|command| command.first())
        .and_then(Value::as_str)
        .is_some_and(crate::setup::is_orbit_command)
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

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(command: &str) -> Value {
        let board = std::env::temp_dir().join("board");
        serde_json::json!({
            "command": command,
            "args": ["mcp", "--dir", board.to_string_lossy()],
        })
    }

    #[test]
    fn absolute_board_check_recognizes_orbit_commands() {
        assert!(json_entry_records_absolute_board(&entry("orbit")));
        assert!(json_entry_records_absolute_board(&entry("orbit.exe")));
        assert!(!json_entry_records_absolute_board(&entry("not-orbit")));
    }

    /// Doctor shares setup's matcher, so Windows command names match case-insensitively.
    #[cfg(windows)]
    #[test]
    fn absolute_board_check_ignores_case_on_windows() {
        assert!(json_entry_records_absolute_board(&entry(
            r"C:\Tools\Orbit.EXE"
        )));
    }
}

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::atomic;
use crate::board::{Board, Config};
use crate::cli::WorkspaceMigrateArgs;
use crate::error::{OrbitError, Result};
use crate::graph::{Graph, LoadMode};
use crate::task::{self, Task};

const STATE_FILE: &str = "workspace-migration-state.json";
const STAGE_DIR: &str = ".workspace-board-migration-stage";

#[derive(Debug, Serialize, Deserialize)]
struct MigrationPlan {
    workspace: PathBuf,
    destination: PathBuf,
    initialize_destination: bool,
    destination_config_before: Vec<u8>,
    destination_config_after: Option<Vec<u8>>,
    #[serde(default)]
    configuration_source: Option<PathBuf>,
    accepted_prefixes_added: Vec<String>,
    sources: Vec<PathBuf>,
    files: Vec<PlannedFile>,
    task_count: usize,
    mappings: Vec<IdMapping>,
    configuration_conflicts: Vec<ConfigConflict>,
    commit: Option<String>,
}

#[derive(Debug, Serialize, Deserialize)]
struct PlannedFile {
    path: PathBuf,
    content: Vec<u8>,
}

#[derive(Debug, Serialize, Deserialize)]
struct IdMapping {
    source: PathBuf,
    from: String,
    to: String,
    reason: String,
}

#[derive(Debug, Serialize, Deserialize)]
struct ConfigConflict {
    source: PathBuf,
    setting: String,
    workspace_value: Value,
    source_value: Value,
}

pub fn run(dir: Option<&Path>, args: WorkspaceMigrateArgs) -> Result<crate::output::CommandOutput> {
    let requested = args.workspace.or_else(|| dir.map(Path::to_path_buf));
    let workspace = match requested {
        Some(path) => crate::board::resolve_external(&path)?,
        None => std::env::current_dir()
            .map_err(|error| OrbitError::io("resolve current directory", error))
            .and_then(|path| crate::board::resolve_external(&path))?,
    };
    if crate::git::toplevel_containing(&workspace).is_some() {
        return Err(OrbitError::config(format!(
            "{} is inside a Git working tree; Workspace migration requires an ordinary non-Git Workspace folder",
            workspace.display()
        )));
    }

    let state_path = workspace.join(".orbit").join(STATE_FILE);
    let stage_path = workspace.join(".orbit").join(STAGE_DIR);
    let mut plan = if args.resume {
        let content = std::fs::read(&state_path).map_err(|error| {
            if error.kind() == std::io::ErrorKind::NotFound {
                OrbitError::config(format!(
                    "no interrupted migration state exists at {}; run without --resume to plan a migration",
                    state_path.display()
                ))
            } else {
                OrbitError::io(format!("read {}", state_path.display()), error)
            }
        })?;
        let plan: MigrationPlan = serde_json::from_slice(&content).map_err(|error| {
            OrbitError::config(format!(
                "invalid migration state {}: {error}",
                state_path.display()
            ))
        })?;
        if plan.workspace != workspace {
            return Err(OrbitError::config(format!(
                "saved migration belongs to {}, not {}; resume it from its original Workspace",
                plan.workspace.display(),
                workspace.display()
            )));
        }
        if plan.destination != workspace.join(".orbit/board") {
            return Err(OrbitError::config(
                "saved migration does not target this Workspace's .orbit/board",
            ));
        }
        let destination_is_board = Board::is_board(&plan.destination);
        if (!plan.initialize_destination && !destination_is_board)
            || (plan.initialize_destination && !destination_is_board && plan.destination.exists())
            || (plan.initialize_destination
                && !destination_is_board
                && stage_path.exists()
                && !Board::is_board(&stage_path))
            || (destination_is_board && stage_path.exists())
        {
            return Err(OrbitError::config(
                "saved Workspace migration has no valid Board destination or staging Board",
            ));
        }
        for file in &plan.files {
            let mut components = file.path.components();
            let valid_root = matches!(components.next(), Some(std::path::Component::Normal(root)) if root == "tasks" || root == "comments");
            let valid_file = matches!(components.next(), Some(std::path::Component::Normal(name)) if name.to_string_lossy().ends_with(".md"));
            if !valid_root || !valid_file || components.next().is_some() {
                return Err(OrbitError::config(
                    "saved migration contains an unsafe destination path",
                ));
            }
        }
        plan
    } else {
        if state_path.exists() {
            return Err(OrbitError::config(format!(
                "an interrupted Workspace migration exists at {}; use `orbit workspace migrate --resume` or remove it only after reviewing the partial migration",
                state_path.display()
            )));
        }
        if stage_path.exists() {
            return Err(OrbitError::config(format!(
                "migration staging path {} exists without a saved plan; inspect it before continuing",
                stage_path.display()
            )));
        }
        let plan = make_plan(&workspace)?;
        if args.dry_run {
            return Ok(render_plan(&plan, false, false, None));
        }
        plan
    };

    if args.dry_run {
        return Ok(render_plan(&plan, false, false, None));
    }
    apply_plan(&mut plan, &state_path, args.resume)
}

fn make_plan(workspace: &Path) -> Result<MigrationPlan> {
    let destination = workspace.join(".orbit/board");
    let initialize_destination = !Board::is_board(&destination);
    if initialize_destination && destination.exists() {
        return Err(OrbitError::config(format!(
            "{} exists but is not an Orbit Board; move it before migrating",
            destination.display()
        )));
    }
    let (destination_config, destination_graph, destination_config_before, configuration_source) =
        if initialize_destination {
            let (slug, _) = crate::slug::auto_slug_for_dir(workspace);
            let source = workspace_config_source(workspace, &destination);
            let mut config = match &source {
                Some(root) => Config::load(root)?,
                None => Config::default(),
            };
            config.id.prefix = crate::slug::prefix(&slug);
            config.board.storage = Some(crate::board::StorageMode::Repository);
            config.board.branch = "orbit".to_string();
            config.validate()?;
            let mut initialized = Config::default();
            initialized.id.prefix = config.id.prefix.clone();
            initialized.board.storage = Some(crate::board::StorageMode::Repository);
            let config_bytes = initialized.to_toml()?.into_bytes();
            (config, Graph::from_tasks(Vec::new()), config_bytes, source)
        } else {
            let board = Board::at_root(destination.clone(), Vec::new())?;
            let (graph, _) = Graph::load(&board, LoadMode::Strict)?;
            let config_path = destination.join("config.toml");
            let config_bytes = std::fs::read(&config_path).map_err(|error| {
                OrbitError::io(format!("read {}", config_path.display()), error)
            })?;
            (board.config, graph, config_bytes, None)
        };
    let sources = find_sources(workspace, &destination)?;
    if sources.is_empty() {
        return Err(OrbitError::config(format!(
            "no legacy Workspace or child-repository Boards were found under {}",
            workspace.display()
        )));
    }

    let mut source_boards = Vec::new();
    for source in &sources {
        let board = Board::at_root(source.clone(), Vec::new())?;
        let (graph, _) = Graph::load(&board, LoadMode::Strict)?;
        for id in board.comment_ids()? {
            board.read_comments(&id)?;
        }
        source_boards.push((board, graph));
    }

    let mut merged_config = destination_config.clone();
    let mut accepted_prefixes_added = Vec::new();
    for (board, graph) in &source_boards {
        for task in graph.iter() {
            if !merged_config.id.is_valid_task_id(&task.id) {
                let prefix = board
                    .config
                    .id
                    .prefix_for_id(&task.id)
                    .ok_or_else(|| {
                        OrbitError::corrupt_task(format!(
                            "source Task {} uses no configured Task-ID prefix",
                            task.id
                        ))
                    })?
                    .to_string();
                if prefix != merged_config.id.prefix
                    && !merged_config.id.accepted_prefixes.contains(&prefix)
                {
                    merged_config.id.accepted_prefixes.push(prefix.clone());
                    accepted_prefixes_added.push(prefix);
                }
            }
        }
    }
    merged_config.validate()?;

    ensure_unique_github_links(&destination, &destination_graph, &source_boards)?;
    let mut occupied: BTreeSet<String> = destination_graph.tasks.keys().cloned().collect();
    let mut merged_tasks: Vec<Task> = destination_graph.iter().cloned().collect();
    let mut plan_files = Vec::new();
    let mut mappings = Vec::new();
    let mut task_count = 0;

    for (board, graph) in &source_boards {
        let mut local_map = HashMap::new();
        let mut ordered: Vec<&Task> = graph.iter().collect();
        ordered.sort_by(|left, right| {
            left.front
                .parent
                .is_some()
                .cmp(&right.front.parent.is_some())
                .then_with(|| left.id.cmp(&right.id))
        });

        for task in &ordered {
            let parent_mapped = task
                .front
                .parent
                .as_ref()
                .and_then(|parent| local_map.get(parent));
            let prefix_matches = merged_config.id.is_valid_task_id(&task.id);
            let desired = if let Some(parent) = parent_mapped {
                task.id
                    .rsplit_once('.')
                    .map(|(_, suffix)| format!("{parent}.{suffix}"))
                    .unwrap_or_else(|| task.id.clone())
            } else {
                task.id.clone()
            };
            let (target, reason) = if prefix_matches && !occupied.contains(&desired) {
                (desired, None)
            } else {
                let reason = if !prefix_matches {
                    "destination Board uses a different Task-ID prefix"
                } else {
                    "Task-ID collision"
                };
                (
                    replacement_id(
                        &board.root,
                        &task.id,
                        task.front
                            .parent
                            .as_deref()
                            .and_then(|parent| local_map.get(parent)),
                        &merged_config,
                        &occupied,
                    ),
                    Some(reason),
                )
            };
            occupied.insert(target.clone());
            local_map.insert(task.id.clone(), target.clone());
            if target != task.id {
                mappings.push(IdMapping {
                    source: board.root.clone(),
                    from: task.id.clone(),
                    to: target,
                    reason: reason
                        .unwrap_or("structured parent ID remapping")
                        .to_string(),
                });
            }
        }

        for task in ordered {
            let mut migrated = task.clone();
            migrated.id = local_map[&task.id].clone();
            migrated.front.parent = task.front.parent.as_ref().map(|parent| {
                local_map
                    .get(parent)
                    .cloned()
                    .unwrap_or_else(|| parent.clone())
            });
            migrated.front.depends_on = task
                .front
                .depends_on
                .iter()
                .map(|dependency| {
                    local_map
                        .get(dependency)
                        .cloned()
                        .unwrap_or_else(|| dependency.clone())
                })
                .collect();
            let changed = migrated.id != task.id
                || migrated.front.parent != task.front.parent
                || migrated.front.depends_on != task.front.depends_on;
            let content = if changed {
                task::render_task(&migrated)?.into_bytes()
            } else {
                std::fs::read(board.task_path(&task.id)).map_err(|error| {
                    OrbitError::io(
                        format!("read {}", board.task_path(&task.id).display()),
                        error,
                    )
                })?
            };
            plan_files.push(PlannedFile {
                path: PathBuf::from("tasks").join(format!("{}.md", migrated.id)),
                content,
            });
            merged_tasks.push(migrated);
            task_count += 1;
        }

        for id in board.comment_ids()? {
            let target_id = local_map.get(&id).cloned().unwrap_or_else(|| id.clone());
            let source_path = board.comment_path(&id);
            let bytes = std::fs::read(&source_path).map_err(|error| {
                OrbitError::io(format!("read {}", source_path.display()), error)
            })?;
            board.read_comments(&id)?;
            let content = std::str::from_utf8(&bytes).map_err(|error| {
                OrbitError::new(
                    crate::error::code::CORRUPT_COMMENT,
                    format!("{} is not UTF-8: {error}", source_path.display()),
                )
            })?;
            if !crate::comment::has_header(content, &id) {
                return Err(OrbitError::new(
                    crate::error::code::CORRUPT_COMMENT,
                    format!("{} has a mismatched comment header", source_path.display()),
                ));
            }
            let content = if target_id == id {
                bytes
            } else {
                crate::comment::rewrite_header(content, &id, &target_id)?.into_bytes()
            };
            plan_files.push(PlannedFile {
                path: PathBuf::from("comments").join(format!("{target_id}.md")),
                content,
            });
        }
    }

    let merged = Graph::from_tasks_with_holds(
        merged_tasks,
        destination_config.board.non_ready_labels.clone(),
    );
    let validation = merged.validate();
    if !validation.is_empty() {
        return Err(OrbitError::board_corrupt(
            "the merged Workspace Board would have integrity issues; resolve the source Boards before migrating",
            json!({ "issues": validation }),
        ));
    }

    let mut configuration_conflicts = Vec::new();
    let workspace_values = config_values(&destination_config)?;
    for (board, _) in &source_boards {
        let source_config = Config::load(&board.root)?;
        for (setting, source_value) in config_values(&source_config)? {
            if workspace_values
                .get(&setting)
                .is_some_and(|value| value != &source_value)
            {
                configuration_conflicts.push(ConfigConflict {
                    source: board.root.clone(),
                    setting: setting.clone(),
                    workspace_value: workspace_values[&setting].clone(),
                    source_value,
                });
            }
        }
    }

    plan_files.sort_by(|left, right| left.path.cmp(&right.path));
    for pair in plan_files.windows(2) {
        if pair[0].path == pair[1].path && pair[0].content != pair[1].content {
            return Err(OrbitError::config(format!(
                "two source Boards contain different data for {}; resolve this collision before migration",
                pair[0].path.display()
            )));
        }
    }
    plan_files.dedup_by(|left, right| left.path == right.path && left.content == right.content);
    for file in &plan_files {
        let destination_file = destination.join(&file.path);
        if destination_file.exists()
            && std::fs::read(&destination_file).map_err(|error| {
                OrbitError::io(format!("read {}", destination_file.display()), error)
            })? != file.content
        {
            return Err(OrbitError::config(format!(
                "destination {} already contains different data; resolve it before migration",
                destination_file.display()
            )));
        }
    }

    let destination_config_after = if initialize_destination {
        let bytes = merged_config.to_toml()?.into_bytes();
        (bytes != destination_config_before).then_some(bytes)
    } else if accepted_prefixes_added.is_empty() {
        None
    } else {
        Some(merged_config.to_toml()?.into_bytes())
    };

    Ok(MigrationPlan {
        workspace: workspace.to_path_buf(),
        destination,
        initialize_destination,
        destination_config_before,
        destination_config_after,
        configuration_source,
        accepted_prefixes_added,
        sources,
        files: plan_files,
        task_count,
        mappings,
        configuration_conflicts,
        commit: None,
    })
}

fn workspace_config_source(workspace: &Path, destination: &Path) -> Option<PathBuf> {
    [
        workspace.join(".orbit"),
        workspace.join("board"),
        workspace.join(".board"),
    ]
    .into_iter()
    .find(|candidate| candidate != destination && Board::is_board(candidate))
}

fn find_sources(workspace: &Path, destination: &Path) -> Result<Vec<PathBuf>> {
    let mut found = BTreeSet::new();
    for legacy in [
        workspace.join(".board"),
        workspace.join("board"),
        workspace.join(".orbit"),
    ] {
        if legacy != destination && Board::is_board(&legacy) {
            found.insert(dunce::canonicalize(&legacy).unwrap_or(legacy));
        }
    }
    let mut pending = vec![workspace.to_path_buf()];
    while let Some(directory) = pending.pop() {
        let entries = std::fs::read_dir(&directory)
            .map_err(|error| OrbitError::io(format!("read {}", directory.display()), error))?;
        for entry in entries {
            let entry = entry
                .map_err(|error| OrbitError::io(format!("read {}", directory.display()), error))?;
            if !entry.file_type().map(|kind| kind.is_dir()).unwrap_or(false) {
                continue;
            }
            let path = entry.path();
            let name = entry.file_name();
            if name == ".git" || name == ".orbit" || name == ".board" {
                continue;
            }
            if path.join(".git").exists() {
                let local = path.join(".orbit");
                if Board::is_board(&local) && local != destination {
                    found.insert(dunce::canonicalize(&local).unwrap_or(local.clone()));
                }
                if let Ok(common) = crate::git::common_dir_at(&path) {
                    let legacy = common.join("orbit/board");
                    if Board::is_board(&legacy) && legacy != destination && legacy != local {
                        found.insert(dunce::canonicalize(&legacy).unwrap_or(legacy));
                    }
                }
                continue;
            }
            pending.push(path);
        }
    }
    Ok(found.into_iter().collect())
}

fn ensure_unique_github_links(
    destination_root: &Path,
    destination: &Graph,
    sources: &[(Board, Graph)],
) -> Result<()> {
    let mut seen: BTreeMap<(String, u64), (PathBuf, String)> = BTreeMap::new();
    for (board, graph) in std::iter::once((destination_root.to_path_buf(), destination)).chain(
        sources
            .iter()
            .map(|(board, graph)| (board.root.clone(), graph)),
    ) {
        for task in graph.iter() {
            let Some(link) = task.github_link() else {
                continue;
            };
            let key = (link.repo.to_ascii_lowercase(), link.number);
            if let Some((previous_source, previous_task)) = seen.get(&key) {
                return Err(OrbitError::config(format!(
                    "duplicate GitHub Issue identity {}#{} is linked by Tasks {} and {}; choose which Task owns it, then retry",
                    link.repo,
                    link.number,
                    previous_task,
                    task.id
                ))
                .with_details(json!({
                    "repository": link.repo,
                    "number": link.number,
                    "first_source": previous_source,
                    "first_task": previous_task,
                    "duplicate_source": board,
                    "duplicate_task": task.id,
                })));
            }
            seen.insert(key, (board.clone(), task.id.clone()));
        }
    }
    Ok(())
}

fn ensure_planned_github_links_unique(destination: &Graph, files: &[PlannedFile]) -> Result<()> {
    let mut seen: BTreeMap<(String, u64), String> = BTreeMap::new();
    for task in destination.iter() {
        if let Some(link) = task.github_link() {
            seen.insert(
                (link.repo.to_ascii_lowercase(), link.number),
                task.id.clone(),
            );
        }
    }
    for file in files.iter().filter(|file| file.path.starts_with("tasks")) {
        let Some(id) = file.path.file_stem().and_then(std::ffi::OsStr::to_str) else {
            continue;
        };
        if destination.tasks.contains_key(id) {
            continue;
        }
        let content = std::str::from_utf8(&file.content).map_err(|error| {
            OrbitError::corrupt_task(format!("planned Task {id} is not UTF-8: {error}"))
        })?;
        let task = task::parse_task(id.to_string(), content)?;
        if let Some(link) = task.github_link() {
            let key = (link.repo.to_ascii_lowercase(), link.number);
            if let Some(previous) = seen.get(&key) {
                return Err(OrbitError::config(format!(
                    "duplicate GitHub Issue identity {}#{} appeared after planning (Tasks {previous} and {id}); resolve it and build a new plan",
                    link.repo, link.number
                )));
            }
            seen.insert(key, id.to_string());
        }
    }
    Ok(())
}

fn replacement_id(
    source: &Path,
    original: &str,
    parent: Option<&String>,
    config: &Config,
    occupied: &BTreeSet<String>,
) -> String {
    let base = parent.cloned().or_else(|| config.id.parent_id(original));
    for attempt in 0..u64::MAX {
        let suffix = stable_suffix(source, original, attempt, config.id.length);
        let candidate = match &base {
            Some(parent_id) => format!("{parent_id}.{suffix}"),
            None => format!("{}{suffix}", config.id.prefix),
        };
        if !occupied.contains(&candidate) {
            return candidate;
        }
    }
    unreachable!("a unique generated task id is available")
}

fn stable_suffix(source: &Path, original: &str, attempt: u64, len: usize) -> String {
    let mut hash = 0xcbf29ce484222325u64;
    for byte in source
        .as_os_str()
        .to_string_lossy()
        .bytes()
        .chain([0])
        .chain(original.bytes())
        .chain(attempt.to_le_bytes())
    {
        hash ^= u64::from(byte);
        hash = hash.wrapping_mul(0x100000001b3);
    }
    let alphabet = task::ID_ALPHABET;
    (0..len)
        .map(|index| alphabet[((hash >> ((index % 12) * 5)) as usize) % alphabet.len()] as char)
        .collect()
}

fn config_values(config: &Config) -> Result<BTreeMap<String, Value>> {
    let value = toml::Value::try_from(config)
        .map_err(|error| OrbitError::config(format!("serialize Board config: {error}")))?;
    let mut values = BTreeMap::new();
    flatten_toml("", &value, &mut values);
    Ok(values)
}

fn flatten_toml(prefix: &str, value: &toml::Value, output: &mut BTreeMap<String, Value>) {
    if let toml::Value::Table(table) = value {
        for (key, child) in table {
            let path = if prefix.is_empty() {
                key.clone()
            } else {
                format!("{prefix}.{key}")
            };
            flatten_toml(&path, child, output);
        }
    } else if let Ok(value) = serde_json::to_value(value) {
        output.insert(prefix.to_string(), value);
    }
}

fn apply_plan(
    plan: &mut MigrationPlan,
    state_path: &Path,
    resumed: bool,
) -> Result<crate::output::CommandOutput> {
    let _workspace_lock = crate::lock::BoardLock::acquire(
        &plan.workspace.join(".orbit/workspace-migration.lock"),
        2000,
        true,
    )?;
    let stage_path = plan.workspace.join(".orbit").join(STAGE_DIR);
    let initializing = plan.initialize_destination && !Board::is_board(&plan.destination);
    if initializing {
        let serialized = serde_json::to_vec_pretty(plan)
            .map_err(|error| OrbitError::config(format!("serialize migration plan: {error}")))?;
        atomic::write_atomic(state_path, &serialized, true)?;
        if stage_path.exists() {
            if !Board::is_board(&stage_path) {
                return Err(OrbitError::config(format!(
                    "migration staging path {} exists but is not an Orbit Board",
                    stage_path.display()
                )));
            }
        } else {
            let (slug, _) = crate::slug::auto_slug_for_dir(&plan.workspace);
            Board::init_repository(&stage_path, Some(&slug))?;
        }
    }
    let board_root = if initializing {
        stage_path
    } else {
        plan.destination.clone()
    };
    let board = Board::at_root(board_root, Vec::new())?;
    let _lock = board.lock_exclusive()?;
    validate_destination_changes(&board, plan, resumed)?;
    let (current_graph, _) = Graph::load(&board, LoadMode::Strict)?;
    ensure_planned_github_links_unique(&current_graph, &plan.files)?;
    let config_path = board.root.join("config.toml");
    let current_config = std::fs::read(&config_path)
        .map_err(|error| OrbitError::io(format!("read {}", config_path.display()), error))?;
    if current_config != plan.destination_config_before
        && !(resumed
            && plan
                .destination_config_after
                .as_ref()
                .is_some_and(|expected| expected == &current_config))
    {
        return Err(OrbitError::config(format!(
            "Workspace config {} changed after the migration plan was built; preserve the source backups and build a new plan",
            config_path.display()
        )));
    }
    for file in &plan.files {
        let path = board.root.join(&file.path);
        if path.exists()
            && std::fs::read(&path)
                .map_err(|error| OrbitError::io(format!("read {}", path.display()), error))?
                != file.content
        {
            return Err(OrbitError::config(format!(
                "migration paused because {} changed after the plan was built; preserve the source backups and resolve the destination edit before resuming",
                path.display()
            )));
        }
    }
    if !initializing {
        let serialized = serde_json::to_vec_pretty(plan)
            .map_err(|error| OrbitError::config(format!("serialize migration plan: {error}")))?;
        atomic::write_atomic(state_path, &serialized, true)?;
    }
    if let Some(config_after) = &plan.destination_config_after
        && current_config != *config_after
    {
        atomic::write_atomic(&config_path, config_after, true)?;
    }
    for file in &plan.files {
        let path = board.root.join(&file.path);
        if path.exists() {
            let existing = std::fs::read(&path)
                .map_err(|error| OrbitError::io(format!("read {}", path.display()), error))?;
            if existing != file.content {
                return Err(OrbitError::config(format!(
                    "migration paused because {} changed after the plan was built; preserve the source backups and resolve the destination edit before resuming",
                    path.display()
                )));
            }
            continue;
        }
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)
                .map_err(|error| OrbitError::io(format!("create {}", parent.display()), error))?;
        }
        atomic::write_atomic(&path, &file.content, false)?;
    }

    let validated_board = Board::at_root(board.root.clone(), Vec::new())?;
    let (graph, _) = Graph::load(&validated_board, LoadMode::Strict)?;
    let outcome = validated_board.commit("orbit: migrate Workspace Boards", true)?;
    if outcome == crate::git::CommitOutcome::Committed
        || (plan.commit.is_none()
            && !plan.files.is_empty()
            && crate::git::head_subject(&validated_board.root)?
                == "orbit: migrate Workspace Boards")
    {
        plan.commit = Some(crate::git::head_commit(&validated_board.root)?);
    }
    let serialized = serde_json::to_vec_pretty(plan)
        .map_err(|error| OrbitError::config(format!("serialize migration plan: {error}")))?;
    atomic::write_atomic(state_path, &serialized, true)?;
    drop(_lock);
    if initializing {
        if Board::is_board(&plan.destination) {
            return Err(OrbitError::config(format!(
                "Workspace Board {} appeared during migration; keep the staging Board at {} and inspect both before resuming",
                plan.destination.display(),
                validated_board.root.display()
            )));
        }
        std::fs::rename(&validated_board.root, &plan.destination).map_err(|error| {
            OrbitError::io(
                format!(
                    "activate migrated Workspace Board {} at {}",
                    validated_board.root.display(),
                    plan.destination.display()
                ),
                error,
            )
        })?;
    }
    let commit = plan.commit.clone();
    std::fs::remove_file(state_path)
        .map_err(|error| OrbitError::io(format!("remove {}", state_path.display()), error))?;

    let mut output = render_plan(plan, true, resumed, commit.clone());
    if let Some(commit) = &commit {
        output.human.push_str(&format!(
            "Board commit: {commit}\nRollback: git -C {}/.orbit/board revert {commit}\n",
            plan.workspace.display()
        ));
    }
    if let Some(data) = output.data.as_object_mut() {
        data.insert("validated_tasks".to_string(), json!(graph.tasks.len()));
        data.insert("commit_created".to_string(), json!(plan.commit.is_some()));
        data.insert(
            "doctor_check".to_string(),
            json!(format!(
                "run `orbit --dir {} doctor`",
                plan.workspace.display()
            )),
        );
        if let Some(commit) = &commit {
            data.insert(
                "rollback_guidance".to_string(),
                json!(format!(
                    "git -C {}/.orbit/board revert {commit}",
                    plan.workspace.display()
                )),
            );
        }
    }
    if !plan.configuration_conflicts.is_empty() {
        output.warnings.push(format!(
            "{} source Board setting(s) differed; the Workspace Board configuration was kept",
            plan.configuration_conflicts.len()
        ));
    }
    Ok(output)
}

fn validate_destination_changes(board: &Board, plan: &MigrationPlan, resumed: bool) -> Result<()> {
    let status = crate::git::status_porcelain(&board.root)?;
    if status.trim().is_empty() {
        return Ok(());
    }
    if !resumed {
        return Err(OrbitError::config(format!(
            "Workspace Board {} has uncommitted changes; commit or discard them before starting migration",
            board.root.display()
        )));
    }
    let expected: BTreeSet<String> = plan
        .files
        .iter()
        .map(|file| file.path.to_string_lossy().replace('\\', "/"))
        .collect();
    let expected = if plan.destination_config_after.is_some() {
        expected
            .into_iter()
            .chain(std::iter::once("config.toml".to_string()))
            .collect()
    } else {
        expected
    };
    for line in status.lines() {
        let Some(path) = line.get(3..) else {
            return Err(OrbitError::config(
                "could not parse Workspace Board Git status",
            ));
        };
        let path = path.trim().replace('\\', "/");
        if !expected.contains(&path) {
            return Err(OrbitError::config(format!(
                "Workspace Board has uncommitted change `{path}` outside the saved migration; commit or discard it before resuming"
            )));
        }
    }
    Ok(())
}

fn render_plan(
    plan: &MigrationPlan,
    applied: bool,
    resumed: bool,
    commit: Option<String>,
) -> crate::output::CommandOutput {
    let data = json!({
        "workspace": plan.workspace,
        "destination": plan.destination,
        "destination_created": applied && plan.initialize_destination,
        "will_create_destination": !applied && plan.initialize_destination,
        "sources": plan.sources,
        "source_boards": plan.sources.len(),
        "tasks": plan.task_count,
        "files": plan.files.len() + usize::from(plan.destination_config_after.is_some()),
        "applied": applied,
        "resumed": resumed,
        "id_mappings": plan.mappings,
        "accepted_prefixes_added": plan.accepted_prefixes_added,
        "configuration_source": plan.configuration_source,
        "configuration_conflicts": plan.configuration_conflicts,
        "commit": commit,
    });
    let mut human = format!(
        "{} Workspace Board migration: {} task(s) from {} source Board(s){}\n",
        if applied { "Applied" } else { "Plan for" },
        plan.task_count,
        plan.sources.len(),
        if resumed { " (resumed)" } else { "" }
    );
    for mapping in &plan.mappings {
        human.push_str(&format!(
            "remap {} from {} to {} ({})\n",
            mapping.from,
            mapping.source.display(),
            mapping.to,
            mapping.reason
        ));
    }
    if let Some(source) = &plan.configuration_source {
        human.push_str(&format!(
            "configuration inherited from {} (storage, branch, and ID prefix adapted to the Orbit folder layout)\n",
            source.display()
        ));
    }
    if !plan.accepted_prefixes_added.is_empty() {
        human.push_str(&format!(
            "accepted source Task-ID prefixes: {}\n",
            plan.accepted_prefixes_added.join(", ")
        ));
    }
    for conflict in &plan.configuration_conflicts {
        human.push_str(&format!(
            "config conflict {} from {}: Workspace={} source={} (kept Workspace value)\n",
            conflict.setting,
            conflict.source.display(),
            conflict.workspace_value,
            conflict.source_value
        ));
    }
    if applied {
        human.push_str("source Boards remain unchanged as backups; run `orbit doctor` to verify Workspace discovery\n");
    } else {
        human.push_str("no files were changed; rerun without --dry-run to apply\n");
    }
    crate::output::CommandOutput::new(data, human)
}

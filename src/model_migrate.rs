//! `orbit migrate model`: converts a pre-ADR-0022 Board's Tasks to the Issue
//! model — workflow statuses, named priorities, typed links, properties,
//! components/repository fields, Issue types, and sequential project keys.
//!
//! The layout migration (`orbit migrate`, ADR 0019) must already have run:
//! this command refuses Boards whose discovery verdict still asks for it.
//! Like `orbit slug migrate`, the rewrite is planned deterministically,
//! built and validated inside a disposable Git worktree, and committed as a
//! single commit that the real Board fast-forwards to. Without `--apply`
//! the command only previews; running it on an already-migrated Board is a
//! no-op.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use serde::Serialize;
use serde_json::{Value, json};
use tempfile::Builder;

use crate::board::{self, Board, Config};
use crate::cli::ModelMigrateArgs;
use crate::error::{OrbitError, Result, code};
use crate::git::{self, CommitOutcome};
use crate::graph::{Graph, LoadIssue, LoadMode};
use crate::hierarchy::{Hierarchy, IssueTypeConfig};
use crate::key::{self, KeyState};
use crate::output::CommandOutput;
use crate::task::{self, Task};
use crate::workflow::{Workflow, WorkflowConfig};

const MIGRATE_MESSAGE: &str = "orbit: migrate board model";
const DEFAULT_LOCK_TIMEOUT_MS: u64 = 2000;

/// What the migration found and will change, reported by the dry-run
/// preview and repeated in the apply result.
#[derive(Debug, Default, Serialize)]
pub struct Stats {
    pub tasks_scanned: usize,
    pub files_changed: usize,
    /// Status names found on Tasks before remapping, with counts.
    pub statuses: BTreeMap<String, usize>,
    /// Status remaps applied, `"old → new"` with counts.
    pub status_remaps: BTreeMap<String, usize>,
    /// Legacy numeric priorities rewritten to Priority names.
    pub priorities_remapped: usize,
    /// Non-numeric priorities normalized to the configured spelling.
    pub priorities_normalized: usize,
    pub components_set: usize,
    pub repositories_set: usize,
    pub repository_labels: usize,
    pub properties_merged: usize,
    /// Parent Tasks re-typed to the hierarchy's leaf (SubTask) type.
    pub types_retyped: usize,
    /// Issue type names added to `board.types`/`board.hierarchy`.
    pub types_added: Vec<String>,
    /// Project keys allocated; `keys.toml` is written when non-zero.
    pub keys_allocated: usize,
    pub keys_prefix: Option<String>,
    pub config_changes: Vec<String>,
    /// Graph issues the planned result introduces; a non-empty list blocks
    /// `--apply` (pre-existing issues are left for `orbit doctor`).
    pub new_issues: Vec<LoadIssue>,
    /// Issues the un-migrated Board already reports, for delta reporting.
    pub baseline_issues: Vec<LoadIssue>,
    pub warnings: Vec<String>,
}

#[derive(Debug)]
struct ScannedTask {
    id: String,
    original: String,
    task: Task,
    /// The canonical rendering after migration.
    rendered: Option<String>,
}

/// Per-run migration context shared by every Task transform.
struct Ctx<'a> {
    /// Whether legacy status names remap through the workflow (`todo` → the
    /// open status, and friends). Only a Board with neither
    /// `board.priorities` nor `[workflow]` is unambiguously pre-model.
    remap_statuses: bool,
    workflow: &'a Workflow,
    priorities: &'a [String],
    components: &'a [String],
    hierarchy: &'a Hierarchy,
    key_prefix: String,
    next_key: u64,
    stats: Stats,
}

/// The resolved migration: rewritten config, per-Task renderings, and the
/// stats the preview reports.
struct Plan {
    root: PathBuf,
    /// `config.toml` rewritten to the new model.
    config_toml: String,
    /// `keys.toml` content when the allocator state changes.
    keys_toml: Option<String>,
    tasks: Vec<ScannedTask>,
    stats: Stats,
    /// Whether any file or config write is needed at all.
    needed: bool,
}

pub fn cmd_model(args: &ModelMigrateArgs, dir: Option<&Path>) -> Result<CommandOutput> {
    let discovery = Board::discovery(dir);
    if let Some(error) = discovery.error {
        return Err(error);
    }
    let Some(root) = discovery.root.clone() else {
        return Ok(CommandOutput::new(
            json!({ "migrated": false, "reason": "no Board found; run `orbit init` to create one" }),
            "no Orbit Board found; run `orbit init` to create one\n".to_string(),
        ));
    };
    if discovery.needs_migration {
        return Err(OrbitError::new(
            code::MIGRATION_BLOCKED,
            format!(
                "the Board at {} uses a pre-ADR-0019 layout; run `orbit migrate` before `orbit migrate model`",
                root.display()
            ),
        ));
    }
    if !root.join(".git").exists() {
        return Err(OrbitError::new(
            code::MIGRATION_BLOCKED,
            format!(
                "the Board at {} is not a standalone Git repository; run `orbit migrate` first",
                root.display()
            ),
        ));
    }

    // Hold the Board lock before reading any issue so a concurrent writer cannot land
    // between the scan and the apply and be overwritten by stale renderings.
    let _lock = if args.apply {
        Some(crate::lock::BoardLock::acquire(
            &root.join(".lock"),
            plan_lock_timeout(&root)?,
            true,
        )?)
    } else {
        None
    };
    let plan = build_plan(&root)?;

    if args.apply && plan.needed && !plan.stats.new_issues.is_empty() {
        return Err(blocked_by_issues(&plan.stats.new_issues));
    }
    if args.apply && plan.needed {
        let applied = apply(&plan)?;
        let mut data = plan_data(&plan, true);
        data["commit"] = applied
            .commit
            .map(|commit| json!(commit))
            .unwrap_or(Value::Null);
        let mut human = human_summary(&plan, true);
        if let Some(commit) = &data["commit"].as_str() {
            human.push_str(&format!("  committed as {commit}\n"));
        }
        human.push_str("  run `orbit doctor` to verify the migrated Board\n");
        human
            .push_str("  to roll back, reset the Board repository to the commit before this one\n");
        let mut output = CommandOutput::new(data, human);
        output.warnings = plan.stats.warnings.clone();
        output.warnings.extend(applied.warnings);
        return Ok(output);
    }

    let mut data = plan_data(&plan, false);
    data["applied"] = json!(false);
    let mut output = CommandOutput::new(data, human_summary(&plan, false));
    output.warnings = plan.stats.warnings.clone();
    Ok(output)
}

fn plan_data(plan: &Plan, applied: bool) -> Value {
    let stats = &plan.stats;
    json!({
        "migrated": plan.needed,
        "applied": applied,
        "board": plan.root,
        "issues_scanned": stats.tasks_scanned,
        "files_changed": stats.files_changed,
        "already_migrated": !plan.needed,
        "statuses": stats.statuses,
        "status_remaps": stats.status_remaps,
        "priorities_remapped": stats.priorities_remapped,
        "priorities_normalized": stats.priorities_normalized,
        "components_set": stats.components_set,
        "repositories_set": stats.repositories_set,
        "repository_labels": stats.repository_labels,
        "properties_merged": stats.properties_merged,
        "types_retyped": stats.types_retyped,
        "types_added": stats.types_added,
        "keys_allocated": stats.keys_allocated,
        "keys_prefix": stats.keys_prefix,
        "config_changes": stats.config_changes,
        "new_issues": stats.new_issues,
        "baseline_issues": stats.baseline_issues,
        "commit": Value::Null,
        "warnings": stats.warnings,
    })
}

fn human_summary(plan: &Plan, applied: bool) -> String {
    let stats = &plan.stats;
    if !plan.needed {
        return format!(
            "the Board at {} already uses the Issue model; nothing to migrate\n",
            plan.root.display()
        );
    }
    let mut human = format!(
        "orbit migrate model {} the Board at {}\n  tasks: {} scanned, {} file(s) changed\n  statuses remapped: {}, priorities remapped: {}, keys allocated: {}\n  components set: {}, repositories set: {}, properties merged: {}, types retyped: {}\n",
        if applied { "applied to" } else { "preview for" },
        plan.root.display(),
        stats.tasks_scanned,
        stats.files_changed,
        stats.status_remaps.values().sum::<usize>(),
        stats.priorities_remapped,
        stats.keys_allocated,
        stats.components_set,
        stats.repositories_set,
        stats.properties_merged,
        stats.types_retyped,
    );
    for change in &stats.config_changes {
        human.push_str(&format!("  config: {change}\n"));
    }
    for issue in &stats.new_issues {
        human.push_str(&format!("  issue: {}\n", issue.message));
    }
    human
}

fn blocked_by_issues(issues: &[LoadIssue]) -> OrbitError {
    OrbitError::new(
        code::MIGRATION_BLOCKED,
        "the migration would introduce Board issues; fix them or adjust board.types/board.hierarchy first",
    )
    .with_details(serde_json::json!({
        "reason": "new_issues",
        "issues": issues,
    }))
}

/// Reads `config.toml` tolerantly: pre-model Boards carry values the typed
/// `Config` rejects (`default_priority = 2`, `board.areas`), so the legacy
/// keys are rewritten at the TOML level before the typed parse. Returns the
/// typed config plus whether legacy status names should remap — true only
/// when the Board has neither `board.priorities` nor a `[workflow]` section,
/// the unambiguous pre-model shape.
fn load_config(root: &Path) -> Result<(Config, bool)> {
    let path = root.join("config.toml");
    let content = std::fs::read_to_string(&path)
        .map_err(|error| OrbitError::io(format!("read {}", path.display()), error))?;
    let mut doc: toml::Value = toml::from_str(&content)
        .map_err(|error| OrbitError::config(format!("invalid {}: {error}", path.display())))?;

    let has_workflow = doc.get("workflow").is_some();
    let remap_statuses = {
        let board_table = doc
            .get_mut("board")
            .and_then(toml::Value::as_table_mut)
            .ok_or_else(|| OrbitError::config("config.toml has no [board] section"))?;

        // `board.priorities` only exists on post-model Boards: `orbit init`
        // always writes it and no pre-model config carries it.
        let legacy_model = board_table.get("priorities").is_none();

        // Numeric `default_priority` (0–3, lowest number = most important)
        // maps onto the ordinal positions of the effective Priority list.
        let priorities = effective_priorities(board_table);
        let default_priority = board_table.get("default_priority").cloned();
        if let Some(value) = default_priority {
            match value {
                toml::Value::Integer(number) => {
                    let index = usize::try_from(number).unwrap_or(usize::MAX);
                    let name = priorities.get(index).ok_or_else(|| {
                        OrbitError::config(format!(
                            "board.default_priority {number} is out of range for {} priorities",
                            priorities.len()
                        ))
                    })?;
                    board_table.insert(
                        "default_priority".to_string(),
                        toml::Value::String(name.clone()),
                    );
                }
                toml::Value::String(_) => {}
                other => {
                    return Err(OrbitError::config(format!(
                        "board.default_priority has unsupported value {other}"
                    )));
                }
            }
        }

        // `board.areas` restricted the retired `area` field;
        // `board.components` is its Issue-model replacement.
        if let Some(toml::Value::Array(areas)) = board_table.remove("areas") {
            let mut components: BTreeSet<String> = board_table
                .get("components")
                .and_then(toml::Value::as_array)
                .map(|list| {
                    list.iter()
                        .filter_map(toml::Value::as_str)
                        .map(str::to_string)
                        .collect()
                })
                .unwrap_or_default();
            for area in areas.iter().filter_map(toml::Value::as_str) {
                components.insert(area.to_string());
            }
            if !components.is_empty() {
                board_table.insert(
                    "components".to_string(),
                    toml::Value::Array(components.into_iter().map(toml::Value::String).collect()),
                );
            }
        }
        legacy_model && !has_workflow
    };

    let config: Config = doc
        .try_into()
        .map_err(|error| OrbitError::config(format!("invalid {}: {error}", path.display())))?;
    config.validate()?;
    crate::modules::validate(&config)?;
    Ok((config, remap_statuses))
}

/// The Priority names a numeric legacy priority indexes into: the configured
/// `board.priorities` when present, else the shipped default vocabulary.
fn effective_priorities(board_table: &toml::Table) -> Vec<String> {
    board_table
        .get("priorities")
        .and_then(toml::Value::as_array)
        .map(|list| {
            list.iter()
                .filter_map(toml::Value::as_str)
                .map(str::to_string)
                .collect::<Vec<String>>()
        })
        .filter(|list| !list.is_empty())
        .unwrap_or_else(task::default_priorities)
}

fn plan_lock_timeout(root: &Path) -> Result<u64> {
    let content = std::fs::read_to_string(root.join("config.toml")).unwrap_or_default();
    let doc: toml::Value =
        toml::from_str(&content).unwrap_or(toml::Value::Table(toml::Table::new()));
    Ok(doc
        .get("board")
        .and_then(|board| board.get("lock_timeout_ms"))
        .and_then(toml::Value::as_integer)
        .and_then(|value| u64::try_from(value).ok())
        .unwrap_or(DEFAULT_LOCK_TIMEOUT_MS))
}

/// Scans every Task file once, applies the model transforms, and derives the
/// config mutations (types, workflow) the scan requires.
fn build_plan(root: &Path) -> Result<Plan> {
    let (mut config, remap_statuses) = load_config(root)?;
    let mut stats = Stats::default();

    // Scan Tasks. Status remaps only run on unambiguously pre-model Boards;
    // every other transform is idempotent.
    let mut tasks = Vec::new();
    for (id, path) in task_files(&root.join("tasks"))? {
        let content = std::fs::read_to_string(&path)
            .map_err(|error| OrbitError::io(format!("read {}", path.display()), error))?;
        let task = task::parse_task(id.clone(), &content)?;
        stats.tasks_scanned += 1;
        *stats.statuses.entry(task.front.status.clone()).or_insert(0) += 1;
        tasks.push(ScannedTask {
            id,
            original: content,
            task,
            rendered: None,
        });
    }

    // Issue types the files use but the Board does not configure join
    // `board.types` and the hierarchy level that can still parent SubTasks.
    let known: BTreeSet<String> = Hierarchy::from_config(&config.board)?
        .type_names()
        .iter()
        .map(|name| name.to_string())
        .collect();
    let used: BTreeSet<String> = tasks
        .iter()
        .map(|entry| entry.task.front.task_type.clone())
        .collect();
    let mut types_added: Vec<String> = used.difference(&known).cloned().collect();
    types_added.sort();
    for name in &types_added {
        config.board.types.push(IssueTypeConfig {
            name: name.clone(),
            required: Vec::new(),
        });
        stats.config_changes.push(format!("board.types += {name}"));
    }
    if !types_added.is_empty() {
        let level = placement_level(&config.board);
        for name in &types_added {
            if let Some(level_config) = config.board.hierarchy.get_mut(level) {
                level_config.types.push(name.clone());
                level_config.types.sort();
            }
        }
        stats
            .config_changes
            .push("board.hierarchy updated for added types".to_string());
    }
    let hierarchy = Hierarchy::from_config(&config.board)?;

    // A pre-model Board gets the standard workflow written explicitly so the
    // status remap has a stable target and the config documents it.
    if remap_statuses && config.workflow.is_default() {
        config.workflow = WorkflowConfig {
            template: Some("standard".to_string()),
            ..WorkflowConfig::default()
        };
        stats
            .config_changes
            .push("workflow.template = \"standard\"".to_string());
    }
    let workflow = Workflow::from_config(&config)?;

    // Key allocation state: `keys.toml` when present, else derived defaults
    // raised past the highest key or alias already on a Task.
    let mut key_state = load_key_state(root, &config)?;
    let max_found = tasks
        .iter()
        .flat_map(|entry| key::task_keys(&entry.task.front))
        .filter_map(key::key_number)
        .max()
        .unwrap_or(0);
    key_state.next_key = key_state.next_key.max(max_found + 1);

    let mut ctx = Ctx {
        remap_statuses,
        workflow: &workflow,
        priorities: &config.board.priorities,
        components: &config.board.components,
        hierarchy: &hierarchy,
        key_prefix: key_state.prefix.clone(),
        next_key: key_state.next_key,
        stats,
    };
    ctx.stats.keys_prefix = Some(ctx.key_prefix.clone());

    // Field transforms first (status, priority, extra keys, labels, keys) —
    // none depend on other Tasks.
    // Allocate keys in creation order so ORB-N follows chronology, not id spelling.
    tasks.sort_by(|a, b| (a.task.front.created_at, &a.id).cmp(&(b.task.front.created_at, &b.id)));
    for entry in &mut tasks {
        migrate_fields(&mut ctx, &mut entry.task);
    }

    // Parent-aware re-typing, parents first so a child's decision sees the
    // parent's final type.
    let order = parent_first_order(&tasks);
    let mut final_types: BTreeMap<String, String> = BTreeMap::new();
    for index in order {
        let entry = &mut tasks[index];
        retype_if_needed(&mut ctx, &mut entry.task, &final_types);
        final_types.insert(entry.task.id.clone(), entry.task.front.task_type.clone());
    }

    // Render every Task and record which files change.
    let mut files_changed = 0;
    for entry in &mut tasks {
        let rendered = task::render_task(&entry.task)?;
        // A Windows checkout may hold CRLF; git normalizes it, so it is not a change.
        if rendered.replace("\r\n", "\n") != entry.original.replace("\r\n", "\n") {
            files_changed += 1;
        }
        entry.rendered = Some(rendered);
    }
    ctx.stats.files_changed = files_changed;

    // `keys.toml` is written when at least one key was allocated.
    let keys_toml = if ctx.stats.keys_allocated > 0 {
        key_state.next_key = ctx.next_key;
        Some(
            toml::to_string_pretty(&key_state)
                .map_err(|error| OrbitError::config(format!("serialize keys.toml: {error}")))?,
        )
    } else {
        None
    };

    let config_toml = config.to_toml()?;
    let original_config = std::fs::read_to_string(root.join("config.toml")).unwrap_or_default();
    let needed = files_changed > 0
        || keys_toml.is_some()
        || config_toml.replace("\r\n", "\n") != original_config.replace("\r\n", "\n");

    // Validate the planned result in memory and keep only the issues the
    // migration *introduces*; pre-existing Board problems stay for
    // `orbit doctor` either way.
    let planned_tasks: Vec<Task> = tasks.iter().map(|entry| entry.task.clone()).collect();
    let mut planned_graph = Graph::from_tasks_with_workflow(
        planned_tasks,
        config.board.non_ready_labels.clone(),
        workflow.clone(),
    );
    planned_graph.hierarchy = hierarchy.clone();
    planned_graph.priorities = config.board.priorities.clone();
    let planned_issues = planned_graph.validate();

    let baseline_tasks: Vec<Task> = tasks
        .iter()
        .map(|entry| {
            task::parse_task(entry.id.clone(), &entry.original)
                .unwrap_or_else(|_| entry.task.clone())
        })
        .collect();
    let mut baseline_graph = Graph::from_tasks_with_workflow(
        baseline_tasks,
        config.board.non_ready_labels.clone(),
        Workflow::standard(),
    );
    baseline_graph.hierarchy = Hierarchy::default();
    baseline_graph.priorities = task::default_priorities();
    let baseline_issues = baseline_graph.validate();
    let baseline_keys = issue_keys(&baseline_issues);
    ctx.stats.baseline_issues = baseline_issues;
    let (carried, introduced): (Vec<_>, Vec<_>) = planned_issues
        .into_iter()
        .partition(|issue| baseline_keys.contains(&issue_key(issue)));
    for issue in &carried {
        ctx.stats.warnings.push(format!(
            "pre-existing Board problem kept (run `orbit doctor`): {} {}",
            issue.code, issue.message
        ));
    }
    ctx.stats.new_issues = introduced;

    Ok(Plan {
        root: root.to_path_buf(),
        config_toml,
        keys_toml,
        tasks,
        stats: ctx.stats,
        needed,
    })
}

/// The hierarchy level new Issue types join: the default type's level when
/// it is not the leaf, otherwise the level above the leaf so the added type
/// can still parent SubTasks like the retired flat types could.
fn placement_level(board: &board::BoardConfig) -> usize {
    let hierarchy = Hierarchy::from_config(board).unwrap_or_default();
    let levels = hierarchy.max_depth();
    let default_level = hierarchy.level_of(board.default_type.as_str()).unwrap_or(0);
    if default_level + 1 < levels {
        default_level
    } else if levels > 1 {
        levels - 2
    } else {
        0
    }
}

/// Field-level transforms that do not depend on other Tasks: status,
/// priority, legacy `extra` keys, `repo:*` labels, and key allocation.
fn migrate_fields(ctx: &mut Ctx, task: &mut Task) {
    let id = task.id.clone();
    let front = &mut task.front;

    if ctx.remap_statuses {
        let mapped = map_legacy_status(&front.status, ctx.workflow);
        if mapped != front.status {
            *ctx.stats
                .status_remaps
                .entry(format!("{} → {}", front.status, mapped))
                .or_insert(0) += 1;
            front.status = mapped;
        }
    }

    let was_numeric = is_numeric_priority(&front.priority);
    let priority = migrate_priority(&front.priority, ctx.priorities);
    if priority != front.priority {
        if was_numeric {
            ctx.stats.priorities_remapped += 1;
        } else {
            ctx.stats.priorities_normalized += 1;
        }
        front.priority = priority;
    }

    // `area:` → `component:`, `repo:` → `repository:`, `annotations:` →
    // `properties:`. Values move only when they fit the new field's shape;
    // anything else stays in `extra` and is reported.
    if let Some(area) = take_extra_string(front, "area") {
        if task::is_valid_component(&area)
            && task::normalize_component(&area, ctx.components).is_ok()
        {
            match &front.component {
                Some(existing) if existing != &area => {
                    ctx.stats.warnings.push(format!(
                        "{id}: `area: {area}` conflicts with `component: {existing}`; kept component, restored area"
                    ));
                    front
                        .extra
                        .insert("area".to_string(), serde_yaml_ng::Value::String(area));
                }
                Some(_) => {}
                None => {
                    front.component = Some(area);
                    ctx.stats.components_set += 1;
                }
            }
        } else {
            ctx.stats.warnings.push(format!(
                "{id}: `area: {area}` is not a valid component; kept as-is"
            ));
            front
                .extra
                .insert("area".to_string(), serde_yaml_ng::Value::String(area));
        }
    }
    if let Some(repository) = take_extra_string(front, "repo") {
        if task::is_valid_repository(&repository) {
            match &front.repository {
                Some(existing) if existing != &repository => {
                    ctx.stats.warnings.push(format!(
                        "{id}: `repo: {repository}` conflicts with `repository: {existing}`; kept repository, restored repo"
                    ));
                    front
                        .extra
                        .insert("repo".to_string(), serde_yaml_ng::Value::String(repository));
                }
                Some(_) => {}
                None => {
                    front.repository = Some(repository);
                    ctx.stats.repositories_set += 1;
                }
            }
        } else {
            ctx.stats.warnings.push(format!(
                "{id}: `repo: {repository}` is not a valid repository; kept as-is"
            ));
            front
                .extra
                .insert("repo".to_string(), serde_yaml_ng::Value::String(repository));
        }
    }
    if let Some(annotations) = front.extra.remove("annotations") {
        match annotations_to_properties(&annotations) {
            Ok(incoming) => {
                // Pre-check every owner so a conflict moves nothing.
                let conflicts: Vec<String> = incoming
                    .iter()
                    .filter(|(owner, _)| front.properties.contains_key(owner))
                    .map(|(owner, _)| owner.clone())
                    .collect();
                if conflicts.is_empty() {
                    for (owner, value) in incoming {
                        front.properties.insert(owner, value);
                        ctx.stats.properties_merged += 1;
                    }
                } else {
                    ctx.stats.warnings.push(format!(
                        "{id}: property owner(s) {} already set; `annotations` kept as-is",
                        conflicts.join(", ")
                    ));
                    front.extra.insert("annotations".to_string(), annotations);
                }
            }
            Err(reason) => {
                ctx.stats.warnings.push(format!(
                    "{id}: `annotations` could not move to `properties` ({reason}); kept as-is"
                ));
                front.extra.insert("annotations".to_string(), annotations);
            }
        }
    }

    // `repo:*` labels become the `repository` field (same rule as the
    // layout migration).
    let repo_label = front
        .labels
        .iter()
        .find_map(|label| label.strip_prefix("repo:"))
        .map(str::to_string);
    if repo_label.is_some() {
        if front.repository.is_none()
            && let Some(name) = &repo_label
        {
            front.repository = Some(name.clone());
        }
        front.labels.retain(|label| !label.starts_with("repo:"));
        ctx.stats.repository_labels += 1;
    }

    if front.key.is_none() {
        front.key = Some(format!("{}-{}", ctx.key_prefix, ctx.next_key));
        ctx.next_key += 1;
        ctx.stats.keys_allocated += 1;
    }
}

/// Re-types a parented Task from the Board's default type to the hierarchy's
/// leaf (SubTask) type when the parent allows it — the canonical shape new
/// `add --parent` writes. Other types are preserved.
fn retype_if_needed(ctx: &mut Ctx, task: &mut Task, final_types: &BTreeMap<String, String>) {
    let default_type = ctx.hierarchy.default_type().to_string();
    if task.front.parent.is_none() || task.front.task_type != default_type {
        return;
    }
    let child_type = ctx.hierarchy.default_child_type().to_string();
    if child_type == default_type {
        return;
    }
    let Some(parent_id) = task.front.parent.clone() else {
        return;
    };
    let Some(parent_type) = final_types.get(&parent_id) else {
        return;
    };
    if ctx.hierarchy.allows_parent(&child_type, parent_type) {
        task.front.task_type = child_type;
        ctx.stats.types_retyped += 1;
    }
}

/// Maps a pre-model status name onto the resolved workflow's preferred
/// status for its category. Other names pass through untouched.
fn map_legacy_status(status: &str, workflow: &Workflow) -> String {
    match status {
        "todo" => workflow.open_status().to_string(),
        "in_progress" => workflow.active_status().to_string(),
        "done" => workflow.done_status().to_string(),
        "cancelled" | "canceled" => workflow.cancelled_status().to_string(),
        other => other.to_string(),
    }
}

fn is_numeric_priority(priority: &str) -> bool {
    !priority.is_empty() && priority.bytes().all(|byte| byte.is_ascii_digit())
}

/// Normalizes a stored Priority: a legacy number indexes the Board's
/// ordered Priority list (0 = most important); a name is matched
/// case-insensitively against the configured vocabulary.
fn migrate_priority(priority: &str, priorities: &[String]) -> String {
    let trimmed = priority.trim();
    if is_numeric_priority(trimmed) {
        if let Ok(index) = trimmed.parse::<usize>()
            && let Some(name) = priorities.get(index)
        {
            return name.clone();
        }
        return trimmed.to_lowercase();
    }
    let lowered = trimmed.to_lowercase();
    priorities
        .iter()
        .find(|name| name.eq_ignore_ascii_case(&lowered))
        .cloned()
        .unwrap_or(lowered)
}

/// Removes and returns a string `extra` value, leaving non-string or absent
/// values untouched.
fn take_extra_string(front: &mut task::TaskFrontmatter, key: &str) -> Option<String> {
    let value = front.extra.get(key)?;
    let text = value.as_str()?.to_string();
    front.extra.remove(key);
    Some(text)
}

/// Converts the `annotations` map (owner → JSON object) into `properties`
/// entries. Every owner must map to a JSON object; anything else fails so
/// the caller can keep the original key.
fn annotations_to_properties(
    annotations: &serde_yaml_ng::Value,
) -> std::result::Result<Vec<(String, serde_json::Value)>, String> {
    let serde_yaml_ng::Value::Mapping(map) = annotations else {
        return Err("expected a map of owner to object".to_string());
    };
    let mut properties = Vec::with_capacity(map.len());
    for (owner, value) in map {
        let Some(owner) = owner.as_str() else {
            return Err("owner keys must be strings".to_string());
        };
        let Some(json) = yaml_to_json(value) else {
            return Err(format!("owner `{owner}` is not convertible to JSON"));
        };
        if !json.is_object() {
            return Err(format!("owner `{owner}` must map to a JSON object"));
        }
        properties.push((owner.to_string(), json));
    }
    Ok(properties)
}

fn yaml_to_json(value: &serde_yaml_ng::Value) -> Option<serde_json::Value> {
    match value {
        serde_yaml_ng::Value::Null => Some(serde_json::Value::Null),
        serde_yaml_ng::Value::Bool(flag) => Some(serde_json::Value::Bool(*flag)),
        serde_yaml_ng::Value::Number(number) => {
            if let Some(integer) = number.as_i64() {
                Some(serde_json::Value::from(integer))
            } else if let Some(unsigned) = number.as_u64() {
                Some(serde_json::Value::from(unsigned))
            } else {
                number.as_f64().map(serde_json::Value::from)
            }
        }
        serde_yaml_ng::Value::String(text) => Some(serde_json::Value::String(text.clone())),
        serde_yaml_ng::Value::Sequence(sequence) => sequence
            .iter()
            .map(yaml_to_json)
            .collect::<Option<Vec<_>>>()
            .map(serde_json::Value::Array),
        serde_yaml_ng::Value::Mapping(map) => {
            let mut object = serde_json::Map::new();
            for (key, item) in map {
                let key = key.as_str()?;
                object.insert(key.to_string(), yaml_to_json(item)?);
            }
            Some(serde_json::Value::Object(object))
        }
        serde_yaml_ng::Value::Tagged(tagged) => yaml_to_json(&tagged.value),
    }
}

/// `keys.toml` state, tolerant of a missing or hand-edited file.
fn load_key_state(root: &Path, config: &Config) -> Result<KeyState> {
    let path = root.join(key::STATE_FILE);
    match std::fs::read_to_string(&path) {
        Ok(content) => {
            let mut state: KeyState = toml::from_str(&content).map_err(|error| {
                OrbitError::config(format!("invalid {}: {error}", path.display()))
            })?;
            if state.prefix.is_empty() {
                state.prefix = key::derive_prefix(&config.id);
            }
            Ok(state)
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(KeyState {
            prefix: key::derive_prefix(&config.id),
            next_key: 1,
        }),
        Err(error) => Err(OrbitError::io(format!("read {}", path.display()), error)),
    }
}

/// Order Task indexes so every Task is visited after its parent. Missing
/// parents and cycles degrade to ID order for the affected Tasks.
fn parent_first_order(tasks: &[ScannedTask]) -> Vec<usize> {
    let by_id: BTreeMap<&str, usize> = tasks
        .iter()
        .enumerate()
        .map(|(index, entry)| (entry.id.as_str(), index))
        .collect();
    let mut depth = vec![0usize; tasks.len()];
    for (index, entry) in tasks.iter().enumerate() {
        let mut seen = BTreeSet::new();
        let mut current = entry.task.front.parent.as_deref();
        let mut hops = 0;
        while let Some(parent) = current {
            if !seen.insert(parent) || hops > tasks.len() {
                break;
            }
            hops += 1;
            match by_id.get(parent) {
                Some(&parent_index) => {
                    depth[index] = depth[index].max(hops);
                    current = tasks[parent_index].task.front.parent.as_deref();
                }
                None => break,
            }
        }
    }
    let mut order: Vec<usize> = (0..tasks.len()).collect();
    order.sort_by_key(|&index| (depth[index], tasks[index].id.clone()));
    order
}

/// The `<id>.md` Task files under `tasks_dir`; a missing folder has none.
fn task_files(tasks_dir: &Path) -> Result<Vec<(String, PathBuf)>> {
    let entries = match std::fs::read_dir(tasks_dir) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(error) => {
            return Err(OrbitError::io(
                format!("read {}", tasks_dir.display()),
                error,
            ));
        }
    };
    let mut files = Vec::new();
    for entry in entries {
        let path = entry
            .map_err(|e| OrbitError::io(format!("read {}", tasks_dir.display()), e))?
            .path();
        if path.extension().and_then(|e| e.to_str()) != Some("md") {
            continue;
        }
        let Some(id) = path.file_stem().and_then(|s| s.to_str()) else {
            continue;
        };
        if !id.starts_with('.') {
            files.push((id.to_string(), path.clone()));
        }
    }
    // Deterministic key allocation order.
    files.sort_by(|a, b| a.0.cmp(&b.0));
    Ok(files)
}

fn issue_key(issue: &LoadIssue) -> String {
    format!(
        "{}|{}|{}",
        issue.code,
        issue.issue.clone().unwrap_or_default(),
        issue.message
    )
}

fn issue_keys(issues: &[LoadIssue]) -> BTreeSet<String> {
    issues.iter().map(issue_key).collect()
}

struct Applied {
    commit: Option<String>,
    warnings: Vec<String>,
}

/// Builds, validates, and commits the migration in a temporary worktree,
/// then fast-forwards the real Board to that commit. The caller holds the
/// exclusive lock; the tree must be clean.
fn apply(plan: &Plan) -> Result<Applied> {
    if !plan.needed {
        return Ok(Applied {
            commit: None,
            warnings: Vec::new(),
        });
    }
    if !git::status_porcelain(&plan.root)?.trim().is_empty() {
        return Err(OrbitError::new(
            code::MIGRATION_BLOCKED,
            "the Board has uncommitted changes; commit or discard them before migrating",
        ));
    }
    let base = git::head_commit(&plan.root)?;

    let temp = Builder::new()
        .prefix("orbit-model-migrate-")
        .tempdir()
        .map_err(|error| OrbitError::io("create temporary migration directory", error))?;
    let temp_path = temp.path().join("board");
    git::worktree_add_detached(&plan.root, &temp_path)?;

    match build_and_commit(plan, &temp_path) {
        Ok(commit) => {
            let current = match git::head_commit(&plan.root) {
                Ok(current) => current,
                Err(error) => {
                    let _ = git::worktree_remove(&plan.root, &temp_path);
                    return Err(error);
                }
            };
            if current != base {
                let _ = git::worktree_remove(&plan.root, &temp_path);
                return Err(OrbitError::new(
                    code::MIGRATION_BLOCKED,
                    "the Board branch moved while the migration was being built; no files were changed, retry the migration",
                )
                .with_details(serde_json::json!({ "reason": "board_moved" })));
            }
            if let Err(error) = git::reset_hard(&plan.root, &commit) {
                let path = temp_path.clone();
                std::mem::forget(temp);
                return Err(OrbitError::new(
                    code::GIT_ERROR,
                    format!(
                        "{}; the migrated commit {commit} is in the temporary worktree {}; inspect it before retrying",
                        error.message,
                        path.display()
                    ),
                ));
            }

            let mut warnings = Vec::new();
            if let Err(error) = git::worktree_remove(&plan.root, &temp_path) {
                warnings.push(format!(
                    "could not remove the temporary worktree {}: {}; run `git worktree prune`",
                    temp_path.display(),
                    error.message
                ));
            }
            Ok(Applied {
                commit: Some(commit),
                warnings,
            })
        }
        Err(error) => {
            let _ = git::worktree_remove(&plan.root, &temp_path);
            Err(error)
        }
    }
}

/// Writes the planned config, Task files, and `keys.toml` into the temporary
/// worktree, validates the result as a Board, and commits it there.
fn build_and_commit(plan: &Plan, root: &Path) -> Result<String> {
    std::fs::write(root.join("config.toml"), plan.config_toml.as_bytes())
        .map_err(|error| OrbitError::io("write migrated config.toml", error))?;
    if let Some(keys_toml) = &plan.keys_toml {
        std::fs::write(root.join(key::STATE_FILE), keys_toml.as_bytes())
            .map_err(|error| OrbitError::io("write migrated keys.toml", error))?;
    }
    for entry in &plan.tasks {
        if let Some(rendered) = &entry.rendered {
            let path = root.join("tasks").join(format!("{}.md", entry.id));
            std::fs::write(&path, rendered.as_bytes())
                .map_err(|error| OrbitError::io(format!("write {}", path.display()), error))?;
        }
    }

    // Validate the complete result before the real Board moves. A bug here
    // leaves the original Board untouched and only discards the temp
    // worktree. Lenient loading plus the same new-issue delta the plan
    // computed tolerates problems the Board already had.
    let config: Config = toml::from_str(&plan.config_toml)
        .map_err(|error| OrbitError::config(format!("migrated config.toml: {error}")))?;
    let temp_board = Board {
        root: root.to_path_buf(),
        lock_path: root.join(".lock"),
        config,
        warnings: Vec::new(),
        orbit_dir: None,
        needs_migration: false,
        commit_failed: Default::default(),
    };
    let (graph, mut issues) = Graph::load(&temp_board, LoadMode::Lenient)?;
    crate::health::comment_files(&temp_board, &graph, true, &mut issues)?;
    let baseline = issue_keys(&plan.stats.baseline_issues);
    let fresh: Vec<LoadIssue> = issues
        .into_iter()
        .filter(|issue| !baseline.contains(&issue_key(issue)))
        .collect();
    if !fresh.is_empty() {
        return Err(OrbitError::board_corrupt(
            "the migrated Board failed validation; the original Board was not changed",
            serde_json::json!({ "issues": fresh }),
        ));
    }

    match git::commit(root, MIGRATE_MESSAGE)? {
        CommitOutcome::Committed => {}
        other => {
            return Err(OrbitError::git(format!(
                "migration produced no commit ({other:?})"
            )));
        }
    }
    git::head_commit(root)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn legacy_statuses_map_onto_workflow_categories() {
        let workflow = Workflow::standard();
        assert_eq!(map_legacy_status("todo", &workflow), "ready");
        assert_eq!(map_legacy_status("in_progress", &workflow), "in_progress");
        assert_eq!(map_legacy_status("done", &workflow), "done");
        assert_eq!(map_legacy_status("cancelled", &workflow), "cancelled");
        assert_eq!(map_legacy_status("canceled", &workflow), "cancelled");
        assert_eq!(map_legacy_status("ready", &workflow), "ready");
        assert_eq!(map_legacy_status("in_review", &workflow), "in_review");
    }

    #[test]
    fn numeric_priorities_index_the_ordered_list() {
        let priorities = task::default_priorities();
        assert_eq!(migrate_priority("0", &priorities), "highest");
        assert_eq!(migrate_priority("2", &priorities), "medium");
        assert_eq!(migrate_priority("3", &priorities), "low");
        assert_eq!(migrate_priority("9", &priorities), "9");
        assert_eq!(migrate_priority("HIGH", &priorities), "high");
        assert_eq!(migrate_priority("custom", &priorities), "custom");
    }

    #[test]
    fn annotations_convert_to_properties() {
        let yaml: serde_yaml_ng::Value = serde_yaml_ng::from_str(
            "devin:\n  score: 3\n  tags:\n  - a\n  - b\nkai:\n  note: hi\n",
        )
        .unwrap();
        let properties = annotations_to_properties(&yaml).unwrap();
        assert_eq!(properties.len(), 2);
        assert_eq!(properties[0].0, "devin");
        assert_eq!(properties[0].1["score"], serde_json::json!(3));
        assert_eq!(properties[0].1["tags"], serde_json::json!(["a", "b"]));
        assert_eq!(properties[1].1["note"], serde_json::json!("hi"));
    }

    #[test]
    fn annotations_reject_non_object_owners() {
        let yaml: serde_yaml_ng::Value = serde_yaml_ng::from_str("devin: 3\n").unwrap();
        assert!(annotations_to_properties(&yaml).is_err());
        let yaml: serde_yaml_ng::Value = serde_yaml_ng::from_str("- a\n- b\n").unwrap();
        assert!(annotations_to_properties(&yaml).is_err());
    }

    fn scanned(id: &str, parent: Option<&str>) -> ScannedTask {
        let now = "2026-01-01T00:00:00Z".parse().unwrap();
        ScannedTask {
            id: id.to_string(),
            original: String::new(),
            task: Task {
                id: id.to_string(),
                front: task::TaskFrontmatter {
                    title: id.to_string(),
                    status: "todo".to_string(),
                    key: None,
                    key_aliases: Vec::new(),
                    resolution: None,
                    task_type: "task".to_string(),
                    priority: "medium".to_string(),
                    labels: Vec::new(),
                    links: Vec::new(),
                    parent: parent.map(str::to_string),
                    checkpoint: None,
                    component: None,
                    verification: None,
                    github: None,
                    remote: None,
                    repository: None,
                    external_id: None,
                    assignee: None,
                    lease: None,
                    reporter: None,
                    source_key: None,
                    properties: Default::default(),
                    created_at: now,
                    updated_at: now,
                    extra: Default::default(),
                },
                body: String::new(),
            },
            rendered: None,
        }
    }

    #[test]
    fn parent_first_order_visits_parents_before_children() {
        let tasks = vec![
            scanned("orb-child", Some("orb-parent")),
            scanned("orb-parent", None),
            scanned("orb-other", None),
        ];
        let order = parent_first_order(&tasks);
        let positions: BTreeMap<String, usize> = order
            .iter()
            .enumerate()
            .map(|(position, &index)| (tasks[index].id.clone(), position))
            .collect();
        assert!(positions["orb-parent"] < positions["orb-child"]);
    }
}

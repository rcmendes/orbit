//! In-graph Task operations shared by single commands and `orbit batch`.
//! Each operation changes an in-memory Graph and reports whether the Task
//! changed; callers decide when to write files and commit.

use chrono::{DateTime, Utc};
use std::collections::BTreeMap;

use crate::board::Board;
use crate::commands::{
    normalize_labels, normalize_waiting, parse_component_option, validate_priority,
};
use crate::error::{OrbitError, Result, code};
use crate::graph::Graph;
use crate::mutation::TerminalKind;
use crate::task::{self, Status, Task, TaskFrontmatter, TaskType};
use crate::transition::{ClaimGate, FieldUpdate, Outcome, TransitionError};

/// Fields for a new Task. Values are raw and validated by [`add`].
pub struct NewTask {
    pub title: String,
    pub priority: Option<u8>,
    pub task_type: Option<TaskType>,
    pub labels: Vec<String>,
    pub depends_on: Vec<String>,
    pub parent: Option<String>,
    pub waiting_on: Option<String>,
    pub component: Option<String>,
    pub verification: Option<String>,
    pub repository: Option<String>,
    pub external_id: Option<String>,
    pub body: String,
    pub source_key: Option<String>,
    pub properties: BTreeMap<String, serde_json::Value>,
}

pub struct AddOutcome {
    pub id: String,
    /// True when an existing Task with the same Source Key was returned.
    pub matched: bool,
}

/// Validates a new Task and inserts it into the Graph, or returns the Task
/// that already carries its Source Key. Writes no files.
pub fn add(
    graph: &mut Graph,
    board: &Board,
    new: NewTask,
    now: DateTime<Utc>,
) -> Result<AddOutcome> {
    if let Some(key) = &new.source_key {
        task::validate_source_key(key)?;
        if let Some(existing) = graph.find_by_source_key(key)? {
            return Ok(AddOutcome {
                id: existing.id.clone(),
                matched: true,
            });
        }
    }

    let title = new.title.trim().to_string();
    if title.is_empty() {
        return Err(OrbitError::invalid_argument("title must not be empty"));
    }

    let priority = new.priority.unwrap_or(board.config.board.default_priority);
    validate_priority(priority)?;
    let task_type = new.task_type.unwrap_or(TaskType::Task);
    let labels = normalize_labels(&new.labels)?;

    let mut depends_on = Vec::new();
    for dep in new.depends_on {
        if !graph.tasks.contains_key(&dep) {
            return Err(OrbitError::not_found(&dep));
        }
        if !depends_on.contains(&dep) {
            depends_on.push(dep);
        }
    }

    let parent = match new.parent {
        Some(parent_id) => {
            let parent_task = graph.get(&parent_id)?;
            if parent_task.front.status.is_terminal() {
                return Err(OrbitError::invalid_argument(format!(
                    "parent {parent_id} is {}; a SubTask needs an active Parent Task",
                    parent_task.front.status.as_str()
                )));
            }
            if graph.depth(&parent_id) + 1 > usize::from(graph.max_subtask_depth) {
                let rule = if graph.max_subtask_depth == 1 {
                    "SubTasks are one level deep; raise board.max_subtask_depth to nest further"
                        .to_string()
                } else {
                    format!(
                        "SubTasks are limited to {} levels by board.max_subtask_depth",
                        graph.max_subtask_depth
                    )
                };
                return Err(OrbitError::invalid_argument(format!(
                    "parent {parent_id} is a SubTask; {rule}"
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

    if let Some(repository) = &new.repository
        && !task::is_valid_repository(repository)
    {
        return Err(OrbitError::invalid_argument(format!(
            "repository `{repository}` is not a valid Repository name"
        )));
    }
    if let Some(external_id) = &new.external_id {
        task::validate_external_id(external_id)?;
        if let Some(existing) = graph.find_by_external_id(external_id)? {
            return Err(OrbitError::new(
                code::DUPLICATE_EXTERNAL_ID,
                format!(
                    "external id {external_id} is already on task {}",
                    existing.id
                ),
            ));
        }
    }
    let waiting_on = normalize_waiting(new.waiting_on)?;
    let component = parse_component_option(board, new.component)?;
    let verification = task::normalize_verification(new.verification)?;
    for (owner, value) in &new.properties {
        task::validate_property(owner, value)?;
    }
    let id = generate_unique_id(graph, board, parent.as_deref())?;

    graph.insert(Task {
        id: id.clone(),
        front: TaskFrontmatter {
            title,
            status: Status::Todo,
            task_type,
            priority,
            labels,
            depends_on,
            parent,
            waiting_on,
            checkpoint: None,
            component,
            verification,
            github: None,
            remote: None,
            repository: new.repository,
            external_id: new.external_id,
            claim: None,
            stray_claim_times: None,
            created_at: now,
            updated_at: now,
            source_key: new.source_key,
            properties: new.properties,
            extra: Default::default(),
        },
        body: new.body,
    });
    Ok(AddOutcome { id, matched: false })
}

/// A fresh Task ID that is neither on disk nor already in the Graph (a Batch
/// may have added Tasks that are not written yet).
fn generate_unique_id(graph: &Graph, board: &Board, parent: Option<&str>) -> Result<String> {
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
        if !board.task_path(&id).exists() && !graph.tasks.contains_key(&id) {
            return Ok(id);
        }
    }
    Err(OrbitError::new(
        code::ID_COLLISION,
        "could not generate a unique task id",
    ))
}

/// Sets (`Some`) or removes (`None`) one owner's Property on a Task.
pub fn property(
    graph: &mut Graph,
    id: &str,
    owner: &str,
    value: Option<serde_json::Value>,
    now: DateTime<Utc>,
) -> Result<bool> {
    if let Some(value) = &value {
        crate::task::validate_property(owner, value)?;
    }
    let task = graph.get_mut(id)?;
    let changed = match value {
        Some(value) => {
            task.front.properties.get(owner) != Some(&value) && {
                task.front.properties.insert(owner.to_string(), value);
                true
            }
        }
        None => task.front.properties.remove(owner).is_some(),
    };
    if changed {
        task.touch(now);
    }
    Ok(changed)
}

/// Field changes for [`update`]. `None` and `FieldUpdate::Keep` leave a field as is.
#[derive(Default)]
pub struct TaskUpdate {
    pub title: Option<String>,
    pub body: Option<String>,
    pub priority: Option<u8>,
    pub task_type: Option<TaskType>,
    pub waiting_on: FieldUpdate<String>,
    pub component: FieldUpdate<String>,
    pub verification: FieldUpdate<String>,
    pub repository: FieldUpdate<String>,
    pub external_id: FieldUpdate<String>,
}

/// Applies field changes to a Task. Returns whether any value differs.
pub fn update(
    graph: &mut Graph,
    board: &Board,
    id: &str,
    change: TaskUpdate,
    now: DateTime<Utc>,
) -> Result<bool> {
    if let Some(priority) = change.priority {
        validate_priority(priority)?;
    }
    let current = graph.get(id)?;
    let next_type = change.task_type.unwrap_or(current.front.task_type);
    if next_type == TaskType::Epic && current.front.parent.is_some() {
        return Err(OrbitError::new(
            code::INVALID_TYPE,
            "epic type is only allowed on a top-level Task",
        ));
    }
    let title = match change.title {
        Some(title) => {
            let title = title.trim().to_string();
            if title.is_empty() {
                return Err(OrbitError::invalid_argument("title must not be empty"));
            }
            Some(title)
        }
        None => None,
    };
    let waiting_on = match change.waiting_on {
        FieldUpdate::Set(value) => match normalize_waiting(Some(value))? {
            Some(value) => FieldUpdate::Set(value),
            None => FieldUpdate::Keep,
        },
        other => other,
    };
    let component = match change.component {
        FieldUpdate::Set(value) => match parse_component_option(board, Some(value))? {
            Some(value) => FieldUpdate::Set(value),
            None => FieldUpdate::Keep,
        },
        other => other,
    };
    if let FieldUpdate::Set(value) = &change.repository
        && !task::is_valid_repository(value)
    {
        return Err(OrbitError::invalid_argument(format!(
            "repository `{value}` is not a valid Repository name"
        )));
    }
    if let FieldUpdate::Set(value) = &change.external_id {
        task::validate_external_id(value)?;
        if let Some(existing) = graph.find_by_external_id(value)?
            && existing.id != id
        {
            return Err(OrbitError::new(
                code::DUPLICATE_EXTERNAL_ID,
                format!("external id {value} is already on task {}", existing.id),
            ));
        }
    }
    let verification = match change.verification {
        FieldUpdate::Set(value) => match task::normalize_verification(Some(value))? {
            Some(value) => FieldUpdate::Set(value),
            None => FieldUpdate::Keep,
        },
        other => other,
    };

    let task = graph.get_mut(id)?;
    let before = (task.front.clone(), task.body.clone());
    if let Some(title) = title {
        task.front.title = title;
    }
    if let Some(body) = change.body {
        task.body = body;
    }
    if let Some(priority) = change.priority {
        task.front.priority = priority;
    }
    if let Some(task_type) = change.task_type {
        task.front.task_type = task_type;
    }
    apply_field(&mut task.front.waiting_on, waiting_on);
    apply_field(&mut task.front.component, component);
    apply_field(&mut task.front.verification, verification);
    apply_field(&mut task.front.repository, change.repository);
    apply_field(&mut task.front.external_id, change.external_id);
    let changed = front_differs(&before.0, &task.front) || before.1 != task.body;
    task.touch(now);
    Ok(changed)
}

fn apply_field(field: &mut Option<String>, change: FieldUpdate<String>) {
    match change {
        FieldUpdate::Keep => {}
        FieldUpdate::Clear => *field = None,
        FieldUpdate::Set(value) => *field = Some(value),
    }
}

fn front_differs(before: &TaskFrontmatter, after: &TaskFrontmatter) -> bool {
    before.title != after.title
        || before.priority != after.priority
        || before.task_type != after.task_type
        || before.waiting_on != after.waiting_on
        || before.component != after.component
        || before.verification != after.verification
        || before.repository != after.repository
        || before.external_id != after.external_id
}

/// Adds and removes Labels. Returns whether the Task's Labels changed.
pub fn label(
    graph: &mut Graph,
    id: &str,
    add: &[String],
    remove: &[String],
    now: DateTime<Utc>,
) -> Result<bool> {
    let add = normalize_labels(add)?;
    let remove = normalize_labels(remove)?;
    let task = graph.get_mut(id)?;
    let added = !add.is_empty() && task.add_labels(&add, now);
    let removed = !remove.is_empty() && task.remove_labels(&remove, now);
    Ok(added || removed)
}

/// Makes `id` wait for `on`. An existing edge is unchanged (`Ok(false)`).
pub fn depend(graph: &mut Graph, id: &str, on: &str, now: DateTime<Utc>) -> Result<bool> {
    {
        let task = graph.get(id)?;
        let dep = graph.get(on)?;
        if task.front.parent.as_deref() == Some(on) || dep.front.parent.as_deref() == Some(id) {
            return Err(OrbitError::invalid_argument(
                "parentage is not a dependency; use `orbit add --parent` to create a SubTask",
            ));
        }
        if task.front.status.is_terminal() {
            return Err(OrbitError::new(
                code::INVALID_TRANSITION,
                format!(
                    "{id} is {}; cannot add a dependency to a terminal task",
                    task.front.status.as_str()
                ),
            ));
        }
    }
    if id == on {
        return Err(cycle_error(&[id.to_string(), on.to_string()]));
    }
    if graph.get(id)?.front.depends_on.iter().any(|dep| dep == on) {
        return Ok(false);
    }
    if let Some(cycle) = graph.would_create_cycle(id, on) {
        return Err(cycle_error(&cycle));
    }
    graph.get_mut(id)?.add_dependency(on, now);
    Ok(true)
}

/// Removes the edge from `id` to `on`. A missing edge is unchanged (`Ok(false)`).
pub fn undepend(graph: &mut Graph, id: &str, on: &str, now: DateTime<Utc>) -> Result<bool> {
    Ok(graph.get_mut(id)?.remove_dependency(on, now))
}

pub(crate) fn cycle_error(path: &[String]) -> OrbitError {
    OrbitError::new(
        code::CYCLE_DETECTED,
        format!("dependency cycle: {}", path.join(" -> ")),
    )
    .with_details(serde_json::json!({ "cycle": path }))
}

fn lease(board: &Board) -> chrono::Duration {
    chrono::Duration::minutes(board.config.board.lease_minutes)
}

/// Claims a Task for `by`. The current holder renews its own Claim.
pub fn claim(
    graph: &mut Graph,
    board: &Board,
    id: &str,
    by: &str,
    steal: bool,
    now: DateTime<Utc>,
) -> Result<bool> {
    let task = graph.get(id)?;
    let gate = ClaimGate {
        hold_labels: graph.hold_labels(task),
        blocking: graph.blocking_ids(task),
        open_subtasks: graph.open_children(id),
    };
    graph
        .get_mut(id)?
        .claim(by, steal, lease(board), &gate, now)?;
    Ok(true)
}

/// Extends the holder's Lease. Changes only Lease fields.
pub fn heartbeat(
    graph: &mut Graph,
    board: &Board,
    id: &str,
    by: &str,
    now: DateTime<Utc>,
) -> Result<bool> {
    graph.get_mut(id)?.heartbeat(by, lease(board), now)?;
    Ok(true)
}

/// Releases a Claim. An unclaimed Task is unchanged (`Ok(false)`).
pub fn release(
    graph: &mut Graph,
    id: &str,
    by: Option<&str>,
    force: bool,
    now: DateTime<Utc>,
) -> Result<bool> {
    Ok(graph.get_mut(id)?.release(by, force, now)? == Outcome::Changed)
}

pub struct FinishOutcome {
    /// False when the Task was already in the target state.
    pub changed: bool,
    /// Open SubTasks cancelled because `force` was given.
    pub cancelled_subtasks: Vec<String>,
}

/// Moves a Task to Done or Cancelled, cancelling open SubTasks when forced.
pub fn finish(
    graph: &mut Graph,
    id: &str,
    by: Option<&str>,
    force: bool,
    kind: TerminalKind,
    now: DateTime<Utc>,
) -> Result<FinishOutcome> {
    let open_children = graph.open_children(id);
    if !open_children.is_empty() && !force {
        return Err(OrbitError::new(
            code::SUBTASKS_OPEN,
            format!("{id} has open SubTasks: {}", open_children.join(", ")),
        )
        .with_details(serde_json::json!({"task": id, "subtasks": open_children})));
    }
    let blocking = graph.blocking_ids(graph.get(id)?);
    if matches!(kind, TerminalKind::Done) && !blocking.is_empty() {
        return Err(TransitionError::DependencyBlocked {
            id: id.into(),
            blocking,
        }
        .into());
    }
    let open: Vec<String> = graph
        .descendants(id)
        .into_iter()
        .filter(|descendant| {
            graph
                .get(descendant)
                .is_ok_and(|task| !task.front.status.is_terminal())
        })
        .collect();
    for child in &open {
        graph.get_mut(child)?.cancel(by, true, now)?;
    }
    let task = graph.get_mut(id)?;
    let outcome = match kind {
        TerminalKind::Done => task.complete(by, force, &blocking, &[], now)?,
        TerminalKind::Cancelled => task.cancel(by, force, now)?,
    };
    Ok(FinishOutcome {
        changed: outcome == Outcome::Changed,
        cancelled_subtasks: open,
    })
}

pub fn done(
    graph: &mut Graph,
    id: &str,
    by: Option<&str>,
    force: bool,
    now: DateTime<Utc>,
) -> Result<bool> {
    Ok(finish(graph, id, by, force, TerminalKind::Done, now)?.changed)
}

pub fn cancel(
    graph: &mut Graph,
    id: &str,
    by: Option<&str>,
    force: bool,
    now: DateTime<Utc>,
) -> Result<bool> {
    Ok(finish(graph, id, by, force, TerminalKind::Cancelled, now)?.changed)
}

/// Reopens a Task and, when needed, its completed ancestor Tasks. Returns the
/// reopened ancestors' IDs, nearest first.
pub fn reopen(graph: &mut Graph, id: &str, force: bool, now: DateTime<Utc>) -> Result<Vec<String>> {
    let task = graph.get(id)?.clone();
    let child_dependents = done_dependents(graph, id);
    let mut probe = task.clone();
    probe.reopen(force, &child_dependents, now)?;

    let mut reopened = Vec::new();
    for ancestor_id in graph.ancestors(id) {
        match graph.get(&ancestor_id)?.front.status {
            Status::Done => {
                let ancestor_dependents = done_dependents(graph, &ancestor_id);
                graph
                    .get_mut(&ancestor_id)?
                    .reopen(force, &ancestor_dependents, now)?;
                reopened.push(ancestor_id);
            }
            Status::Cancelled => {
                return Err(OrbitError::new(
                    code::INVALID_TRANSITION,
                    format!(
                        "{id} is a SubTask of cancelled {ancestor_id}; reopen the parent first"
                    ),
                ));
            }
            _ => break,
        }
    }
    graph.get_mut(id)?.reopen(force, &child_dependents, now)?;
    Ok(reopened)
}

fn done_dependents(graph: &Graph, id: &str) -> Vec<String> {
    graph
        .dependents(id)
        .iter()
        .filter(|dependent| dependent.front.status == Status::Done)
        .map(|dependent| dependent.id.clone())
        .collect()
}

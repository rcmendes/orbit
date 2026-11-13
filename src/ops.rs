//! In-graph Task operations shared by single commands and `orbit batch`.
//! Each operation changes an in-memory Graph and reports whether the Task
//! changed; callers decide when to write files and commit.

use chrono::{DateTime, Utc};
use std::collections::BTreeMap;

use crate::board::Board;
use crate::commands::{normalize_labels, normalize_waiting, parse_area_option, validate_priority};
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
    pub area: Option<String>,
    pub verification: Option<String>,
    pub repo: Option<String>,
    pub body: String,
    pub source_key: Option<String>,
    pub annotations: BTreeMap<String, serde_json::Value>,
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

    if let Some(repo) = &new.repo
        && !task::is_valid_repo(repo)
    {
        return Err(OrbitError::invalid_argument(format!(
            "repo `{repo}` is not a valid Repository name"
        )));
    }
    let waiting_on = normalize_waiting(new.waiting_on)?;
    let area = parse_area_option(board, new.area)?;
    let verification = task::normalize_verification(new.verification)?;
    for (owner, value) in &new.annotations {
        task::validate_annotation(owner, value)?;
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
            area,
            verification,
            github: None,
            remote: None,
            repo: new.repo,
            claim: None,
            stray_claim_times: None,
            created_at: now,
            updated_at: now,
            source_key: new.source_key,
            annotations: new.annotations,
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

/// Sets (`Some`) or removes (`None`) one owner's Annotation on a Task.
pub fn annotate(
    graph: &mut Graph,
    id: &str,
    owner: &str,
    value: Option<serde_json::Value>,
    now: DateTime<Utc>,
) -> Result<bool> {
    if let Some(value) = &value {
        crate::task::validate_annotation(owner, value)?;
    }
    let task = graph.get_mut(id)?;
    let changed = match value {
        Some(value) => {
            task.front.annotations.get(owner) != Some(&value) && {
                task.front.annotations.insert(owner.to_string(), value);
                true
            }
        }
        None => task.front.annotations.remove(owner).is_some(),
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
    pub area: FieldUpdate<String>,
    pub verification: FieldUpdate<String>,
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
    let area = match change.area {
        FieldUpdate::Set(value) => match parse_area_option(board, Some(value))? {
            Some(value) => FieldUpdate::Set(value),
            None => FieldUpdate::Keep,
        },
        other => other,
    };
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
    apply_field(&mut task.front.area, area);
    apply_field(&mut task.front.verification, verification);
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
        || before.area != after.area
        || before.verification != after.verification
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
    let open = graph.open_children(id);
    if !open.is_empty() && !force {
        return Err(OrbitError::new(
            code::SUBTASKS_OPEN,
            format!("{id} has open SubTasks: {}", open.join(", ")),
        )
        .with_details(serde_json::json!({"task": id, "subtasks": open})));
    }
    let blocking = graph.blocking_ids(graph.get(id)?);
    if matches!(kind, TerminalKind::Done) && !blocking.is_empty() {
        return Err(TransitionError::DependencyBlocked {
            id: id.into(),
            blocking,
        }
        .into());
    }
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

/// Reopens a Task and, when needed, its Done Parent Task. Returns the
/// reopened parent's ID.
pub fn reopen(
    graph: &mut Graph,
    id: &str,
    force: bool,
    now: DateTime<Utc>,
) -> Result<Option<String>> {
    let task = graph.get(id)?.clone();
    let child_dependents = done_dependents(graph, id);
    let mut probe = task.clone();
    probe.reopen(force, &child_dependents, now)?;

    let mut reopened_parent = None;
    if let Some(parent_id) = task.front.parent.clone() {
        match graph.get(&parent_id)?.front.status {
            Status::Done => {
                let parent_dependents = done_dependents(graph, &parent_id);
                graph
                    .get_mut(&parent_id)?
                    .reopen(force, &parent_dependents, now)?;
                reopened_parent = Some(parent_id);
            }
            Status::Cancelled => {
                return Err(OrbitError::new(
                    code::INVALID_TRANSITION,
                    format!("{id} is a SubTask of cancelled {parent_id}; reopen the parent first"),
                ));
            }
            _ => {}
        }
    }
    graph.get_mut(id)?.reopen(force, &child_dependents, now)?;
    Ok(reopened_parent)
}

fn done_dependents(graph: &Graph, id: &str) -> Vec<String> {
    graph
        .dependents(id)
        .iter()
        .filter(|dependent| dependent.front.status == Status::Done)
        .map(|dependent| dependent.id.clone())
        .collect()
}

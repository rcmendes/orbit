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
use crate::task::{self, IdentityKind, Task, TaskFrontmatter};
use crate::transition::{ClaimGate, FieldUpdate, Outcome, TransitionError};
use crate::workflow::StatusCategory;

/// Fields for a new Task. Values are raw and validated by [`add`].
pub struct NewTask {
    pub title: String,
    pub priority: Option<String>,
    /// Issue type name; `None` resolves to `board.default_type`, or to the
    /// hierarchy's leaf type when `parent` is set.
    pub task_type: Option<String>,
    pub labels: Vec<String>,
    pub depends_on: Vec<String>,
    pub parent: Option<String>,
    pub waiting_on: Option<String>,
    pub component: Option<String>,
    pub verification: Option<String>,
    pub repository: Option<String>,
    pub external_id: Option<String>,
    /// Who created the Task; recorded once at creation.
    pub reporter: Option<String>,
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

    let priority = match &new.priority {
        Some(priority) => validate_priority(board, priority)?,
        None => board.config.board.default_priority.clone(),
    };
    let parent = match new.parent {
        Some(parent_id) => {
            // `parent` stores the immutable internal ID; a project key or
            // alias resolves to it here.
            let parent_id = graph.resolve(&parent_id)?;
            let parent_task = graph.get(&parent_id)?;
            if graph.workflow.is_terminal(&parent_task.front.status) {
                return Err(OrbitError::invalid_argument(format!(
                    "parent {parent_id} is {}; a child Task needs an active parent",
                    parent_task.front.status
                )));
            }
            Some((parent_id, parent_task.front.task_type.clone()))
        }
        None => None,
    };
    let parent_id = parent.as_ref().map(|(id, _)| id.clone());

    let task_type = match new.task_type {
        Some(task_type) => {
            let task_type = crate::hierarchy::normalize_type_name(&task_type);
            if task_type.is_empty() {
                return Err(OrbitError::invalid_argument("type must not be empty"));
            }
            if !graph.hierarchy.is_known_type(&task_type) {
                return Err(crate::hierarchy::unknown_type_error(
                    &graph.hierarchy,
                    &task_type,
                ));
            }
            task_type
        }
        // `--parent` without `--type` files a leaf child: the deepest level's
        // first type, `sub-task` in the default model.
        None => match &parent {
            Some(_) => graph.hierarchy.default_child_type().to_string(),
            None => board.config.board.default_type.clone(),
        },
    };

    if let Some((parent_id, parent_type)) = &parent
        && let Some(reason) = graph
            .hierarchy
            .parent_rule_violation(&task_type, parent_type)
    {
        return Err(OrbitError::new(
            code::INVALID_TYPE,
            format!("cannot add {task_type} under {parent_id}: {reason}"),
        ));
    }

    let labels = normalize_labels(&new.labels)?;

    // `depends_on` ids become blocking `blocks` links; `waiting_on` becomes a
    // blocking `waiting-on` link to an external reference.
    let mut links: Vec<task::Link> = Vec::new();
    for dep in new.depends_on {
        // Link targets store internal IDs; resolve keys and aliases first.
        let dep = graph.resolve(&dep)?;
        let link = task::Link::dependency(&dep);
        if !links.contains(&link) {
            links.push(link);
        }
    }
    let waiting_on = normalize_waiting(new.waiting_on)?;
    if let Some(waiting) = &waiting_on {
        links.push(task::Link::waiting_on(waiting));
    }

    if let Some(parent_id) = &parent_id
        && links
            .iter()
            .any(|link| link.is_blocking() && link.target_task() == Some(parent_id.as_str()))
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
    let reporter = new
        .reporter
        .map(|reporter| reporter.trim().to_string())
        .filter(|reporter| !reporter.is_empty());
    let component = parse_component_option(board, new.component)?;
    let verification = task::normalize_verification(new.verification)?;
    for (owner, value) in &new.properties {
        task::validate_property(owner, value)?;
    }
    let id = generate_unique_id(graph, board)?;
    // The sequential project key is allocated under the same exclusive Board
    // lock the caller holds; `keys.toml` persists the high-water mark.
    let key = crate::key::allocate(graph, board)?;

    let task = Task {
        id: id.clone(),
        front: TaskFrontmatter {
            title,
            status: graph.workflow.initial_status().to_string(),
            key: Some(key),
            key_aliases: Vec::new(),
            resolution: None,
            task_type,
            priority,
            labels,
            links,
            parent: parent_id,
            checkpoint: None,
            component,
            verification,
            github: None,
            remote: None,
            repository: new.repository,
            external_id: new.external_id,
            assignee: None,
            lease: None,
            reporter,
            stray_lease_times: None,
            created_at: now,
            updated_at: now,
            source_key: new.source_key,
            properties: new.properties,
            extra: Default::default(),
        },
        body: new.body,
    };
    let missing = graph.hierarchy.missing_required_fields(&task);
    if !missing.is_empty() {
        return Err(crate::hierarchy::missing_field_error(&id, &missing));
    }
    graph.insert(task);
    Ok(AddOutcome { id, matched: false })
}

/// A fresh Task ID that is neither on disk nor already in the Graph (a Batch
/// may have added Tasks that are not written yet). IDs never encode the
/// parent: `parent` is an ordinary field (ADR 0022).
fn generate_unique_id(graph: &Graph, board: &Board) -> Result<String> {
    let config = &board.config.id;
    for attempt in 0..50 {
        let length = if attempt < 40 {
            config.length
        } else {
            (config.length + 2).min(8)
        };
        let id = task::generate_id(&config.prefix, length);
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
    pub priority: Option<String>,
    /// New Issue type name; must keep the Task's `parent` edge and its
    /// children's edges valid under the Board hierarchy.
    pub task_type: Option<String>,
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
    let priority = change
        .priority
        .as_deref()
        .map(|priority| validate_priority(board, priority))
        .transpose()?;
    let current = graph.get(id)?;
    let next_type = match &change.task_type {
        Some(task_type) => {
            let task_type = crate::hierarchy::normalize_type_name(task_type);
            if task_type.is_empty() {
                return Err(OrbitError::invalid_argument("type must not be empty"));
            }
            if !graph.hierarchy.is_known_type(&task_type) {
                return Err(crate::hierarchy::unknown_type_error(
                    &graph.hierarchy,
                    &task_type,
                ));
            }
            task_type
        }
        None => current.front.task_type.clone(),
    };
    // The new type must keep the edge to the parent and the edges from every
    // child valid under the Board hierarchy.
    if let Some(parent_id) = &current.front.parent
        && let Ok(parent) = graph.get(parent_id)
        && let Some(reason) = graph
            .hierarchy
            .parent_rule_violation(&next_type, &parent.front.task_type)
    {
        return Err(OrbitError::new(
            code::INVALID_TYPE,
            format!("cannot make {id} type `{next_type}` under {parent_id}: {reason}"),
        ));
    }
    for child in graph.children(id) {
        if let Some(reason) = graph
            .hierarchy
            .parent_rule_violation(&child.front.task_type, &next_type)
        {
            return Err(OrbitError::new(
                code::INVALID_TYPE,
                format!(
                    "cannot make {id} type `{next_type}`; child {} has type `{}`: {reason}",
                    child.id, child.front.task_type
                ),
            ));
        }
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
    // `waiting_on` is a convenience over `waiting-on` links to an external
    // reference: Set replaces them, Clear removes them.
    let waiting_change = match change.waiting_on {
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

    let hierarchy = graph.hierarchy.clone();
    let task = graph.get_mut(id)?;
    let before = (task.front.clone(), task.body.clone());
    if let Some(title) = title {
        task.front.title = title;
    }
    if let Some(body) = change.body {
        task.body = body;
    }
    if let Some(priority) = priority {
        task.front.priority = priority;
    }
    if change.task_type.is_some() {
        task.front.task_type = next_type.clone();
    }
    apply_waiting_links(&mut task.front.links, waiting_change);
    apply_field(&mut task.front.component, component);
    apply_field(&mut task.front.verification, verification);
    apply_field(&mut task.front.repository, change.repository);
    apply_field(&mut task.front.external_id, change.external_id);
    let changed = front_differs(&before.0, &task.front) || before.1 != task.body;
    task.touch(now);
    let missing = hierarchy.missing_required_fields(task);
    if !missing.is_empty() {
        return Err(crate::hierarchy::missing_field_error(id, &missing));
    }
    Ok(changed)
}

/// Replaces or removes `waiting-on` external links — the stored form of the
/// `waiting_on` convenience field.
fn apply_waiting_links(links: &mut Vec<task::Link>, change: FieldUpdate<String>) {
    let is_waiting = |link: &task::Link| {
        link.kind == task::LINK_TYPE_WAITING_ON
            && matches!(link.target, task::LinkTarget::External(_))
    };
    match change {
        FieldUpdate::Keep => {}
        FieldUpdate::Clear => links.retain(|link| !is_waiting(link)),
        FieldUpdate::Set(value) => {
            links.retain(|link| !is_waiting(link));
            links.push(task::Link::waiting_on(&value));
        }
    }
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
        || before.links != after.links
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

/// Adds a typed link from `id` to `link.target`. An identical link is
/// unchanged (`Ok(false)`). Blocking links to Orbit Tasks reject cycles and
/// links on terminal Tasks; non-blocking links only need the target to exist.
pub fn link(graph: &mut Graph, id: &str, mut link: task::Link, now: DateTime<Utc>) -> Result<bool> {
    // Both ends store immutable internal IDs; resolve project keys and
    // aliases before any check or write.
    let resolved_id = graph.resolve(id)?;
    let id = resolved_id.as_str();
    if let Some(target) = link.target_task() {
        let resolved = graph.resolve(target)?;
        link.target = task::LinkTarget::Task(resolved);
    }
    {
        let task = graph.get(id)?;
        if let Some(target) = link.target_task() {
            graph.get(target)?;
            if link.is_blocking()
                && (task.front.parent.as_deref() == Some(target)
                    || graph.get(target)?.front.parent.as_deref() == Some(id))
            {
                return Err(OrbitError::invalid_argument(
                    "parentage is not a dependency; use `orbit add --parent` to create a SubTask",
                ));
            }
        }
        if link.is_blocking() && graph.workflow.is_terminal(&task.front.status) {
            return Err(OrbitError::new(
                code::INVALID_TRANSITION,
                format!(
                    "{id} is {}; cannot add a blocking link to a terminal task",
                    task.front.status
                ),
            ));
        }
    }
    if link.is_blocking() && link.target_task() == Some(id) {
        return Err(cycle_error(&[id.to_string(), id.to_string()]));
    }
    if graph
        .get(id)?
        .front
        .links
        .iter()
        .any(|existing| existing.kind == link.kind && existing.target.same_reference(&link.target))
    {
        return Ok(false);
    }
    if link.is_blocking()
        && let Some(target) = link.target_task()
        && let Some(cycle) = graph.would_create_cycle(id, target)
    {
        return Err(cycle_error(&cycle));
    }
    graph.get_mut(id)?.add_link(link, now);
    Ok(true)
}

/// Removes links from `id` matching `kind` (when given) and `target`.
/// A missing link is unchanged (`Ok(false)`).
pub fn unlink(
    graph: &mut Graph,
    id: &str,
    kind: Option<&str>,
    target: &task::LinkTarget,
    now: DateTime<Utc>,
) -> Result<bool> {
    let target = match target {
        task::LinkTarget::Task(target) => task::LinkTarget::Task(graph.resolve(target)?),
        other => other.clone(),
    };
    Ok(graph.get_mut(id)?.remove_links(kind, &target, now))
}

/// Makes `id` wait for `on` through a blocking `blocks` link.
pub fn depend(graph: &mut Graph, id: &str, on: &str, now: DateTime<Utc>) -> Result<bool> {
    link(graph, id, task::Link::dependency(on), now)
}

/// Parses a link target spec: `to` is an Orbit Task ID, `external` is
/// `SYSTEM:ID` (split at the first colon). Exactly one must be given.
pub fn link_target(to: Option<&str>, external: Option<&str>) -> Result<task::LinkTarget> {
    match (to, external) {
        (Some(id), None) => {
            if id.trim().is_empty() {
                return Err(OrbitError::invalid_argument(
                    "link target must not be empty",
                ));
            }
            Ok(task::LinkTarget::Task(id.trim().to_string()))
        }
        (None, Some(spec)) => {
            let (system, id) = spec.split_once(':').ok_or_else(|| {
                OrbitError::invalid_argument(
                    "external link target must be SYSTEM:ID, for example jira:PROJ-123",
                )
            })?;
            let reference = task::ExternalRef {
                system: system.trim().to_string(),
                id: id.trim().to_string(),
                url: None,
            };
            reference.validate().map_err(OrbitError::invalid_argument)?;
            Ok(task::LinkTarget::External(reference))
        }
        _ => Err(OrbitError::invalid_argument(
            "a link needs exactly one target: --to ID or --external SYSTEM:ID",
        )),
    }
}

/// Parses repeatable `KEY=VALUE` attribute pairs; VALUE is read as YAML so
/// numbers, booleans, and lists keep their type.
pub fn link_attributes(pairs: &[String]) -> Result<BTreeMap<String, serde_yaml_ng::Value>> {
    let mut attributes = BTreeMap::new();
    for pair in pairs {
        let (key, value) = pair.split_once('=').ok_or_else(|| {
            OrbitError::invalid_argument(format!("invalid attribute `{pair}`; expected KEY=VALUE"))
        })?;
        let key = key.trim();
        if key.is_empty() {
            return Err(OrbitError::invalid_argument(
                "link attribute names must not be empty",
            ));
        }
        let parsed = serde_yaml_ng::from_str::<serde_yaml_ng::Value>(value)
            .unwrap_or_else(|_| serde_yaml_ng::Value::String(value.to_string()));
        attributes.insert(key.to_string(), parsed);
    }
    Ok(attributes)
}

/// Removes blocking links from `id` to `on`. Non-blocking links stay.
pub fn undepend(graph: &mut Graph, id: &str, on: &str, now: DateTime<Utc>) -> Result<bool> {
    let on = graph.resolve(on)?;
    let target = task::LinkTarget::Task(on);
    let task = graph.get_mut(id)?;
    let before = task.front.links.len();
    task.front
        .links
        .retain(|existing| !(existing.is_blocking() && existing.target == target));
    let changed = task.front.links.len() != before;
    if changed {
        task.touch(now);
    }
    Ok(changed)
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

/// Snapshots the named Tasks so a policy violation can roll the in-memory
/// mutation back before the error reaches the caller.
fn snapshot_tasks(graph: &Graph, ids: &[String]) -> Vec<(String, Task)> {
    ids.iter()
        .filter_map(|id| graph.tasks.get(id).map(|task| (id.clone(), task.clone())))
        .collect()
}

fn restore_tasks(graph: &mut Graph, snapshots: Vec<(String, Task)>) {
    for (id, task) in snapshots {
        graph.tasks.insert(id, task);
    }
}

/// The Task's status after `f` ran, when it differs from `before` — the
/// signal that a governed transition actually happened (lease renewals and
/// same-state no-ops skip policy).
fn transitioned(before: &Task, after: &Task) -> bool {
    before.front.status != after.front.status || before.front.assignee != after.front.assignee
}

/// Claims a Task for `by`, setting the assignee and a fresh lease. The
/// current assignee renews its own claim. The readiness lint gates the
/// claim, and `[policy]` rules on the `claim` transition enforce after it.
pub fn claim(
    graph: &mut Graph,
    board: &Board,
    id: &str,
    by: &str,
    kind: IdentityKind,
    steal: bool,
    now: DateTime<Utc>,
) -> Result<bool> {
    let resolved = graph.resolve(id)?;
    let before = graph
        .tasks
        .get(&resolved)
        .cloned()
        .ok_or_else(|| OrbitError::not_found(id))?;
    // The readiness lint keeps vague Tasks out of agent hands even when
    // they sit in the Ready status.
    crate::policy::lint_error(graph, &resolved)?;
    let task = graph.get(&resolved)?;
    let workflow = graph.workflow.clone();
    let gate = ClaimGate {
        hold_labels: graph.hold_labels(task),
        blocking: graph.blocking_ids(task),
        open_subtasks: graph.open_descendants(&resolved),
    };
    graph
        .get_mut(&resolved)?
        .claim(by, kind, steal, lease(board), &gate, now, &workflow)?;
    let after = graph.get(&resolved)?;
    if transitioned(&before, after) {
        let status = after.front.status.clone();
        crate::policy::enforce(
            graph,
            &resolved,
            crate::workflow::TRANSITION_CLAIM,
            Some(status.as_str()),
            &before,
            now,
        )?;
    }
    Ok(true)
}

/// Assigns a Task to `name` without a lease — how humans take work. The
/// readiness gates do not apply; `force` overrides a different assignee.
/// `[policy]` rules on the `claim` transition still enforce: assignment is
/// a deliberate act, but it performs the same transition.
pub fn assign(
    graph: &mut Graph,
    id: &str,
    name: &str,
    kind: IdentityKind,
    force: bool,
    now: DateTime<Utc>,
) -> Result<bool> {
    let name = name.trim();
    if name.is_empty() {
        return Err(OrbitError::invalid_argument("assignee must not be empty"));
    }
    let resolved = graph.resolve(id)?;
    let before = graph
        .tasks
        .get(&resolved)
        .cloned()
        .ok_or_else(|| OrbitError::not_found(id))?;
    let workflow = graph.workflow.clone();
    let changed = graph
        .get_mut(&resolved)?
        .assign(name, kind, force, now, &workflow)?
        == Outcome::Changed;
    if changed {
        let status = graph.get(&resolved)?.front.status.clone();
        crate::policy::enforce(
            graph,
            &resolved,
            crate::workflow::TRANSITION_CLAIM,
            Some(status.as_str()),
            &before,
            now,
        )?;
    }
    Ok(changed)
}

/// Extends the holder's Lease. Changes only Lease fields.
pub fn heartbeat(
    graph: &mut Graph,
    board: &Board,
    id: &str,
    by: &str,
    now: DateTime<Utc>,
) -> Result<bool> {
    let workflow = graph.workflow.clone();
    graph
        .get_mut(id)?
        .heartbeat(by, lease(board), now, &workflow)?;
    Ok(true)
}

/// Releases a Claim. An unclaimed Task is unchanged (`Ok(false)`).
/// `[policy]` rules on the `release` transition enforce.
pub fn release(
    graph: &mut Graph,
    id: &str,
    by: Option<&str>,
    force: bool,
    now: DateTime<Utc>,
) -> Result<bool> {
    let resolved = graph.resolve(id)?;
    let before = graph
        .tasks
        .get(&resolved)
        .cloned()
        .ok_or_else(|| OrbitError::not_found(id))?;
    let workflow = graph.workflow.clone();
    let changed = graph
        .get_mut(&resolved)?
        .release(by, force, now, &workflow)?
        == Outcome::Changed;
    if changed {
        let status = graph.get(&resolved)?.front.status.clone();
        crate::policy::enforce(
            graph,
            &resolved,
            crate::workflow::TRANSITION_RELEASE,
            Some(status.as_str()),
            &before,
            now,
        )?;
    }
    Ok(changed)
}

pub struct FinishOutcome {
    /// False when the Task was already in the target state.
    pub changed: bool,
    /// Open SubTasks cancelled because `force` was given.
    pub cancelled_subtasks: Vec<String>,
}

/// Moves a Task to Done or Cancelled through the workflow's closing
/// transitions, cancelling open SubTasks when forced.
pub fn finish(
    graph: &mut Graph,
    id: &str,
    by: Option<&str>,
    force: bool,
    kind: TerminalKind,
    now: DateTime<Utc>,
) -> Result<FinishOutcome> {
    // Policy snapshots and rollbacks key off the immutable internal ID.
    let resolved = graph.resolve(id)?;
    let id = resolved.as_str();
    let open_descendants = graph.open_descendants(id);
    if !open_descendants.is_empty() && !force {
        return Err(OrbitError::new(
            code::SUBTASKS_OPEN,
            format!("{id} has open SubTasks: {}", open_descendants.join(", ")),
        )
        .with_details(serde_json::json!({"task": id, "subtasks": open_descendants})));
    }
    let blocking = graph.blocking_ids(graph.get(id)?);
    if matches!(kind, TerminalKind::Done) && !blocking.is_empty() {
        return Err(TransitionError::DependencyBlocked {
            id: id.into(),
            blocking,
        }
        .into());
    }
    let workflow = graph.workflow.clone();
    let open: Vec<String> = graph
        .descendants(id)
        .into_iter()
        .filter(|descendant| {
            graph
                .get(descendant)
                .is_ok_and(|task| !workflow.is_terminal(&task.front.status))
        })
        .collect();
    // Snapshot every Task this finish can touch so a `[policy]` violation
    // rolls the whole in-memory change back.
    let snapshots = snapshot_tasks(graph, &open);
    let main_before = graph.tasks.get(id).cloned();
    for child in &open {
        graph.get_mut(child)?.cancel(by, true, now, &workflow)?;
    }
    let task = graph.get_mut(id)?;
    let outcome = match kind {
        TerminalKind::Done => task.complete(by, force, &blocking, &[], now, &workflow)?,
        TerminalKind::Cancelled => task.cancel(by, force, now, &workflow)?,
    };
    // Enforce `[policy]` on the closing transition of the target Task and
    // on each descendant's `cancel`; a violation restores every snapshot.
    let transition_name = match kind {
        TerminalKind::Done => crate::workflow::TRANSITION_DONE,
        TerminalKind::Cancelled => crate::workflow::TRANSITION_CANCEL,
    };
    let mut governed = vec![(id.to_string(), transition_name)];
    for child in &open {
        governed.push((child.clone(), crate::workflow::TRANSITION_CANCEL));
    }
    for (task_id, name) in governed {
        let before = if task_id == id {
            main_before.clone()
        } else {
            snapshots
                .iter()
                .find(|(snapshot_id, _)| snapshot_id == &task_id)
                .map(|(_, task)| task.clone())
        };
        let Some(before) = before else { continue };
        let after = match graph.tasks.get(&task_id) {
            Some(after) => after.clone(),
            None => continue,
        };
        if !transitioned(&before, &after) {
            continue;
        }
        let status = after.front.status.clone();
        if let Err(error) =
            crate::policy::enforce(graph, &task_id, name, Some(status.as_str()), &before, now)
        {
            // Roll back every Task this finish touched, not just the one
            // whose rule failed.
            if let Some(before) = &main_before {
                graph.tasks.insert(id.to_string(), before.clone());
            }
            restore_tasks(graph, snapshots.clone());
            return Err(error);
        }
    }
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

/// Runs one named workflow transition on a Task, e.g. `orbit transition
/// orb-… promote`. Closing transitions record the transition's resolution.
/// `[policy]` rules scoped to the transition name enforce here, so the CLI,
/// `orbit batch`, and MCP share them.
pub fn transition(
    graph: &mut Graph,
    id: &str,
    name: &str,
    caller: Option<&str>,
    force: bool,
    now: DateTime<Utc>,
) -> Result<Outcome> {
    let resolved = graph.resolve(id)?;
    let before = graph
        .tasks
        .get(&resolved)
        .cloned()
        .ok_or_else(|| OrbitError::not_found(id))?;
    let workflow = graph.workflow.clone();
    let outcome = graph
        .get_mut(&resolved)?
        .transition_to(name, caller, force, now, &workflow)?;
    if outcome == Outcome::Changed {
        let after = graph.get(&resolved)?;
        let status = after.front.status.clone();
        crate::policy::enforce(graph, &resolved, name, Some(status.as_str()), &before, now)?;
    }
    Ok(outcome)
}

/// Reopens a Task and, when needed, its completed ancestor Tasks. Returns the
/// reopened ancestors' IDs, nearest first. `[policy]` rules on the `reopen`
/// transition enforce for the Task and each ancestor it reopens.
pub fn reopen(graph: &mut Graph, id: &str, force: bool, now: DateTime<Utc>) -> Result<Vec<String>> {
    let resolved = graph.resolve(id)?;
    let mut snapshots: Vec<(String, Task)> = Vec::new();
    let task = graph.get(&resolved)?.clone();
    let workflow = graph.workflow.clone();
    let child_dependents = done_dependents(graph, &resolved);
    let mut probe = task.clone();
    probe.reopen(force, &child_dependents, now, &workflow)?;
    snapshots.push((resolved.clone(), task.clone()));

    let mut reopened = Vec::new();
    for ancestor_id in graph.ancestors(&resolved) {
        match workflow.category_of(&graph.get(&ancestor_id)?.front.status) {
            Some(StatusCategory::Done) => {
                if let Some(before) = graph.tasks.get(&ancestor_id).cloned() {
                    snapshots.push((ancestor_id.clone(), before));
                }
                let ancestor_dependents = done_dependents(graph, &ancestor_id);
                graph
                    .get_mut(&ancestor_id)?
                    .reopen(force, &ancestor_dependents, now, &workflow)?;
                reopened.push(ancestor_id);
            }
            Some(StatusCategory::Canceled) => {
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
    graph
        .get_mut(&resolved)?
        .reopen(force, &child_dependents, now, &workflow)?;

    // Enforce `[policy]` on every Task the reopen touched; a violation
    // restores them all.
    for (task_id, before) in &snapshots {
        let after = match graph.tasks.get(task_id) {
            Some(after) => after.clone(),
            None => continue,
        };
        if !transitioned(before, &after) {
            continue;
        }
        let status = after.front.status.clone();
        if let Err(error) = crate::policy::enforce(
            graph,
            task_id,
            crate::workflow::TRANSITION_REOPEN,
            Some(status.as_str()),
            before,
            now,
        ) {
            restore_tasks(graph, snapshots.clone());
            return Err(error);
        }
    }
    Ok(reopened)
}

fn done_dependents(graph: &Graph, id: &str) -> Vec<String> {
    graph
        .blocking_dependents(id)
        .iter()
        .filter(|dependent| {
            graph.workflow.category_of(&dependent.front.status) == Some(StatusCategory::Done)
        })
        .map(|dependent| dependent.id.clone())
        .collect()
}

use std::collections::{BTreeMap, HashMap, HashSet};

use chrono::Utc;
use serde::Serialize;

use crate::board::Board;
use crate::comment::Comment;
use crate::error::{OrbitError, Result, code};
use crate::hierarchy::Hierarchy;
use crate::task::{self, Status, Task};

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct Issue {
    pub code: String,
    pub severity: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub task: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub repository: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub issue: Option<String>,
    pub message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cause: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub next_action: Option<String>,
}

impl Issue {
    pub fn new(code: &str, task: Option<&str>, message: impl Into<String>) -> Self {
        Self {
            code: code.to_string(),
            severity: "error".to_string(),
            task: task.map(str::to_string),
            repository: None,
            issue: None,
            message: message.into(),
            cause: None,
            next_action: None,
        }
    }

    pub fn warning(code: &str, message: impl Into<String>) -> Self {
        Self {
            severity: "warning".to_string(),
            ..Self::new(code, None, message)
        }
    }

    pub fn with_task(mut self, task: impl Into<String>) -> Self {
        self.task = Some(task.into());
        self
    }

    pub fn with_repository(mut self, repository: impl Into<String>) -> Self {
        self.repository = Some(repository.into());
        self
    }

    pub fn with_issue(mut self, issue: impl Into<String>) -> Self {
        self.issue = Some(issue.into());
        self
    }

    pub fn with_cause(mut self, cause: impl Into<String>) -> Self {
        self.cause = Some(cause.into());
        self
    }

    pub fn with_next_action(mut self, next_action: impl Into<String>) -> Self {
        self.next_action = Some(next_action.into());
        self
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LoadMode {
    Strict,
    Lenient,
}

#[derive(Debug, Default, Serialize)]
pub struct Counts {
    pub total: usize,
    pub todo: usize,
    pub in_progress: usize,
    pub done: usize,
    pub cancelled: usize,
    pub ready: usize,
    pub stale: usize,
    pub waiting: usize,
    pub blocked: usize,
}

pub struct Graph {
    pub tasks: HashMap<String, Task>,
    non_ready_labels: Vec<String>,
    /// The Board's Issue model: type vocabulary, required fields, and the
    /// ordered parent hierarchy (ADR 0022).
    pub hierarchy: Hierarchy,
    /// Ordered Priority names from `board.priorities`, most important first.
    /// Tasks sort by the name's ordinal; a name outside the list sorts last.
    pub priorities: Vec<String>,
}

impl Graph {
    pub fn from_tasks(tasks: Vec<Task>) -> Self {
        Self::from_tasks_with_priorities(tasks, task::default_priorities())
    }

    pub fn from_tasks_with_priorities(tasks: Vec<Task>, priorities: Vec<String>) -> Self {
        Self {
            tasks: tasks
                .into_iter()
                .map(|task| (task.id.clone(), task))
                .collect(),
            non_ready_labels: Vec::new(),
            hierarchy: Hierarchy::default(),
            priorities,
        }
    }

    pub fn from_tasks_with_holds(tasks: Vec<Task>, non_ready_labels: Vec<String>) -> Self {
        Self {
            tasks: tasks
                .into_iter()
                .map(|task| (task.id.clone(), task))
                .collect(),
            non_ready_labels,
            hierarchy: Hierarchy::default(),
            priorities: task::default_priorities(),
        }
    }

    /// A Priority's ordinal in the Board's ordered list: lower sorts first.
    /// A name the Board no longer configures sorts after every known name.
    pub fn priority_ordinal(&self, priority: &str) -> usize {
        self.priorities
            .iter()
            .position(|name| name == priority)
            .unwrap_or(self.priorities.len())
    }

    pub fn load(board: &Board, mode: LoadMode) -> Result<(Self, Vec<Issue>)> {
        let mut tasks = HashMap::new();
        let mut issues = Vec::new();
        let ids = board.task_ids()?;
        let prefix_mismatch = detect_prefix_mismatch(&ids, &board.config.id);
        let accept_observed_prefix = if let Some(mismatch) = prefix_mismatch {
            let message = format!(
                "board prefix is `{}` but {} task files use `{}`; restore `id.prefix` before running other commands",
                mismatch.configured, mismatch.tasks, mismatch.observed
            );
            if mode == LoadMode::Strict {
                return Err(
                    OrbitError::new(code::PREFIX_MISMATCH, message).with_details(
                        serde_json::json!({
                            "configured": mismatch.configured,
                            "observed": mismatch.observed,
                            "tasks": mismatch.tasks,
                        }),
                    ),
                );
            }
            issues.push(Issue::new(code::PREFIX_MISMATCH, None, message));
            true
        } else {
            false
        };

        for id in ids {
            // With one PREFIX_MISMATCH reported, the files are known to use the
            // observed prefix, so accept their IDs instead of one CORRUPT_TASK
            // per file. Structural validation still runs below.
            if !accept_observed_prefix && !board.config.id.is_valid_task_id(&id) {
                let issue = Issue::new(
                    code::CORRUPT_TASK,
                    Some(&id),
                    format!("filename {id}.md is not a valid task id"),
                );
                if mode == LoadMode::Strict {
                    return Err(OrbitError::corrupt_task(issue.message));
                }
                issues.push(issue);
                continue;
            }

            let path = board.task_path(&id);
            let content = match std::fs::read_to_string(&path) {
                Ok(content) => content,
                Err(error) => {
                    let error = OrbitError::io(format!("read {}", path.display()), error);
                    if mode == LoadMode::Strict {
                        return Err(error);
                    }
                    issues.push(Issue::new(error.code, Some(&id), error.message));
                    continue;
                }
            };

            match task::parse_task(id.clone(), &content) {
                Ok(parsed) => {
                    tasks.insert(id, parsed);
                }
                Err(error) => {
                    if mode == LoadMode::Strict {
                        return Err(error);
                    }
                    issues.push(Issue::new(error.code, Some(&id), error.message));
                }
            }
        }

        let graph = Self {
            tasks,
            non_ready_labels: board.config.board.non_ready_labels.clone(),
            hierarchy: Hierarchy::from_config(&board.config.board)?,
            priorities: board.config.board.priorities.clone(),
        };
        let validation = graph.validate();
        if mode == LoadMode::Strict && !validation.is_empty() {
            return Err(OrbitError::board_corrupt(
                "the board has integrity issues; run `orbit doctor`",
                serde_json::json!({ "issues": validation }),
            ));
        }
        issues.extend(validation);

        Ok((graph, issues))
    }

    pub fn get(&self, id: &str) -> Result<&Task> {
        self.tasks.get(id).ok_or_else(|| OrbitError::not_found(id))
    }

    pub fn iter(&self) -> impl Iterator<Item = &Task> {
        self.tasks.values()
    }

    pub fn get_mut(&mut self, id: &str) -> Result<&mut Task> {
        self.tasks
            .get_mut(id)
            .ok_or_else(|| OrbitError::not_found(id))
    }

    /// Inserts a Task keyed by its own id so the map cannot be desynced.
    pub fn insert(&mut self, task: Task) {
        self.tasks.insert(task.id.clone(), task);
    }

    /// Removes and returns the Task, if present.
    pub fn remove(&mut self, id: &str) -> Option<Task> {
        self.tasks.remove(id)
    }

    pub fn validate(&self) -> Vec<Issue> {
        let mut issues = Vec::new();

        for task in self.tasks.values() {
            for link in &task.front.links {
                let Some(target) = link.target_task() else {
                    continue;
                };
                if target == task.id {
                    issues.push(Issue::new(
                        code::CYCLE_DETECTED,
                        Some(&task.id),
                        format!("{} has a {} link to itself", task.id, link.kind),
                    ));
                    continue;
                }

                if !self.tasks.contains_key(target) {
                    issues.push(Issue::new(
                        code::DANGLING_DEP,
                        Some(&task.id),
                        format!(
                            "{} has a {} link to missing task {target}",
                            task.id, link.kind
                        ),
                    ));
                }
            }

            if !self.hierarchy.is_known_type(&task.front.task_type) {
                issues.push(Issue::new(
                    code::INVALID_TYPE,
                    Some(&task.id),
                    format!(
                        "{} has unknown type `{}`; the Board configures: {}",
                        task.id,
                        task.front.task_type,
                        self.hierarchy.type_names().join(", ")
                    ),
                ));
            }
            let missing = self.hierarchy.missing_required_fields(task);
            if !missing.is_empty() {
                issues.push(Issue::new(
                    code::MISSING_REQUIRED_FIELD,
                    Some(&task.id),
                    format!(
                        "{} is missing required field(s): {}",
                        task.id,
                        missing.join(", ")
                    ),
                ));
            }

            if let Some(parent_id) = &task.front.parent {
                // Self-parents and longer parent cycles are reported once by
                // `find_parent_cycle` below.
                if parent_id == &task.id {
                    // covered by find_parent_cycle
                } else if !self.tasks.contains_key(parent_id) {
                    issues.push(Issue::new(
                        code::DANGLING_PARENT,
                        Some(&task.id),
                        format!("{} has missing parent {parent_id}", task.id),
                    ));
                } else {
                    let parent_type = &self.tasks[parent_id].front.task_type;
                    if let Some(reason) = self
                        .hierarchy
                        .parent_rule_violation(&task.front.task_type, parent_type)
                    {
                        issues.push(Issue::new(
                            code::INVALID_TYPE,
                            Some(&task.id),
                            format!("{} has parent {parent_id}: {reason}", task.id),
                        ));
                    }
                }
            }
        }

        if let Some(cycle) = self.find_parent_cycle() {
            issues.push(Issue::new(
                code::PARENT_CYCLE,
                cycle.first().map(String::as_str),
                format!("parent cycle: {}", cycle.join(" -> ")),
            ));
        }

        if let Some(cycle) = self.find_cycle() {
            issues.push(Issue::new(
                code::CYCLE_DETECTED,
                cycle.first().map(String::as_str),
                format!("dependency cycle: {}", cycle.join(" -> ")),
            ));
        }

        issues
    }

    /// The Task carrying `key` as its Source Key. Two carriers are an error,
    /// because picking one would silently hide the other.
    pub fn find_by_source_key(&self, key: &str) -> Result<Option<&Task>> {
        let mut found = self
            .tasks
            .values()
            .filter(|task| task.front.source_key.as_deref() == Some(key));
        let first = found.next();
        if let Some(second) = found.next() {
            return Err(OrbitError::new(
                code::DUPLICATE_SOURCE_KEY,
                format!(
                    "source key {key} is on more than one task: {}, {}",
                    first.map(|task| task.id.as_str()).unwrap_or_default(),
                    second.id
                ),
            ));
        }
        Ok(first)
    }

    pub fn is_ready(&self, task: &Task) -> bool {
        task.front.status == Status::Todo
            && task.front.claim.is_none()
            && self.hold_labels(task).is_empty()
            && self.blocking_ids(task).is_empty()
            && self.open_descendants(&task.id).is_empty()
    }

    pub fn hold_labels(&self, task: &Task) -> Vec<String> {
        task.front
            .labels
            .iter()
            .filter(|label| self.non_ready_labels.iter().any(|hold| hold == *label))
            .cloned()
            .collect()
    }

    /// Blocking links that still hold the Task back: internal targets that
    /// are missing or not terminal (fail-closed), plus every blocking
    /// external reference. External blockers are labelled `system:id`.
    pub fn blocking_links<'a>(&self, task: &'a Task) -> Vec<&'a crate::task::Link> {
        task.blocking_links()
            .filter(|link| match link.target_task() {
                Some(target) => match self.tasks.get(target) {
                    Some(dependency) => !dependency.front.status.is_terminal(),
                    None => true,
                },
                None => true,
            })
            .collect()
    }

    pub fn blocking_ids(&self, task: &Task) -> Vec<String> {
        self.blocking_links(task)
            .iter()
            .map(|link| link.target.label())
            .collect()
    }

    pub fn blocked_by(&self, task: &Task) -> Vec<String> {
        self.blocking_links(task)
            .iter()
            .map(|link| match link.target_task() {
                Some(target) => match self.tasks.get(target) {
                    Some(dependency) => {
                        format!("{target} ({})", dependency.front.status.as_str())
                    }
                    None => format!("{target} (missing)"),
                },
                None => format!("{} (external)", link.target.label()),
            })
            .collect()
    }

    /// Tasks holding any link that targets `id`, whatever its type.
    pub fn dependents(&self, id: &str) -> Vec<&Task> {
        let mut dependents: Vec<&Task> = self
            .tasks
            .values()
            .filter(|task| {
                task.front
                    .links
                    .iter()
                    .any(|link| link.target_task() == Some(id))
            })
            .collect();
        dependents.sort_by(|left, right| left.id.cmp(&right.id));
        dependents
    }

    /// Tasks whose *blocking* link targets `id` — the ones `id` actually
    /// holds back.
    pub fn blocking_dependents(&self, id: &str) -> Vec<&Task> {
        self.dependents(id)
            .into_iter()
            .filter(|task| {
                task.front
                    .links
                    .iter()
                    .any(|link| link.is_blocking() && link.target_task() == Some(id))
            })
            .collect()
    }

    pub fn children(&self, id: &str) -> Vec<&Task> {
        let mut children: Vec<&Task> = self
            .tasks
            .values()
            .filter(|task| task.front.parent.as_deref() == Some(id))
            .collect();
        children.sort_by(|left, right| left.id.cmp(&right.id));
        children
    }

    pub fn open_children(&self, id: &str) -> Vec<String> {
        self.children(id)
            .into_iter()
            .filter(|task| !task.front.status.is_terminal())
            .map(|task| task.id.clone())
            .collect()
    }

    /// Every non-terminal descendant of `id`, at any depth: a Parent Task is
    /// ready or closable only once its whole subtree is resolved (ADR 0022).
    pub fn open_descendants(&self, id: &str) -> Vec<String> {
        self.descendants(id)
            .into_iter()
            .filter(|descendant| {
                self.tasks
                    .get(descendant)
                    .is_some_and(|task| !task.front.status.is_terminal())
            })
            .collect()
    }

    /// Number of `parent` hops from a top-level Task: 0 for a Task with no
    /// parent, 1 for its SubTask, and so on. A missing Task or a parent cycle
    /// reports the hops walked before stopping.
    pub fn depth(&self, id: &str) -> usize {
        self.ancestors(id).len()
    }

    /// Every ancestor of `id`, nearest parent first. Stops at a missing Task
    /// or a parent cycle.
    pub fn ancestors(&self, id: &str) -> Vec<String> {
        let mut ancestors = Vec::new();
        let mut seen = HashSet::from([id.to_string()]);
        let mut current = id;
        while let Some(task) = self.tasks.get(current) {
            let Some(parent) = task.front.parent.as_deref() else {
                break;
            };
            if !seen.insert(parent.to_string()) {
                break;
            }
            ancestors.push(parent.to_string());
            current = parent;
        }
        ancestors
    }

    /// Every descendant of `id` in depth-first order: each SubTask is listed
    /// before its own SubTasks, and siblings keep `children` order.
    pub fn descendants(&self, id: &str) -> Vec<String> {
        fn visit(graph: &Graph, id: &str, seen: &mut HashSet<String>, out: &mut Vec<String>) {
            for child in graph.children(id) {
                if seen.insert(child.id.clone()) {
                    out.push(child.id.clone());
                    visit(graph, &child.id, seen, out);
                }
            }
        }
        let mut seen = HashSet::from([id.to_string()]);
        let mut descendants = Vec::new();
        visit(self, id, &mut seen, &mut descendants);
        descendants
    }

    /// A Parent Task is closable once it has descendants and every one of
    /// them — at any depth — is terminal.
    pub fn eligible_for_close(&self, task: &Task) -> bool {
        if task.front.status.is_terminal() {
            return false;
        }
        let descendants = self.descendants(&task.id);
        !descendants.is_empty()
            && descendants.iter().all(|descendant| {
                self.tasks
                    .get(descendant)
                    .is_some_and(|task| task.front.status.is_terminal())
            })
    }

    pub fn close_candidates(&self) -> Vec<&Task> {
        let mut candidates: Vec<&Task> = self
            .tasks
            .values()
            .filter(|task| self.eligible_for_close(task))
            .collect();
        candidates.sort_by(|left, right| {
            (
                self.priority_ordinal(&left.front.priority),
                left.front.created_at,
                left.id.as_str(),
            )
                .cmp(&(
                    self.priority_ordinal(&right.front.priority),
                    right.front.created_at,
                    right.id.as_str(),
                ))
        });
        candidates
    }

    pub fn ready_tasks(&self) -> Vec<Task> {
        let mut ready: Vec<Task> = self
            .tasks
            .values()
            .filter(|task| self.is_ready(task))
            .cloned()
            .collect();
        ready.sort_by(|left, right| {
            (
                self.priority_ordinal(&left.front.priority),
                left.front.created_at,
                left.id.as_str(),
            )
                .cmp(&(
                    self.priority_ordinal(&right.front.priority),
                    right.front.created_at,
                    right.id.as_str(),
                ))
        });
        ready
    }

    pub fn stale_tasks(&self) -> Vec<&Task> {
        let now = Utc::now();
        let mut stale: Vec<&Task> = self
            .tasks
            .values()
            .filter(|task| {
                task.front.status == Status::InProgress
                    && task
                        .front
                        .claim
                        .as_ref()
                        .is_some_and(|claim| claim.is_stale(now))
            })
            .collect();
        stale.sort_by(|left, right| left.id.cmp(&right.id));
        stale
    }

    pub fn would_create_cycle(&self, task_id: &str, dep_id: &str) -> Option<Vec<String>> {
        if task_id == dep_id {
            return Some(vec![task_id.to_string(), dep_id.to_string()]);
        }

        let mut visited = HashSet::new();
        let mut path = Vec::new();
        if self.path_to(dep_id, task_id, &mut visited, &mut path) {
            let mut cycle = vec![task_id.to_string()];
            cycle.extend(path);
            Some(cycle)
        } else {
            None
        }
    }

    /// The first `parent` cycle found, as the path of Task IDs that loops.
    /// Each Task has at most one parent, so following the chain from any
    /// member reaches the cycle; IDs are visited in sorted order for a
    /// deterministic report.
    pub fn find_parent_cycle(&self) -> Option<Vec<String>> {
        let mut ids: Vec<&String> = self.tasks.keys().collect();
        ids.sort();
        for start in ids {
            let mut position: HashMap<String, usize> = HashMap::new();
            let mut path: Vec<String> = Vec::new();
            let mut current = start.clone();
            loop {
                if let Some(&pos) = position.get(&current) {
                    let mut cycle = path[pos..].to_vec();
                    cycle.push(current.clone());
                    return Some(cycle);
                }
                let Some(task) = self.tasks.get(&current) else {
                    break;
                };
                let Some(parent) = task.front.parent.clone() else {
                    break;
                };
                position.insert(current.clone(), path.len());
                path.push(current.clone());
                current = parent;
            }
        }
        None
    }

    pub fn find_cycle(&self) -> Option<Vec<String>> {
        let mut state: HashMap<String, u8> = HashMap::new();
        let mut stack = Vec::new();

        for id in self.tasks.keys() {
            if state.get(id).copied().unwrap_or(0) == 0
                && let Some(cycle) = self.dfs_cycle(id, &mut state, &mut stack)
            {
                return Some(cycle);
            }
        }
        None
    }

    pub fn counts(&self) -> Counts {
        self.counts_where(|_| true)
    }

    pub fn counts_where(&self, matches: impl Fn(&Task) -> bool) -> Counts {
        let now = Utc::now();
        let mut counts = Counts::default();

        for task in self.tasks.values() {
            if !matches(task) {
                continue;
            }

            counts.total += 1;
            match task.front.status {
                Status::Todo => counts.todo += 1,
                Status::InProgress => counts.in_progress += 1,
                Status::Done => counts.done += 1,
                Status::Cancelled => counts.cancelled += 1,
            }

            if self.is_ready(task) {
                counts.ready += 1;
            }
            if task.front.status == Status::InProgress
                && task
                    .front
                    .claim
                    .as_ref()
                    .is_some_and(|claim| claim.is_stale(now))
            {
                counts.stale += 1;
            }
            if !task.front.status.is_terminal() && task.external_blockers().next().is_some() {
                counts.waiting += 1;
            }
            if !task.front.status.is_terminal()
                && (!self.blocking_ids(task).is_empty()
                    || !self.open_descendants(&task.id).is_empty())
            {
                counts.blocked += 1;
            }
        }

        counts
    }

    fn path_to(
        &self,
        from: &str,
        target: &str,
        visited: &mut HashSet<String>,
        path: &mut Vec<String>,
    ) -> bool {
        if from == target {
            path.push(from.to_string());
            return true;
        }
        if !visited.insert(from.to_string()) {
            return false;
        }

        path.push(from.to_string());
        if let Some(task) = self.tasks.get(from) {
            for dep in blocking_link_targets(task) {
                if self.path_to(dep, target, visited, path) {
                    return true;
                }
            }
        }
        path.pop();
        false
    }

    fn dfs_cycle(
        &self,
        id: &str,
        state: &mut HashMap<String, u8>,
        stack: &mut Vec<String>,
    ) -> Option<Vec<String>> {
        state.insert(id.to_string(), 1);
        stack.push(id.to_string());

        if let Some(task) = self.tasks.get(id) {
            for dep in blocking_link_targets(task) {
                if !self.tasks.contains_key(dep) {
                    continue;
                }
                match state.get(dep).copied().unwrap_or(0) {
                    0 => {
                        if let Some(cycle) = self.dfs_cycle(dep, state, stack) {
                            return Some(cycle);
                        }
                    }
                    1 => {
                        if let Some(position) = stack.iter().position(|node| node.as_str() == dep) {
                            let mut cycle = stack[position..].to_vec();
                            cycle.push(dep.to_string());
                            return Some(cycle);
                        }
                    }
                    _ => {}
                }
            }
        }

        stack.pop();
        state.insert(id.to_string(), 2);
        None
    }
}

/// Internal targets of a Task's blocking links — the edges cycle detection
/// and dependency walks follow. Non-blocking link types never participate.
fn blocking_link_targets(task: &Task) -> Vec<&str> {
    task.blocking_links()
        .filter_map(|link| link.target_task())
        .collect()
}

#[derive(Debug)]
struct PrefixMismatch {
    configured: String,
    observed: String,
    tasks: usize,
}

fn detect_prefix_mismatch(
    ids: &[String],
    config: &crate::board::IdConfig,
) -> Option<PrefixMismatch> {
    if ids.is_empty() || ids.iter().any(|id| config.is_valid_task_id(id)) {
        return None;
    }

    let mut counts: BTreeMap<String, usize> = BTreeMap::new();
    for id in ids {
        if let Some(prefix) = inferred_prefix(id) {
            *counts.entry(prefix).or_default() += 1;
        }
    }

    let (observed, tasks) = counts
        .into_iter()
        .max_by(|left, right| left.1.cmp(&right.1).then_with(|| right.0.cmp(&left.0)))?;
    Some(PrefixMismatch {
        configured: config.prefix.clone(),
        observed,
        tasks,
    })
}

fn inferred_prefix(id: &str) -> Option<String> {
    let parent = id.split('.').next()?;
    let (prefix, suffix) = parent.rsplit_once('-')?;
    if prefix.is_empty() || !(6..=8).contains(&suffix.len()) {
        return None;
    }
    if !suffix.bytes().all(|byte| task::ID_ALPHABET.contains(&byte)) {
        return None;
    }
    Some(format!("{prefix}-"))
}

pub fn task_summary(graph: &Graph, task: &Task) -> serde_json::Value {
    // `subtasks` covers the whole subtree at any depth; `children` remains
    // the direct-children view used by `tree`, `delete`, and grouping.
    let subtasks: Vec<String> = graph.descendants(&task.id);
    let open_subtasks = graph.open_descendants(&task.id);
    let subtask_count = subtasks.len();
    let open_subtask_count = open_subtasks.len();
    let resolved_subtask_count = subtask_count.saturating_sub(open_subtask_count);
    let eligible_for_close = graph.eligible_for_close(task);
    // Derived `waiting_on` view over external blocking links.
    let waiting_labels = task.waiting_labels();
    let waiting_on = if waiting_labels.is_empty() {
        serde_json::Value::Null
    } else {
        serde_json::Value::String(waiting_labels.join(", "))
    };

    serde_json::json!({
        "id": task.id,
        "title": task.front.title,
        "status": task.front.status,
        "type": task.front.task_type,
        "priority": task.front.priority,
        "kind": task.kind(),
        "labels": task.front.labels,
        "links": task.front.links.iter().map(crate::task::Link::to_json).collect::<Vec<_>>(),
        // Derived read-only views kept for consumers of the old fields;
        // `links` is the stored truth.
        "depends_on": task
            .blocking_links()
            .filter_map(|link| link.target_task().map(str::to_string))
            .collect::<Vec<_>>(),
        "parent": task.front.parent,
        "depth": graph.depth(&task.id),
        "subtasks": subtasks,
        "open_subtasks": open_subtasks,
        "subtask_count": subtask_count,
        "resolved_subtask_count": resolved_subtask_count,
        "open_subtask_count": open_subtask_count,
        "eligible_for_close": eligible_for_close,
        "waiting_on": waiting_on,
        "checkpoint": task.front.checkpoint,
        "checkpoint_archived": task
            .front
            .checkpoint
            .as_ref()
            .map(|_| task.front.status.is_terminal()),
        "area": task.front.area.as_deref(),
        "verification": task.front.verification,
        "source_key": task.front.source_key,
        "annotations": task.front.annotations,
        "github": crate::remote::link::github_compat_json(task),
        "remote": crate::remote::link::link_json(task),
        "claimed_by": task.holder(),
        "claimed_at": task.claimed_at(),
        "lease_until": task.lease_until(),
        "created_at": task.front.created_at,
        "updated_at": task.front.updated_at,
        "ready": graph.is_ready(task),
        "blocking": graph.blocking_ids(task),
    })
}

pub fn task_detail(graph: &Graph, task: &Task) -> serde_json::Value {
    let mut value = task_summary(graph, task);
    value["body"] = serde_json::json!(task.body);
    value["dependents"] = serde_json::json!(
        graph
            .dependents(&task.id)
            .iter()
            .map(|dependent| dependent.id.clone())
            .collect::<Vec<_>>()
    );
    value
}

pub fn task_detail_with_comments(
    graph: &Graph,
    task: &Task,
    comments: &[Comment],
) -> serde_json::Value {
    let mut value = task_detail(graph, task);
    value["comments"] = serde_json::to_value(comments).unwrap_or_else(|_| serde_json::json!([]));
    value["comment_count"] = serde_json::json!(comments.len());
    value
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::task::{TaskFrontmatter, default_priority};
    use chrono::TimeZone;
    use std::collections::BTreeMap;

    fn task(id: &str, status: Status, deps: &[&str]) -> Task {
        let now = Utc.with_ymd_and_hms(2026, 10, 23, 12, 0, 0).unwrap();
        Task {
            id: id.to_string(),
            front: TaskFrontmatter {
                title: id.to_string(),
                status,
                task_type: "task".to_string(),
                priority: default_priority(),
                labels: Vec::new(),
                links: deps
                    .iter()
                    .map(|dep| crate::task::Link::dependency(dep))
                    .collect(),
                parent: None,
                checkpoint: None,
                area: None,
                verification: None,
                github: None,
                remote: None,
                repo: None,
                claim: None,
                stray_claim_times: None,
                created_at: now,
                updated_at: now,
                source_key: None,
                annotations: BTreeMap::new(),
                extra: BTreeMap::new(),
            },
            body: String::new(),
        }
    }

    #[test]
    fn detects_cycles() {
        let graph = Graph::from_tasks(vec![
            task("orb-aaaaaa", Status::Todo, &["orb-bbbbbb"]),
            task("orb-bbbbbb", Status::Todo, &["orb-cccccc"]),
            task("orb-cccccc", Status::Todo, &["orb-aaaaaa"]),
        ]);

        let cycle = graph.find_cycle().unwrap();
        assert_eq!(cycle.first(), cycle.last());
    }

    #[test]
    fn readiness_requires_terminal_dependencies() {
        let graph = Graph::from_tasks(vec![
            task("orb-aaaaaa", Status::Todo, &[]),
            task("orb-bbbbbb", Status::Todo, &["orb-aaaaaa"]),
        ]);

        assert!(graph.is_ready(graph.get("orb-aaaaaa").unwrap()));
        assert!(!graph.is_ready(graph.get("orb-bbbbbb").unwrap()));
        assert_eq!(
            graph.blocked_by(graph.get("orb-bbbbbb").unwrap()),
            vec!["orb-aaaaaa (todo)".to_string()]
        );
    }

    #[test]
    fn parent_waits_for_open_subtasks() {
        let mut child = task("orb-bbbbbb", Status::Todo, &[]);
        child.front.parent = Some("orb-aaaaaa".to_string());
        let graph = Graph::from_tasks(vec![task("orb-aaaaaa", Status::Todo, &[]), child]);

        let parent = graph.get("orb-aaaaaa").unwrap();
        assert!(!graph.is_ready(parent));
        assert_eq!(graph.open_children("orb-aaaaaa"), vec!["orb-bbbbbb"]);
        assert_eq!(
            graph
                .children("orb-aaaaaa")
                .iter()
                .map(|task| task.id.as_str())
                .collect::<Vec<_>>(),
            vec!["orb-bbbbbb"]
        );

        let mut done_child = task("orb-bbbbbb", Status::Done, &[]);
        done_child.front.parent = Some("orb-aaaaaa".to_string());
        let graph = Graph::from_tasks(vec![task("orb-aaaaaa", Status::Todo, &[]), done_child]);
        assert!(graph.is_ready(graph.get("orb-aaaaaa").unwrap()));
    }

    #[test]
    fn missing_parent_and_disallowed_parent_types_are_issues() {
        let mut orphan = task("orb-aaaaaa", Status::Todo, &[]);
        orphan.front.parent = Some("orb-missing".to_string());
        let graph = Graph::from_tasks(vec![orphan]);
        assert!(
            graph
                .validate()
                .iter()
                .any(|issue| issue.code == code::DANGLING_PARENT)
        );

        // A `task` may not parent another `task`: both sit on the same
        // hierarchy level.
        let mut parent = task("orb-aaaaaa", Status::Todo, &[]);
        parent.front.parent = Some("orb-cccccc".to_string());
        let mut child = task("orb-bbbbbb", Status::Todo, &[]);
        child.front.parent = Some("orb-aaaaaa".to_string());
        let graph = Graph::from_tasks(vec![task("orb-cccccc", Status::Todo, &[]), parent, child]);
        assert!(
            graph
                .validate()
                .iter()
                .any(|issue| issue.code == code::INVALID_TYPE)
        );

        // An `epic` cannot sit under anything: it is the top level.
        let mut epic = task("orb-aaaaaa", Status::Todo, &[]);
        epic.front.task_type = "epic".to_string();
        epic.front.parent = Some("orb-cccccc".to_string());
        let graph = Graph::from_tasks(vec![task("orb-cccccc", Status::Todo, &[]), epic]);
        assert!(
            graph
                .validate()
                .iter()
                .any(|issue| issue.code == code::INVALID_TYPE)
        );
    }

    #[test]
    fn parent_cycles_are_reported_once() {
        let mut first = task("orb-aaaaaa", Status::Todo, &[]);
        first.front.task_type = "task".to_string();
        first.front.parent = Some("orb-bbbbbb".to_string());
        let mut second = task("orb-bbbbbb", Status::Todo, &[]);
        second.front.task_type = "sub-task".to_string();
        second.front.parent = Some("orb-aaaaaa".to_string());
        let graph = Graph::from_tasks(vec![first, second]);
        let issues = graph.validate();
        let cycles: Vec<_> = issues
            .iter()
            .filter(|issue| issue.code == code::PARENT_CYCLE)
            .collect();
        assert_eq!(cycles.len(), 1, "{cycles:?}");
    }

    #[test]
    fn hierarchy_permits_three_levels_and_rejects_a_fourth() {
        // epic > task > sub-task is the default three-level model.
        let mut epic = task("orb-aaaaaa", Status::Todo, &[]);
        epic.front.task_type = "epic".to_string();
        let mut middle = task("orb-bbbbbb", Status::Todo, &[]);
        middle.front.task_type = "task".to_string();
        middle.front.parent = Some("orb-aaaaaa".to_string());
        let mut leaf = task("orb-cccccc", Status::Todo, &[]);
        leaf.front.task_type = "sub-task".to_string();
        leaf.front.parent = Some("orb-bbbbbb".to_string());
        let graph = Graph::from_tasks(vec![epic, middle, leaf]);
        assert!(
            graph
                .validate()
                .iter()
                .all(|issue| issue.code != code::INVALID_TYPE),
            "{:?}",
            graph.validate()
        );
        assert_eq!(graph.depth("orb-cccccc"), 2);

        // A fourth level does not exist: `sub-task` cannot parent anything.
        let mut too_deep = task("orb-dddddd", Status::Todo, &[]);
        too_deep.front.task_type = "sub-task".to_string();
        too_deep.front.parent = Some("orb-cccccc".to_string());
        let mut graph = graph;
        graph.insert(too_deep);
        assert!(
            graph
                .validate()
                .iter()
                .any(|issue| issue.code == code::INVALID_TYPE)
        );
    }

    #[test]
    fn unknown_types_and_missing_required_fields_are_issues() {
        let mut weird = task("orb-aaaaaa", Status::Todo, &[]);
        weird.front.task_type = "mystery".to_string();
        let graph = Graph::from_tasks(vec![weird]);
        assert!(
            graph
                .validate()
                .iter()
                .any(|issue| issue.code == code::INVALID_TYPE)
        );

        let mut types = crate::hierarchy::default_issue_types();
        types
            .iter_mut()
            .find(|t| t.name == "epic")
            .unwrap()
            .required = vec!["area".to_string()];
        let levels = crate::hierarchy::default_hierarchy();
        let mut graph = Graph::from_tasks(vec![task("orb-aaaaaa", Status::Todo, &[])]);
        graph.hierarchy = Hierarchy::from_parts(types, "task".to_string(), levels).unwrap();
        graph.get_mut("orb-aaaaaa").unwrap().front.task_type = "epic".to_string();
        assert!(
            graph
                .validate()
                .iter()
                .any(|issue| issue.code == code::MISSING_REQUIRED_FIELD)
        );
    }

    #[test]
    fn readiness_waits_for_descendants_at_every_depth() {
        // epic > task > sub-task: the epic stays unready until the leaf
        // resolves, even though its direct child is already done.
        let mut epic = task("orb-aaaaaa", Status::Todo, &[]);
        epic.front.task_type = "epic".to_string();
        let mut middle = task("orb-bbbbbb", Status::Done, &[]);
        middle.front.task_type = "task".to_string();
        middle.front.parent = Some("orb-aaaaaa".to_string());
        let mut leaf = task("orb-cccccc", Status::Todo, &[]);
        leaf.front.task_type = "sub-task".to_string();
        leaf.front.parent = Some("orb-bbbbbb".to_string());
        let graph = Graph::from_tasks(vec![epic, middle, leaf]);

        let epic_task = graph.get("orb-aaaaaa").unwrap();
        assert!(!graph.is_ready(epic_task));
        assert_eq!(
            graph.open_descendants("orb-aaaaaa"),
            vec!["orb-cccccc".to_string()]
        );
        assert!(!graph.eligible_for_close(epic_task));

        let mut resolved_leaf = task("orb-cccccc", Status::Done, &[]);
        resolved_leaf.front.task_type = "sub-task".to_string();
        resolved_leaf.front.parent = Some("orb-bbbbbb".to_string());
        let mut epic = task("orb-aaaaaa", Status::Todo, &[]);
        epic.front.task_type = "epic".to_string();
        let mut middle = task("orb-bbbbbb", Status::Done, &[]);
        middle.front.task_type = "task".to_string();
        middle.front.parent = Some("orb-aaaaaa".to_string());
        let graph = Graph::from_tasks(vec![epic, middle, resolved_leaf]);

        let epic_task = graph.get("orb-aaaaaa").unwrap();
        assert!(graph.is_ready(epic_task));
        assert!(graph.eligible_for_close(epic_task));
    }

    fn child_of(mut task: Task, parent: &str) -> Task {
        task.front.parent = Some(parent.to_string());
        task
    }

    fn typed(mut task: Task, ty: &str) -> Task {
        task.front.task_type = ty.to_string();
        task
    }

    fn chain() -> Graph {
        Graph::from_tasks(vec![
            typed(task("orb-aaaaaa", Status::Todo, &[]), "epic"),
            typed(
                child_of(task("orb-bbbbbb", Status::Todo, &[]), "orb-aaaaaa"),
                "task",
            ),
            typed(
                child_of(task("orb-bbbbbb.cccccc", Status::Todo, &[]), "orb-bbbbbb"),
                "sub-task",
            ),
            child_of(task("orb-eeeeee", Status::Todo, &[]), "orb-bbbbbb.cccccc"),
            child_of(task("orb-fffffff", Status::Todo, &[]), "orb-aaaaaa"),
        ])
    }

    #[test]
    fn depth_counts_parent_hops() {
        let graph = chain();
        assert_eq!(graph.depth("orb-aaaaaa"), 0);
        assert_eq!(graph.depth("orb-bbbbbb"), 1);
        assert_eq!(graph.depth("orb-bbbbbb.cccccc"), 2);
        assert_eq!(graph.depth("orb-eeeeee"), 3);
        assert_eq!(graph.depth("orb-missing"), 0);
    }

    #[test]
    fn ancestors_walk_nearest_first() {
        let graph = chain();
        assert_eq!(
            graph.ancestors("orb-eeeeee"),
            vec![
                "orb-bbbbbb.cccccc".to_string(),
                "orb-bbbbbb".to_string(),
                "orb-aaaaaa".to_string()
            ]
        );
        assert_eq!(graph.ancestors("orb-aaaaaa"), Vec::<String>::new());
        assert_eq!(graph.ancestors("orb-missing"), Vec::<String>::new());
    }

    #[test]
    fn descendants_walk_depth_first() {
        let graph = chain();
        assert_eq!(
            graph.descendants("orb-aaaaaa"),
            vec![
                "orb-bbbbbb".to_string(),
                "orb-bbbbbb.cccccc".to_string(),
                "orb-eeeeee".to_string(),
                "orb-fffffff".to_string()
            ]
        );
        assert_eq!(
            graph.descendants("orb-bbbbbb.cccccc"),
            vec!["orb-eeeeee".to_string()]
        );
        assert_eq!(graph.descendants("orb-fffffff"), Vec::<String>::new());
    }

    #[test]
    fn depth_and_ancestors_survive_a_parent_cycle() {
        let mut first = task("orb-aaaaaa", Status::Todo, &[]);
        first.front.parent = Some("orb-bbbbbb".to_string());
        let mut second = task("orb-bbbbbb", Status::Todo, &[]);
        second.front.parent = Some("orb-aaaaaa".to_string());
        let graph = Graph::from_tasks(vec![first, second]);

        assert!(graph.depth("orb-aaaaaa") <= 2);
        assert!(graph.ancestors("orb-aaaaaa").len() <= 2);
        assert!(graph.descendants("orb-aaaaaa").len() <= 2);
    }

    #[test]
    fn missing_dependencies_fail_closed() {
        let graph = Graph::from_tasks(vec![task("orb-aaaaaa", Status::Todo, &["orb-missing"])]);
        assert!(!graph.is_ready(graph.get("orb-aaaaaa").unwrap()));
        assert!(
            graph
                .validate()
                .iter()
                .any(|issue| issue.code == code::DANGLING_DEP)
        );
    }

    #[test]
    fn hold_labels_block_readiness() {
        let mut held = task("orb-aaaaaa", Status::Todo, &[]);
        held.front.labels = vec!["needs-triage".to_string()];
        let graph = Graph::from_tasks_with_holds(vec![held], vec!["needs-triage".to_string()]);
        let held_task = graph.get("orb-aaaaaa").unwrap().clone();
        assert!(!graph.is_ready(&held_task));
        assert_eq!(graph.hold_labels(&held_task), vec!["needs-triage"]);
        assert!(graph.ready_tasks().is_empty());

        let graph = Graph::from_tasks_with_holds(vec![held_task], Vec::new());
        assert!(graph.is_ready(graph.get("orb-aaaaaa").unwrap()));
    }

    #[test]
    fn would_create_cycle_finds_the_path() {
        let graph = Graph::from_tasks(vec![
            task("orb-aaaaaa", Status::Todo, &[]),
            task("orb-bbbbbb", Status::Todo, &["orb-aaaaaa"]),
        ]);

        let cycle = graph
            .would_create_cycle("orb-aaaaaa", "orb-bbbbbb")
            .unwrap();
        assert_eq!(cycle, vec!["orb-aaaaaa", "orb-bbbbbb", "orb-aaaaaa"]);
        assert!(
            graph
                .would_create_cycle("orb-cccccc", "orb-aaaaaa")
                .is_none()
        );
    }

    #[test]
    fn ready_tasks_sort_by_priority_then_age() {
        let mut high = task("orb-aaaaaa", Status::Todo, &[]);
        high.front.priority = "highest".to_string();
        let mut low = task("orb-bbbbbb", Status::Todo, &[]);
        low.front.priority = "low".to_string();

        let graph = Graph::from_tasks(vec![low, high]);
        let ready = graph.ready_tasks();
        assert_eq!(ready[0].id, "orb-aaaaaa");
    }

    #[test]
    fn priority_ordinal_follows_the_board_order() {
        let mut graph = Graph::from_tasks(vec![]);
        graph.priorities = vec!["p0".to_string(), "p1".to_string(), "p2".to_string()];
        assert_eq!(graph.priority_ordinal("p0"), 0);
        assert_eq!(graph.priority_ordinal("p2"), 2);
        // A name the Board does not configure sorts after every known name.
        assert_eq!(graph.priority_ordinal("unknown"), 3);

        let mut first = task("orb-aaaaaa", Status::Todo, &[]);
        first.front.priority = "p2".to_string();
        let mut second = task("orb-bbbbbb", Status::Todo, &[]);
        second.front.priority = "p0".to_string();
        let graph = Graph::from_tasks_with_priorities(vec![first, second], graph.priorities);
        assert_eq!(graph.ready_tasks()[0].id, "orb-bbbbbb");
    }

    #[test]
    fn default_hierarchy_has_three_levels() {
        assert_eq!(Graph::from_tasks(vec![]).hierarchy.max_depth(), 3);
        assert_eq!(
            Graph::from_tasks_with_holds(vec![], vec![])
                .hierarchy
                .max_depth(),
            3
        );
    }
}

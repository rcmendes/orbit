use std::collections::{HashMap, HashSet};

use chrono::Utc;
use serde::Serialize;

use crate::board::Board;
use crate::comment::Comment;
use crate::error::{OrbitError, Result, code};
use crate::task::{self, Status, Task};

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct Issue {
    pub code: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub task: Option<String>,
    pub message: String,
}

impl Issue {
    pub fn new(code: &str, task: Option<&str>, message: impl Into<String>) -> Self {
        Self {
            code: code.to_string(),
            task: task.map(str::to_string),
            message: message.into(),
        }
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
}

impl Graph {
    pub fn from_tasks(tasks: Vec<Task>) -> Self {
        Self {
            tasks: tasks
                .into_iter()
                .map(|task| (task.id.clone(), task))
                .collect(),
            non_ready_labels: Vec::new(),
        }
    }

    pub fn from_tasks_with_holds(tasks: Vec<Task>, non_ready_labels: Vec<String>) -> Self {
        Self {
            tasks: tasks
                .into_iter()
                .map(|task| (task.id.clone(), task))
                .collect(),
            non_ready_labels,
        }
    }

    pub fn load(board: &Board, mode: LoadMode) -> Result<(Self, Vec<Issue>)> {
        let mut tasks = HashMap::new();
        let mut issues = Vec::new();

        for id in board.task_ids()? {
            if !task::is_valid_id(&id, &board.config.id.prefix) {
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

    pub fn get_mut(&mut self, id: &str) -> Result<&mut Task> {
        self.tasks
            .get_mut(id)
            .ok_or_else(|| OrbitError::not_found(id))
    }

    pub fn validate(&self) -> Vec<Issue> {
        let mut issues = Vec::new();

        for task in self.tasks.values() {
            for dep in &task.front.depends_on {
                if dep == &task.id {
                    issues.push(Issue::new(
                        code::CYCLE_DETECTED,
                        Some(&task.id),
                        format!("{} depends on itself", task.id),
                    ));
                    continue;
                }

                if !self.tasks.contains_key(dep) {
                    issues.push(Issue::new(
                        code::DANGLING_DEP,
                        Some(&task.id),
                        format!("{} depends on missing task {dep}", task.id),
                    ));
                }
            }

            if let Some(parent_id) = &task.front.parent {
                if parent_id == &task.id {
                    issues.push(Issue::new(
                        code::NESTED_SUBTASK,
                        Some(&task.id),
                        format!("{} is its own parent", task.id),
                    ));
                } else if !self.tasks.contains_key(parent_id) {
                    issues.push(Issue::new(
                        code::DANGLING_PARENT,
                        Some(&task.id),
                        format!("{} has missing parent {parent_id}", task.id),
                    ));
                } else if self
                    .tasks
                    .get(parent_id)
                    .is_some_and(|parent| parent.front.parent.is_some())
                {
                    issues.push(Issue::new(
                        code::NESTED_SUBTASK,
                        Some(&task.id),
                        format!(
                            "{} has parent {parent_id}, which is itself a SubTask; only one level is allowed",
                            task.id
                        ),
                    ));
                }
            }
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

    pub fn is_ready(&self, task: &Task) -> bool {
        task.front.status == Status::Todo
            && task.front.claim.is_none()
            && task.front.waiting_on.is_none()
            && self.hold_labels(task).is_empty()
            && self.blocking_ids(task).is_empty()
            && self.open_children(&task.id).is_empty()
    }

    pub fn hold_labels(&self, task: &Task) -> Vec<String> {
        task.front
            .labels
            .iter()
            .filter(|label| self.non_ready_labels.iter().any(|hold| hold == *label))
            .cloned()
            .collect()
    }

    pub fn blocking_ids(&self, task: &Task) -> Vec<String> {
        let mut blocking = Vec::new();
        for dep in &task.front.depends_on {
            match self.tasks.get(dep) {
                Some(dependency) if dependency.front.status.is_terminal() => {}
                _ => blocking.push(dep.clone()),
            }
        }
        blocking
    }

    pub fn blocked_by(&self, task: &Task) -> Vec<String> {
        self.blocking_ids(task)
            .into_iter()
            .map(|dep| match self.tasks.get(&dep) {
                Some(dependency) => format!("{dep} ({})", dependency.front.status.as_str()),
                None => format!("{dep} (missing)"),
            })
            .collect()
    }

    pub fn dependents(&self, id: &str) -> Vec<&Task> {
        let mut dependents: Vec<&Task> = self
            .tasks
            .values()
            .filter(|task| task.front.depends_on.iter().any(|dep| dep == id))
            .collect();
        dependents.sort_by(|left, right| left.id.cmp(&right.id));
        dependents
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

    pub fn ready_tasks(&self) -> Vec<Task> {
        let mut ready: Vec<Task> = self
            .tasks
            .values()
            .filter(|task| self.is_ready(task))
            .cloned()
            .collect();
        ready.sort_by(|left, right| {
            (left.front.priority, left.front.created_at, left.id.as_str()).cmp(&(
                right.front.priority,
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
        self.counts_matching(&[])
    }

    pub fn counts_matching(&self, labels: &[String]) -> Counts {
        let now = Utc::now();
        let mut counts = Counts::default();

        for task in self.tasks.values() {
            if !task.has_labels(labels) {
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
            if !task.front.status.is_terminal() && task.front.waiting_on.is_some() {
                counts.waiting += 1;
            }
            if !task.front.status.is_terminal()
                && (!self.blocking_ids(task).is_empty() || !self.open_children(&task.id).is_empty())
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
            for dep in &task.front.depends_on {
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
            for dep in &task.front.depends_on {
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
                        if let Some(position) = stack.iter().position(|node| node == dep) {
                            let mut cycle = stack[position..].to_vec();
                            cycle.push(dep.clone());
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

pub fn task_summary(graph: &Graph, task: &Task) -> serde_json::Value {
    let subtasks: Vec<String> = graph
        .children(&task.id)
        .iter()
        .map(|child| child.id.clone())
        .collect();
    let open_subtasks = graph.open_children(&task.id);
    let subtask_count = subtasks.len();
    let open_subtask_count = open_subtasks.len();
    let resolved_subtask_count = subtask_count.saturating_sub(open_subtask_count);
    let eligible_for_close =
        subtask_count > 0 && open_subtask_count == 0 && !task.front.status.is_terminal();

    serde_json::json!({
        "id": task.id,
        "title": task.front.title,
        "status": task.front.status,
        "priority": task.front.priority,
        "kind": task.kind(),
        "labels": task.front.labels,
        "depends_on": task.front.depends_on,
        "parent": task.front.parent,
        "subtasks": subtasks,
        "open_subtasks": open_subtasks,
        "subtask_count": subtask_count,
        "resolved_subtask_count": resolved_subtask_count,
        "open_subtask_count": open_subtask_count,
        "eligible_for_close": eligible_for_close,
        "waiting_on": task.front.waiting_on,
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
                priority: default_priority(),
                labels: Vec::new(),
                depends_on: deps.iter().map(|dep| dep.to_string()).collect(),
                parent: None,
                waiting_on: None,
                claim: None,
                stray_claim_times: None,
                created_at: now,
                updated_at: now,
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
    fn missing_parent_and_nested_subtasks_are_issues() {
        let mut orphan = task("orb-aaaaaa", Status::Todo, &[]);
        orphan.front.parent = Some("orb-missing".to_string());
        let graph = Graph::from_tasks(vec![orphan]);
        assert!(
            graph
                .validate()
                .iter()
                .any(|issue| issue.code == code::DANGLING_PARENT)
        );

        let mut parent = task("orb-aaaaaa", Status::Todo, &[]);
        parent.front.parent = Some("orb-cccccc".to_string());
        let mut child = task("orb-bbbbbb", Status::Todo, &[]);
        child.front.parent = Some("orb-aaaaaa".to_string());
        let graph = Graph::from_tasks(vec![task("orb-cccccc", Status::Todo, &[]), parent, child]);
        assert!(
            graph
                .validate()
                .iter()
                .any(|issue| issue.code == code::NESTED_SUBTASK)
        );
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
        high.front.priority = 0;
        let mut low = task("orb-bbbbbb", Status::Todo, &[]);
        low.front.priority = 3;

        let graph = Graph::from_tasks(vec![low, high]);
        let ready = graph.ready_tasks();
        assert_eq!(ready[0].id, "orb-aaaaaa");
    }
}

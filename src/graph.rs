use std::collections::{HashMap, HashSet};

use chrono::Utc;
use serde::Serialize;

use crate::board::Board;
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
}

impl Graph {
    pub fn from_tasks(tasks: Vec<Task>) -> Self {
        Self {
            tasks: tasks
                .into_iter()
                .map(|task| (task.id.clone(), task))
                .collect(),
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

        let graph = Self { tasks };
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
            && task.front.claimed_by.is_none()
            && task.front.waiting_on.is_none()
            && self.blocking_ids(task).is_empty()
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
                    && task.front.claimed_by.is_some()
                    && task.front.lease_until.is_some_and(|lease| lease <= now)
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
        let now = Utc::now();
        let mut counts = Counts {
            total: self.tasks.len(),
            ..Counts::default()
        };

        for task in self.tasks.values() {
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
                && task.front.claimed_by.is_some()
                && task.front.lease_until.is_some_and(|lease| lease <= now)
            {
                counts.stale += 1;
            }
            if !task.front.status.is_terminal() && task.front.waiting_on.is_some() {
                counts.waiting += 1;
            }
            if !task.front.status.is_terminal() && !self.blocking_ids(task).is_empty() {
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
    serde_json::json!({
        "id": task.id,
        "title": task.front.title,
        "status": task.front.status,
        "priority": task.front.priority,
        "depends_on": task.front.depends_on,
        "waiting_on": task.front.waiting_on,
        "claimed_by": task.front.claimed_by,
        "claimed_at": task.front.claimed_at,
        "lease_until": task.front.lease_until,
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
                depends_on: deps.iter().map(|dep| dep.to_string()).collect(),
                waiting_on: None,
                claimed_by: None,
                claimed_at: None,
                lease_until: None,
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

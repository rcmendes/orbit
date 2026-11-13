//! `orbit batch`: many Task operations under one lock, written and committed
//! once, all or none. Operations run in order against one in-memory Graph;
//! nothing touches disk until every operation has succeeded.

use chrono::Utc;
use serde::Deserialize;
use serde_json::{Value, json};
use std::collections::{BTreeMap, HashMap};

use crate::cli::BatchArgs;
use crate::comment;
use crate::graph::LoadMode;
use crate::ops::{NewTask, TaskUpdate};
use crate::task::TaskType;
use crate::transition::FieldUpdate;

use super::*;

/// A reference to a Task: a Task ID, or `key:` followed by a Source Key.
const KEY_PREFIX: &str = "key:";

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct BatchInput {
    operations: Vec<Operation>,
}

#[derive(Deserialize)]
#[serde(tag = "op", rename_all = "snake_case", deny_unknown_fields)]
enum Operation {
    Add {
        key: Option<String>,
        title: String,
        #[serde(rename = "type")]
        task_type: Option<TaskType>,
        parent: Option<String>,
        #[serde(default)]
        labels: Vec<String>,
        #[serde(default)]
        depends_on: Vec<String>,
        body: Option<String>,
        priority: Option<u8>,
        repo: Option<String>,
        waiting_on: Option<String>,
        area: Option<String>,
        verification: Option<String>,
        #[serde(default)]
        annotations: BTreeMap<String, Value>,
    },
    Update {
        id: String,
        title: Option<String>,
        body: Option<String>,
        priority: Option<u8>,
        #[serde(rename = "type")]
        task_type: Option<TaskType>,
        waiting_on: Option<String>,
        #[serde(default)]
        clear_waiting: bool,
        area: Option<String>,
        #[serde(default)]
        clear_area: bool,
        verification: Option<String>,
        #[serde(default)]
        clear_verification: bool,
    },
    Label {
        id: String,
        #[serde(default)]
        add: Vec<String>,
        #[serde(default)]
        remove: Vec<String>,
    },
    Depend {
        id: String,
        on: String,
    },
    Undepend {
        id: String,
        on: String,
    },
    Claim {
        id: String,
        by: Option<String>,
        #[serde(default)]
        steal: bool,
    },
    Heartbeat {
        id: String,
        by: Option<String>,
    },
    Release {
        id: String,
        by: Option<String>,
        #[serde(default)]
        force: bool,
    },
    Done {
        id: String,
        by: Option<String>,
        comment: Option<String>,
        #[serde(default)]
        force: bool,
    },
    Cancel {
        id: String,
        by: Option<String>,
        comment: Option<String>,
        #[serde(default)]
        force: bool,
    },
    Reopen {
        id: String,
        #[serde(default)]
        force: bool,
    },
    Comment {
        id: String,
        by: Option<String>,
        body: String,
    },
    Annotate {
        id: String,
        owner: String,
        value: Option<Value>,
    },
}

impl Operation {
    fn name(&self) -> &'static str {
        match self {
            Operation::Add { .. } => "add",
            Operation::Update { .. } => "update",
            Operation::Label { .. } => "label",
            Operation::Depend { .. } => "depend",
            Operation::Undepend { .. } => "undepend",
            Operation::Claim { .. } => "claim",
            Operation::Heartbeat { .. } => "heartbeat",
            Operation::Release { .. } => "release",
            Operation::Done { .. } => "done",
            Operation::Cancel { .. } => "cancel",
            Operation::Reopen { .. } => "reopen",
            Operation::Comment { .. } => "comment",
            Operation::Annotate { .. } => "annotate",
        }
    }
}

struct Applier<'a> {
    board: &'a Board,
    graph: Graph,
    /// IDs of Tasks this Batch created, in the order it created them.
    created: Vec<String>,
    keys: BTreeMap<String, String>,
    comments: Vec<(String, String, String)>,
    default_by: Option<String>,
}

impl Applier<'_> {
    /// A Task ID from a reference, resolving `key:` against this Batch, then the Board.
    fn resolve(&self, reference: &str) -> Result<String> {
        let Some(key) = reference.strip_prefix(KEY_PREFIX) else {
            return Ok(reference.to_string());
        };
        if let Some(id) = self.keys.get(key) {
            return Ok(id.clone());
        }
        match self.graph.find_by_source_key(key)? {
            Some(task) => Ok(task.id.clone()),
            None => Err(OrbitError::not_found(reference)),
        }
    }

    fn agent(&self, by: Option<String>) -> Result<String> {
        resolve_agent(by.or_else(|| self.default_by.clone()))
    }

    fn optional(&self, by: Option<String>) -> Option<String> {
        optional_agent(by.or_else(|| self.default_by.clone()))
    }

    fn queue_comment(&mut self, id: &str, author: String, body: String) -> Result<()> {
        comment::validate_body(&body)?;
        self.comments.push((id.to_string(), author, body));
        Ok(())
    }

    /// Applies one operation; returns (task id, changed, matched).
    fn apply(&mut self, operation: Operation) -> Result<(String, bool, bool)> {
        let now = Utc::now();
        match operation {
            Operation::Add {
                key,
                title,
                task_type,
                parent,
                labels,
                depends_on,
                body,
                priority,
                repo,
                waiting_on,
                area,
                verification,
                annotations,
            } => {
                let parent = parent.map(|p| self.resolve(&p)).transpose()?;
                let depends_on = depends_on
                    .iter()
                    .map(|dep| self.resolve(dep))
                    .collect::<Result<Vec<_>>>()?;
                let repo = match repo {
                    Some(repo) => Some(repo),
                    None => crate::repository::current_member_name(self.board)?,
                };
                let new = NewTask {
                    title,
                    priority,
                    task_type,
                    labels,
                    depends_on,
                    parent,
                    waiting_on,
                    area,
                    verification,
                    repo,
                    body: body.unwrap_or_default(),
                    source_key: key.clone(),
                    annotations,
                };
                let outcome = crate::ops::add(&mut self.graph, self.board, new, now)?;
                if let Some(key) = key {
                    self.keys.insert(key, outcome.id.clone());
                }
                if !outcome.matched {
                    self.created.push(outcome.id.clone());
                }
                Ok((outcome.id, !outcome.matched, outcome.matched))
            }
            Operation::Update {
                id,
                title,
                body,
                priority,
                task_type,
                waiting_on,
                clear_waiting,
                area,
                clear_area,
                verification,
                clear_verification,
            } => {
                let id = self.resolve(&id)?;
                let field = |value: Option<String>, clear: bool| {
                    if clear {
                        FieldUpdate::Clear
                    } else {
                        value.map_or(FieldUpdate::Keep, FieldUpdate::Set)
                    }
                };
                let change = TaskUpdate {
                    title,
                    body,
                    priority,
                    task_type,
                    waiting_on: field(waiting_on, clear_waiting),
                    area: field(area, clear_area),
                    verification: field(verification, clear_verification),
                };
                let changed = crate::ops::update(&mut self.graph, self.board, &id, change, now)?;
                Ok((id, changed, false))
            }
            Operation::Label { id, add, remove } => {
                let id = self.resolve(&id)?;
                let changed = crate::ops::label(&mut self.graph, &id, &add, &remove, now)?;
                Ok((id, changed, false))
            }
            Operation::Depend { id, on } => {
                let (id, on) = (self.resolve(&id)?, self.resolve(&on)?);
                let changed = crate::ops::depend(&mut self.graph, &id, &on, now)?;
                Ok((id, changed, false))
            }
            Operation::Undepend { id, on } => {
                let (id, on) = (self.resolve(&id)?, self.resolve(&on)?);
                let changed = crate::ops::undepend(&mut self.graph, &id, &on, now)?;
                Ok((id, changed, false))
            }
            Operation::Claim { id, by, steal } => {
                let id = self.resolve(&id)?;
                let by = self.agent(by)?;
                let changed = crate::ops::claim(&mut self.graph, self.board, &id, &by, steal, now)?;
                Ok((id, changed, false))
            }
            Operation::Heartbeat { id, by } => {
                let id = self.resolve(&id)?;
                let by = self.agent(by)?;
                let changed = crate::ops::heartbeat(&mut self.graph, self.board, &id, &by, now)?;
                Ok((id, changed, false))
            }
            Operation::Release { id, by, force } => {
                let id = self.resolve(&id)?;
                let by = self.optional(by);
                let changed = crate::ops::release(&mut self.graph, &id, by.as_deref(), force, now)?;
                Ok((id, changed, false))
            }
            Operation::Done {
                id,
                by,
                comment,
                force,
            } => self.finish(id, by, comment, force, crate::mutation::TerminalKind::Done),
            Operation::Cancel {
                id,
                by,
                comment,
                force,
            } => self.finish(
                id,
                by,
                comment,
                force,
                crate::mutation::TerminalKind::Cancelled,
            ),
            Operation::Reopen { id, force } => {
                let id = self.resolve(&id)?;
                // An open Task is already where reopen leads; a re-run must not fail on it.
                if !self.graph.get(&id)?.front.status.is_terminal() {
                    return Ok((id, false, false));
                }
                crate::ops::reopen(&mut self.graph, &id, force, now)?;
                Ok((id, true, false))
            }
            Operation::Comment { id, by, body } => {
                let id = self.resolve(&id)?;
                self.graph.get(&id)?;
                let author = self.agent(by)?;
                self.queue_comment(&id, author, body)?;
                Ok((id, true, false))
            }
            Operation::Annotate { id, owner, value } => {
                let id = self.resolve(&id)?;
                let changed = crate::ops::annotate(&mut self.graph, &id, &owner, value, now)?;
                Ok((id, changed, false))
            }
        }
    }

    fn finish(
        &mut self,
        id: String,
        by: Option<String>,
        comment: Option<String>,
        force: bool,
        kind: crate::mutation::TerminalKind,
    ) -> Result<(String, bool, bool)> {
        let id = self.resolve(&id)?;
        let caller = self.optional(by.clone());
        if let Some(body) = comment {
            let author = self.agent(by)?;
            self.queue_comment(&id, author, body)?;
        }
        let outcome = crate::ops::finish(
            &mut self.graph,
            &id,
            caller.as_deref(),
            force,
            kind,
            Utc::now(),
        )?;
        Ok((id, outcome.changed, false))
    }
}

/// The order Task files are written in: new Tasks in creation order (each one's
/// parent and dependencies already exist then), then the other Tasks. A crash
/// between writes therefore leaves a consistent Board that a re-run completes.
fn write_order(graph: &Graph, before: &HashMap<String, String>, created: &[String]) -> Vec<String> {
    let mut rest: Vec<String> = graph
        .tasks
        .keys()
        .filter(|id| !created.contains(id) && before.contains_key(*id))
        .cloned()
        .collect();
    rest.sort();
    created.iter().cloned().chain(rest).collect()
}

fn with_position(mut error: OrbitError, index: usize, op: &str) -> OrbitError {
    let mut details = match error.details.take() {
        Some(Value::Object(map)) => map,
        Some(other) => {
            let mut map = serde_json::Map::new();
            map.insert("cause".to_string(), other);
            map
        }
        None => serde_json::Map::new(),
    };
    details.insert("index".to_string(), json!(index));
    details.insert("op".to_string(), json!(op));
    error.message = format!("operation {index} ({op}): {}", error.message);
    error.details = Some(Value::Object(details));
    error
}

fn read_input(file: &str) -> Result<BatchInput> {
    let text = if file == "-" {
        let mut text = String::new();
        std::io::stdin()
            .read_to_string(&mut text)
            .map_err(|error| {
                OrbitError::new(code::INVALID_BATCH, format!("cannot read stdin: {error}"))
            })?;
        text
    } else {
        std::fs::read_to_string(file).map_err(|error| {
            OrbitError::new(code::INVALID_BATCH, format!("cannot read {file}: {error}"))
        })?
    };
    serde_json::from_str(&text)
        .map_err(|error| OrbitError::new(code::INVALID_BATCH, format!("invalid batch: {error}")))
}

pub(super) fn cmd_batch(board: &Board, args: BatchArgs) -> Result<crate::output::CommandOutput> {
    let input = read_input(&args.file)?;
    let (graph, _) = Graph::load(board, LoadMode::Strict)?;
    let before: HashMap<String, String> = graph
        .tasks
        .iter()
        .map(|(id, task)| Ok((id.clone(), task::render_task(task)?)))
        .collect::<Result<_>>()?;
    let heartbeat_only = !input.operations.is_empty()
        && input
            .operations
            .iter()
            .all(|operation| matches!(operation, Operation::Heartbeat { .. }));

    let mut run = Applier {
        board,
        graph,
        keys: BTreeMap::new(),
        comments: Vec::new(),
        created: Vec::new(),
        default_by: args.by,
    };
    let mut results = Vec::with_capacity(input.operations.len());
    for (index, operation) in input.operations.into_iter().enumerate() {
        let op = operation.name();
        let (id, changed, matched) = run
            .apply(operation)
            .map_err(|error| with_position(error, index, op))?;
        results.push(json!({
            "index": index, "op": op, "id": id, "changed": changed, "matched": matched,
        }));
    }

    let mut committed = false;
    let mut warnings = Vec::new();
    if !args.dry_run {
        for id in write_order(&run.graph, &before, &run.created) {
            let id = &id;
            let task = run.graph.get(id)?;
            match before.get(id) {
                None => {
                    crate::mutation::create_task(board, task)?;
                }
                Some(old) if *old != task::render_task(task)? => {
                    crate::mutation::write_task(board, task)?;
                }
                Some(_) => {}
            }
        }
        for (id, author, body) in &run.comments {
            crate::mutation::append_comment(board, id, author, body, Utc::now())?;
        }
        // Commit even when nothing was written now: a re-run after a crash
        // before the commit must publish the files the first run left behind.
        if !heartbeat_only {
            let message = format!("orbit: batch {} operations", results.len());
            match board.commit(&message, false)? {
                crate::git::CommitOutcome::Committed => committed = true,
                outcome => warnings.extend(crate::mutation::commit_warning(outcome)),
            }
        }
    }

    let changed = results.iter().filter(|r| r["changed"] == true).count();
    let human = format!(
        "{}{} operations, {changed} changed",
        if args.dry_run { "dry run: " } else { "" },
        results.len()
    );
    let mut output = crate::output::CommandOutput::new(
        json!({
            "keys": run.keys,
            "results": results,
            "committed": committed,
            "dry_run": args.dry_run,
        }),
        human,
    );
    output.warnings.extend(warnings);
    Ok(output)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::task::{Status, Task, TaskFrontmatter};

    fn task(id: &str) -> Task {
        let now = Utc::now();
        Task {
            id: id.to_string(),
            front: TaskFrontmatter {
                title: id.to_string(),
                status: Status::Todo,
                task_type: TaskType::Task,
                priority: 2,
                labels: Vec::new(),
                depends_on: Vec::new(),
                parent: None,
                waiting_on: None,
                checkpoint: None,
                area: None,
                verification: None,
                github: None,
                remote: None,
                repo: None,
                claim: None,
                stray_claim_times: None,
                source_key: None,
                annotations: BTreeMap::new(),
                created_at: now,
                updated_at: now,
                extra: BTreeMap::new(),
            },
            body: String::new(),
        }
    }

    #[test]
    fn new_tasks_are_written_in_creation_order_before_changed_ones() {
        // "orb-zzzzzz" was created first and the others depend on it; sorted
        // order would write a dependent before the Task it depends on.
        let existing = task("orb-mmmmmm");
        let graph = Graph::from_tasks(vec![
            task("orb-zzzzzz"),
            task("orb-aaaaaa"),
            task("orb-bbbbbb"),
            existing.clone(),
        ]);
        let before: HashMap<String, String> = HashMap::from([(existing.id.clone(), String::new())]);
        let created = vec![
            "orb-zzzzzz".to_string(),
            "orb-bbbbbb".to_string(),
            "orb-aaaaaa".to_string(),
        ];
        assert_eq!(
            write_order(&graph, &before, &created),
            ["orb-zzzzzz", "orb-bbbbbb", "orb-aaaaaa", "orb-mmmmmm"]
        );
    }
}

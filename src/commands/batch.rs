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
        task_type: Option<String>,
        parent: Option<String>,
        #[serde(default)]
        labels: Vec<String>,
        body: Option<String>,
        priority: Option<String>,
        repository: Option<String>,
        component: Option<String>,
        verification: Option<String>,
        external_id: Option<String>,
        reporter: Option<String>,
        #[serde(default)]
        properties: BTreeMap<String, Value>,
        /// Rejected: an `add` creates the Task, so there is no version yet.
        if_version: Option<String>,
    },
    Update {
        id: String,
        if_version: Option<String>,
        title: Option<String>,
        body: Option<String>,
        priority: Option<String>,
        #[serde(rename = "type")]
        task_type: Option<String>,
        component: Option<String>,
        #[serde(default)]
        clear_component: bool,
        verification: Option<String>,
        #[serde(default)]
        clear_verification: bool,
        repository: Option<String>,
        #[serde(default)]
        clear_repository: bool,
        external_id: Option<String>,
        #[serde(default)]
        clear_external_id: bool,
    },
    Label {
        id: String,
        if_version: Option<String>,
        #[serde(default)]
        add: Vec<String>,
        #[serde(default)]
        remove: Vec<String>,
    },
    Depend {
        id: String,
        if_version: Option<String>,
        on: String,
    },
    Undepend {
        id: String,
        if_version: Option<String>,
        on: String,
    },
    Link {
        id: String,
        if_version: Option<String>,
        #[serde(rename = "type")]
        kind: Option<String>,
        to: Option<String>,
        external: Option<String>,
        url: Option<String>,
        #[serde(default)]
        blocking: Option<bool>,
        #[serde(default)]
        attributes: BTreeMap<String, serde_yaml_ng::Value>,
    },
    Unlink {
        id: String,
        if_version: Option<String>,
        #[serde(rename = "type")]
        kind: Option<String>,
        to: Option<String>,
        external: Option<String>,
    },
    Claim {
        id: String,
        if_version: Option<String>,
        by: Option<String>,
        kind: Option<crate::task::IdentityKind>,
        #[serde(default)]
        steal: bool,
    },
    Assign {
        id: String,
        if_version: Option<String>,
        to: String,
        kind: Option<crate::task::IdentityKind>,
        #[serde(default)]
        force: bool,
    },
    Heartbeat {
        id: String,
        if_version: Option<String>,
        by: Option<String>,
    },
    Release {
        id: String,
        if_version: Option<String>,
        by: Option<String>,
        #[serde(default)]
        force: bool,
    },
    Done {
        id: String,
        if_version: Option<String>,
        by: Option<String>,
        comment: Option<String>,
        #[serde(default)]
        force: bool,
    },
    Cancel {
        id: String,
        if_version: Option<String>,
        by: Option<String>,
        comment: Option<String>,
        #[serde(default)]
        force: bool,
    },
    Reopen {
        id: String,
        if_version: Option<String>,
        #[serde(default)]
        force: bool,
    },
    Transition {
        id: String,
        if_version: Option<String>,
        name: String,
        by: Option<String>,
        #[serde(default)]
        force: bool,
    },
    Submit {
        id: String,
        if_version: Option<String>,
        by: Option<String>,
        kind: Option<crate::task::IdentityKind>,
        #[serde(default)]
        command: Vec<String>,
        exit_code: Option<i64>,
        commit: Option<String>,
        #[serde(default)]
        file: Vec<String>,
        note: Option<String>,
        #[serde(default)]
        force: bool,
    },
    Review {
        id: String,
        if_version: Option<String>,
        verdict: String,
        reason: Option<String>,
        by: Option<String>,
        kind: Option<crate::task::IdentityKind>,
        #[serde(default)]
        force: bool,
    },
    Ask {
        id: String,
        if_version: Option<String>,
        question: String,
        by: Option<String>,
    },
    Answer {
        id: String,
        if_version: Option<String>,
        answer: String,
        by: Option<String>,
    },
    Comment {
        id: String,
        if_version: Option<String>,
        by: Option<String>,
        body: String,
    },
    Property {
        id: String,
        if_version: Option<String>,
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
            Operation::Link { .. } => "link",
            Operation::Unlink { .. } => "unlink",
            Operation::Claim { .. } => "claim",
            Operation::Assign { .. } => "assign",
            Operation::Heartbeat { .. } => "heartbeat",
            Operation::Release { .. } => "release",
            Operation::Done { .. } => "done",
            Operation::Cancel { .. } => "cancel",
            Operation::Reopen { .. } => "reopen",
            Operation::Transition { .. } => "transition",
            Operation::Submit { .. } => "submit",
            Operation::Review { .. } => "review",
            Operation::Ask { .. } => "ask",
            Operation::Answer { .. } => "answer",
            Operation::Comment { .. } => "comment",
            Operation::Property { .. } => "property",
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
    /// `updated_at` of every Task as the Batch found it; `if_version`
    /// compares against this snapshot, not the in-memory state earlier
    /// operations may already have touched.
    versions: HashMap<String, chrono::DateTime<Utc>>,
}

impl Applier<'_> {
    /// `if_version` guards the Task's `updated_at` as of batch start. The
    /// `id` is the resolved internal ID.
    fn check_version(&self, id: &str, expected: Option<&str>) -> Result<()> {
        let Some(expected) = expected else {
            return Ok(());
        };
        let expected_at = crate::concurrency::parse_version(expected)?;
        match self.versions.get(id) {
            Some(actual) if *actual == expected_at => Ok(()),
            Some(actual) => Err(crate::concurrency::version_conflict(
                id,
                expected_at,
                *actual,
            )),
            None => Err(OrbitError::invalid_argument(format!(
                "if_version cannot guard {id}: the Task did not exist when the batch started"
            ))),
        }
    }

    /// A Task ID from a reference, resolving `key:` against this Batch, then
    /// the Board's Source Keys, project keys, and aliases.
    fn resolve(&self, reference: &str) -> Result<String> {
        let Some(key) = reference.strip_prefix(KEY_PREFIX) else {
            return self.graph.resolve(reference);
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
                body,
                priority,
                repository,
                component,
                verification,
                external_id,
                reporter,
                properties,
                if_version,
            } => {
                if if_version.is_some() {
                    return Err(OrbitError::invalid_argument(
                        "if_version does not apply to `add`; the Task does not exist yet",
                    ));
                }
                let parent = parent.map(|p| self.resolve(&p)).transpose()?;
                let repository = match repository {
                    Some(repository) => Some(repository),
                    None => crate::repository::current_member_name(self.board)?,
                };
                let new = NewTask {
                    title,
                    priority,
                    task_type,
                    labels,
                    parent,
                    component,
                    verification,
                    repository,
                    external_id,
                    reporter: reporter.or_else(|| self.optional(None)),
                    body: body.unwrap_or_default(),
                    source_key: key.clone(),
                    properties,
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
                component,
                clear_component,
                verification,
                clear_verification,
                repository,
                clear_repository,
                external_id,
                clear_external_id,
                if_version,
            } => {
                let id = self.resolve(&id)?;
                self.check_version(&id, if_version.as_deref())?;
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
                    component: field(component, clear_component),
                    verification: field(verification, clear_verification),
                    repository: field(repository, clear_repository),
                    external_id: field(external_id, clear_external_id),
                };
                let changed = crate::ops::update(&mut self.graph, self.board, &id, change, now)?;
                Ok((id, changed, false))
            }
            Operation::Label {
                id,
                if_version,
                add,
                remove,
            } => {
                let id = self.resolve(&id)?;
                self.check_version(&id, if_version.as_deref())?;
                let changed = crate::ops::label(&mut self.graph, &id, &add, &remove, now)?;
                Ok((id, changed, false))
            }
            Operation::Depend { id, if_version, on } => {
                let (id, on) = (self.resolve(&id)?, self.resolve(&on)?);
                self.check_version(&id, if_version.as_deref())?;
                let changed = crate::ops::depend(&mut self.graph, &id, &on, now)?;
                Ok((id, changed, false))
            }
            Operation::Undepend { id, if_version, on } => {
                let (id, on) = (self.resolve(&id)?, self.resolve(&on)?);
                self.check_version(&id, if_version.as_deref())?;
                let changed = crate::ops::undepend(&mut self.graph, &id, &on, now)?;
                Ok((id, changed, false))
            }
            Operation::Link {
                id,
                if_version,
                kind,
                to,
                external,
                url,
                blocking,
                attributes,
            } => {
                let id = self.resolve(&id)?;
                self.check_version(&id, if_version.as_deref())?;
                let to = to.map(|target| self.resolve(&target)).transpose()?;
                let kind = crate::task::normalize_link_kind(
                    kind.as_deref().unwrap_or(crate::task::LINK_TYPE_RELATES_TO),
                )?;
                let mut target = crate::ops::link_target(to.as_deref(), external.as_deref())?;
                if let (crate::task::LinkTarget::External(reference), Some(url)) =
                    (&mut target, url)
                {
                    reference.url = Some(url);
                }
                let link = crate::task::Link {
                    kind,
                    target,
                    blocking,
                    attributes,
                };
                let changed = crate::ops::link(&mut self.graph, &id, link, now)?;
                Ok((id, changed, false))
            }
            Operation::Unlink {
                id,
                if_version,
                kind,
                to,
                external,
            } => {
                let id = self.resolve(&id)?;
                self.check_version(&id, if_version.as_deref())?;
                let to = to.map(|target| self.resolve(&target)).transpose()?;
                let kind = kind
                    .as_deref()
                    .map(crate::task::normalize_link_kind)
                    .transpose()?;
                let target = crate::ops::link_target(to.as_deref(), external.as_deref())?;
                let changed =
                    crate::ops::unlink(&mut self.graph, &id, kind.as_deref(), &target, now)?;
                Ok((id, changed, false))
            }
            Operation::Claim {
                id,
                if_version,
                by,
                kind,
                steal,
            } => {
                let id = self.resolve(&id)?;
                self.check_version(&id, if_version.as_deref())?;
                let by = self.agent(by)?;
                let changed = crate::ops::claim(
                    &mut self.graph,
                    self.board,
                    &id,
                    &by,
                    kind.unwrap_or(crate::task::IdentityKind::Agent),
                    steal,
                    now,
                )?;
                Ok((id, changed, false))
            }
            Operation::Assign {
                id,
                if_version,
                to,
                kind,
                force,
            } => {
                let id = self.resolve(&id)?;
                self.check_version(&id, if_version.as_deref())?;
                let changed = crate::ops::assign(
                    &mut self.graph,
                    &id,
                    &to,
                    kind.unwrap_or(crate::task::IdentityKind::Human),
                    force,
                    now,
                )?;
                Ok((id, changed, false))
            }
            Operation::Heartbeat { id, if_version, by } => {
                let id = self.resolve(&id)?;
                self.check_version(&id, if_version.as_deref())?;
                let by = self.agent(by)?;
                let changed = crate::ops::heartbeat(&mut self.graph, self.board, &id, &by, now)?;
                Ok((id, changed, false))
            }
            Operation::Release {
                id,
                if_version,
                by,
                force,
            } => {
                let id = self.resolve(&id)?;
                self.check_version(&id, if_version.as_deref())?;
                let by = self.optional(by);
                let changed = crate::ops::release(&mut self.graph, &id, by.as_deref(), force, now)?;
                Ok((id, changed, false))
            }
            Operation::Done {
                id,
                if_version,
                by,
                comment,
                force,
            } => self.finish(
                id,
                if_version,
                by,
                comment,
                force,
                crate::mutation::TerminalKind::Done,
            ),
            Operation::Cancel {
                id,
                if_version,
                by,
                comment,
                force,
            } => self.finish(
                id,
                if_version,
                by,
                comment,
                force,
                crate::mutation::TerminalKind::Cancelled,
            ),
            Operation::Reopen {
                id,
                if_version,
                force,
            } => {
                let id = self.resolve(&id)?;
                self.check_version(&id, if_version.as_deref())?;
                // An open Task is already where reopen leads; a re-run must not fail on it.
                if !self
                    .graph
                    .workflow
                    .is_terminal(&self.graph.get(&id)?.front.status)
                {
                    return Ok((id, false, false));
                }
                crate::ops::reopen(&mut self.graph, &id, force, now)?;
                Ok((id, true, false))
            }
            Operation::Transition {
                id,
                if_version,
                name,
                by,
                force,
            } => {
                let id = self.resolve(&id)?;
                self.check_version(&id, if_version.as_deref())?;
                let caller = self.optional(by);
                let outcome = crate::ops::transition(
                    &mut self.graph,
                    &id,
                    &name,
                    caller.as_deref(),
                    force,
                    now,
                )?;
                Ok((id, outcome == crate::transition::Outcome::Changed, false))
            }
            Operation::Submit {
                id,
                if_version,
                by,
                kind,
                command,
                exit_code,
                commit,
                file,
                note,
                force,
            } => {
                let id = self.resolve(&id)?;
                self.check_version(&id, if_version.as_deref())?;
                let by = self.agent(by)?;
                let evidence = crate::review::Evidence {
                    commands: command,
                    exit_code,
                    commit,
                    files: file,
                    note,
                };
                let changed = crate::ops::submit(
                    &mut self.graph,
                    &id,
                    &by,
                    kind.unwrap_or_default(),
                    &evidence,
                    force,
                    now,
                )?;
                Ok((id, changed, false))
            }
            Operation::Review {
                id,
                if_version,
                verdict,
                reason,
                by,
                kind,
                force,
            } => {
                let id = self.resolve(&id)?;
                self.check_version(&id, if_version.as_deref())?;
                let verdict = match verdict.as_str() {
                    "approve" => crate::review::Verdict::Approve,
                    "request-changes" | "request_changes" => crate::review::Verdict::RequestChanges,
                    other => {
                        return Err(OrbitError::invalid_argument(format!(
                            "unknown review verdict `{other}`; expected approve or request-changes"
                        )));
                    }
                };
                let by = self.agent(by)?;
                let outcome = crate::ops::review(
                    &mut self.graph,
                    &id,
                    verdict,
                    reason.as_deref(),
                    &by,
                    kind.unwrap_or_default(),
                    force,
                    now,
                )?;
                Ok((id, outcome.changed, false))
            }
            Operation::Ask {
                id,
                if_version,
                question,
                by,
            } => {
                let id = self.resolve(&id)?;
                self.check_version(&id, if_version.as_deref())?;
                let by = self.agent(by)?;
                let outcome = crate::ops::ask(&mut self.graph, &id, &question, &by, now)?;
                Ok((id, outcome.changed, false))
            }
            Operation::Answer {
                id,
                if_version,
                answer,
                by,
            } => {
                let id = self.resolve(&id)?;
                self.check_version(&id, if_version.as_deref())?;
                let by = self.agent(by)?;
                let outcome = crate::ops::answer(&mut self.graph, &id, &answer, &by, now)?;
                Ok((id, outcome.changed, false))
            }
            Operation::Comment {
                id,
                if_version,
                by,
                body,
            } => {
                let id = self.resolve(&id)?;
                self.check_version(&id, if_version.as_deref())?;
                self.graph.get(&id)?;
                let author = self.agent(by)?;
                self.queue_comment(&id, author, body)?;
                Ok((id, true, false))
            }
            Operation::Property {
                id,
                if_version,
                owner,
                value,
            } => {
                let id = self.resolve(&id)?;
                self.check_version(&id, if_version.as_deref())?;
                let changed = crate::ops::property(&mut self.graph, &id, &owner, value, now)?;
                Ok((id, changed, false))
            }
        }
    }

    fn finish(
        &mut self,
        id: String,
        if_version: Option<String>,
        by: Option<String>,
        comment: Option<String>,
        force: bool,
        kind: crate::mutation::TerminalKind,
    ) -> Result<(String, bool, bool)> {
        let id = self.resolve(&id)?;
        self.check_version(&id, if_version.as_deref())?;
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
    let versions: HashMap<String, chrono::DateTime<Utc>> = graph
        .tasks
        .iter()
        .map(|(id, task)| (id.clone(), task.front.updated_at))
        .collect();
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

    // The feed's actor for the whole Batch commit: the default `--by`
    // identity (per-operation `by` still wins inside each operation).
    let batch_actor = optional_agent(args.by.clone());
    let mut run = Applier {
        board,
        graph,
        keys: BTreeMap::new(),
        comments: Vec::new(),
        created: Vec::new(),
        default_by: args.by,
        versions,
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
        // Persist the key high-water mark with the Batch's Task files.
        if !run.created.is_empty() {
            crate::key::persist(board, &run.graph)?;
        }
        // Commit even when nothing was written now: a re-run after a crash
        // before the commit must publish the files the first run left behind.
        if !heartbeat_only {
            let message = format!("orbit: batch {} operations", results.len());
            match board.commit_as(&message, false, batch_actor.as_deref())? {
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
    use crate::task::{Task, TaskFrontmatter};

    fn task(id: &str) -> Task {
        let now = Utc::now();
        Task {
            id: id.to_string(),
            front: TaskFrontmatter {
                title: id.to_string(),
                status: "todo".to_string(),
                key: None,
                key_aliases: Vec::new(),
                resolution: None,
                resolved_at: None,
                task_type: "task".to_string(),
                priority: "medium".to_string(),
                labels: Vec::new(),
                links: Vec::new(),
                parent: None,
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
                properties: BTreeMap::new(),
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

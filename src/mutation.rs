//! Board mutation execution. Callers hold the Board's exclusive access scope.
//! Publication is atomic per file; a multi-file mutation does not promise rollback.
use crate::{
    board::Board,
    comment::{self, Comment},
    error::{OrbitError, Result, code},
    git::CommitOutcome,
    graph::{Graph, LoadMode},
    task::{self, Status, Task},
    transition::{Outcome, TransitionError},
};
use chrono::{DateTime, Utc};

#[derive(Clone, Copy)]
pub enum TerminalKind {
    Done,
    Cancelled,
}
impl TerminalKind {
    pub fn past(self) -> &'static str {
        match self {
            Self::Done => "done",
            Self::Cancelled => "cancelled",
        }
    }
    fn verb(self) -> &'static str {
        match self {
            Self::Done => "done",
            Self::Cancelled => "cancel",
        }
    }
}

pub struct MutationResult {
    pub graph: Graph,
    pub task: Task,
    pub changed: bool,
    pub comments: Option<Vec<Comment>>,
    pub warnings: Vec<String>,
    pub comment_failed: bool,
}

pub fn write_task(board: &Board, task: &Task) -> Result<()> {
    let rendered = task::render_task(task)?;
    crate::atomic::write_atomic(&board.task_path(&task.id), rendered.as_bytes(), true)
}

pub fn persist_task(board: &Board, task: &Task, message: &str) -> Result<CommitOutcome> {
    write_task(board, task)?;
    board.commit(message, false)
}

pub fn commit_warning(outcome: CommitOutcome) -> Option<String> {
    match outcome {
        CommitOutcome::Failed(message) => Some(format!("COMMIT_FAILED: {message}")),
        _ => None,
    }
}

pub fn finish(
    board: &Board,
    id: &str,
    caller: Option<&str>,
    force: bool,
    comment: Option<(String, String)>,
    kind: TerminalKind,
    now: DateTime<Utc>,
) -> Result<MutationResult> {
    if let Some((_, body)) = &comment {
        comment::validate_body(body)?;
    }
    let (mut graph, _) = Graph::load(board, LoadMode::Strict)?;
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
        let task = graph.get_mut(child)?;
        task.cancel(caller, true, now)?;
        write_task(board, task)?;
    }
    let task = graph.get_mut(id)?;
    let outcome = match kind {
        TerminalKind::Done => task.complete(caller, force, &blocking, &[], now)?,
        TerminalKind::Cancelled => task.cancel(caller, force, now)?,
    };
    let changed = outcome == Outcome::Changed;
    let mut warnings = Vec::new();
    let mut comments = None;
    let mut comment_failed = false;
    if changed {
        write_task(board, task)?;
    }
    if let Some((author, body)) = comment {
        let append = (|| {
            let mut comments = board.read_comments(id)?;
            comments.push(Comment {
                author,
                body: body.trim_end_matches(['\r', '\n']).into(),
                created_at: now,
            });
            board.write_comments(id, &comments)?;
            Ok(comments)
        })();
        match append {
            Ok(value) => {
                comments = Some(value);
                if !changed {
                    task.touch(now);
                    write_task(board, task)?;
                }
            }
            Err(error) if changed => {
                let error: OrbitError = error;
                warnings.push(format!("COMMENT_FAILED: {}", error.message));
                comment_failed = true;
            }
            Err(error) => return Err(error),
        }
    }
    if (changed || comments.is_some() || !open.is_empty())
        && let Some(warning) =
            commit_warning(board.commit(&format!("orbit: {} {id}", kind.verb()), false)?)
    {
        warnings.push(warning);
    }
    if !open.is_empty() {
        warnings.push(format!("cancelled open SubTasks: {}", open.join(", ")));
    }
    let task = graph.get(id)?.clone();
    Ok(MutationResult {
        graph,
        task,
        changed,
        comments,
        warnings,
        comment_failed,
    })
}

pub enum Lifecycle<'a> {
    Claim {
        agent: &'a str,
        steal: bool,
    },
    Release {
        caller: Option<&'a str>,
        force: bool,
    },
    Heartbeat {
        agent: &'a str,
    },
    Checkpoint {
        agent: &'a str,
        update: crate::transition::CheckpointUpdate,
    },
}

/// Executes a lifecycle operation against an already loaded snapshot. Used by
/// claim-next so selection and claim remain inside the same exclusive scope.
pub fn execute(
    board: &Board,
    mut graph: Graph,
    id: &str,
    operation: Lifecycle<'_>,
    now: DateTime<Utc>,
) -> Result<MutationResult> {
    let lease = chrono::Duration::minutes(board.config.board.lease_minutes);
    let (verb, changed, heartbeat) = match operation {
        Lifecycle::Claim { agent, steal } => {
            let task = graph.get(id)?;
            let gate = crate::transition::ClaimGate {
                hold_labels: graph.hold_labels(task),
                blocking: graph.blocking_ids(task),
                open_subtasks: graph.open_children(id),
            };
            graph.get_mut(id)?.claim(agent, steal, lease, &gate, now)?;
            ("claim", true, false)
        }
        Lifecycle::Release { caller, force } => (
            "release",
            graph.get_mut(id)?.release(caller, force, now)? == Outcome::Changed,
            false,
        ),
        Lifecycle::Heartbeat { agent } => {
            graph.get_mut(id)?.heartbeat(agent, lease, now)?;
            ("heartbeat", true, true)
        }
        Lifecycle::Checkpoint { agent, update } => {
            graph.get_mut(id)?.record_checkpoint(agent, update, now)?;
            ("checkpoint", true, false)
        }
    };
    let task = graph.get(id)?.clone();
    let mut warnings = Vec::new();
    if changed {
        write_task(board, &task)?;
        if !heartbeat
            && let Some(warning) =
                commit_warning(board.commit(&format!("orbit: {verb} {id}"), false)?)
        {
            warnings.push(warning);
        }
    }
    Ok(MutationResult {
        graph,
        task,
        changed,
        comments: None,
        warnings,
        comment_failed: false,
    })
}

/// Reopens a Task and, when needed, its completed Parent Task in one scope.
pub fn reopen(board: &Board, id: &str, force: bool, now: DateTime<Utc>) -> Result<MutationResult> {
    let (mut graph, _) = Graph::load(board, LoadMode::Strict)?;

    let task = graph.get(id)?.clone();
    let child_dependents: Vec<String> = graph
        .dependents(id)
        .iter()
        .filter(|dependent| dependent.front.status == Status::Done)
        .map(|dependent| dependent.id.clone())
        .collect();
    let mut probe = task.clone();
    probe.reopen(force, &child_dependents, now)?;

    let mut reopened_parent = None;
    if let Some(parent_id) = task.front.parent.clone() {
        let parent_status = graph.get(&parent_id)?.front.status;
        match parent_status {
            Status::Done => {
                let done_dependents: Vec<String> = graph
                    .dependents(&parent_id)
                    .iter()
                    .filter(|dependent| dependent.front.status == Status::Done)
                    .map(|dependent| dependent.id.clone())
                    .collect();
                graph
                    .get_mut(&parent_id)?
                    .reopen(force, &done_dependents, now)?;
                let parent = graph.get(&parent_id)?.clone();
                write_task(board, &parent)?;
                reopened_parent = Some(parent_id);
            }
            Status::Cancelled => {
                return Err(OrbitError::new(
                    code::INVALID_TRANSITION,
                    format!(
                        "{} is a SubTask of cancelled {parent_id}; reopen the parent first",
                        id
                    ),
                ));
            }
            _ => {}
        }
    }

    let task = {
        let task = graph.get_mut(id)?;
        task.reopen(force, &child_dependents, now)?;
        task.clone()
    };
    write_task(board, &task)?;

    let mut warnings = Vec::new();
    if let Some(warning) = commit_warning(board.commit(&format!("orbit: reopen {id}"), false)?) {
        warnings.push(warning);
    }
    if let Some(parent) = reopened_parent {
        warnings.push(format!("reopened parent {parent}"));
    }
    Ok(MutationResult {
        graph,
        task,
        changed: true,
        comments: None,
        warnings,
        comment_failed: false,
    })
}

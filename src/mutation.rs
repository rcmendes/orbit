//! Board mutation execution. Callers hold the Board's exclusive access scope.
//! Publication is atomic per file; a multi-file mutation does not promise rollback.
use crate::{
    board::Board,
    comment::{self, Comment},
    error::{OrbitError, Result},
    git::CommitOutcome,
    graph::{Graph, LoadMode},
    task::{self, Task},
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

/// Writes a Task and commits it, returning the commit warning if any.
pub fn save(board: &Board, task: &Task, message: &str) -> Result<Option<String>> {
    Ok(commit_warning(persist_task(board, task, message)?))
}

/// Writes a new Task file (never replacing an existing one) without committing.
pub fn create_task(board: &Board, task: &Task) -> Result<()> {
    let rendered = task::render_task(task)?;
    crate::atomic::write_atomic(&board.task_path(&task.id), rendered.as_bytes(), false)
}

/// Commits already-published files, returning the commit warning if any.
pub fn commit(board: &Board, message: &str) -> Result<Option<String>> {
    Ok(commit_warning(board.commit(message, false)?))
}

/// Removes a Task file and its comment sidecar; absent files are fine.
pub fn remove_task(board: &Board, id: &str) -> Result<()> {
    for path in [board.task_path(id), board.comment_path(id)] {
        match std::fs::remove_file(&path) {
            Ok(()) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => {
                return Err(OrbitError::io(format!("delete {}", path.display()), error));
            }
        }
    }
    Ok(())
}

/// Validates and appends a Comment to a Task's sidecar, returning all comments.
pub fn append_comment(
    board: &Board,
    id: &str,
    author: &str,
    body: &str,
    created_at: DateTime<Utc>,
) -> Result<Vec<Comment>> {
    comment::validate_body(body)?;
    let mut comments = board.read_comments(id)?;
    comments.push(Comment {
        author: author.into(),
        body: body.trim_end_matches(['\r', '\n']).into(),
        created_at,
    });
    board.write_comments(id, &comments)?;
    Ok(comments)
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
    let outcome = crate::ops::finish(&mut graph, id, caller, force, kind, now)?;
    let open = outcome.cancelled_subtasks;
    for child in &open {
        write_task(board, graph.get(child)?)?;
    }
    let task = graph.get_mut(id)?;
    let changed = outcome.changed;
    let mut warnings = Vec::new();
    let mut comments = None;
    let mut comment_failed = false;
    let mut comment_warning = None;
    if changed {
        write_task(board, task)?;
    }
    if let Some((author, body)) = comment {
        let append = append_comment(board, id, &author, &body, now);
        match append {
            Ok(value) => {
                comments = Some(value);
                if !changed {
                    task.touch(now);
                    write_task(board, task)?;
                }
            }
            Err(error) if changed => {
                comment_warning = Some(format!("COMMENT_FAILED: {}", error.message));
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
    // Commit failures precede comment failures, matching the pre-module order.
    warnings.extend(comment_warning);
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

/// Executes a lifecycle operation on a freshly loaded snapshot.
pub fn execute(
    board: &Board,
    id: &str,
    operation: Lifecycle<'_>,
    now: DateTime<Utc>,
) -> Result<MutationResult> {
    let (graph, _) = Graph::load(board, LoadMode::Strict)?;
    execute_in(board, graph, id, operation, now)
}

/// Executes a lifecycle operation against an already loaded snapshot. Used by
/// claim-next so selection and claim remain inside the same exclusive scope.
pub fn execute_in(
    board: &Board,
    mut graph: Graph,
    id: &str,
    operation: Lifecycle<'_>,
    now: DateTime<Utc>,
) -> Result<MutationResult> {
    let (verb, changed, heartbeat) = match operation {
        Lifecycle::Claim { agent, steal } => (
            "claim",
            crate::ops::claim(&mut graph, board, id, agent, steal, now)?,
            false,
        ),
        Lifecycle::Release { caller, force } => (
            "release",
            crate::ops::release(&mut graph, id, caller, force, now)?,
            false,
        ),
        Lifecycle::Heartbeat { agent } => (
            "heartbeat",
            crate::ops::heartbeat(&mut graph, board, id, agent, now)?,
            true,
        ),
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

    let reopened_parent = crate::ops::reopen(&mut graph, id, force, now)?;
    if let Some(parent_id) = &reopened_parent {
        write_task(board, graph.get(parent_id)?)?;
    }
    let task = graph.get(id)?.clone();
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

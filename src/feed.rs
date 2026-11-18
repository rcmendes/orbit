//! Change feed derived from the Board's Git history (ADR 0022).
//!
//! Every meaningful Board mutation lands as one commit on the Board branch
//! with a subject like `orbit: <verb> <id>`. [`collect`] replays that history
//! — oldest commit first — and derives named events (`issue.created`,
//! `issue.transitioned`, `issue.commented`, `board.changed`, ...) from the
//! files each commit touched. Because the events come from Git, they survive
//! `orbit board sync` merges across clones: a merge commit diffs against its
//! first parent and replays the host-side changes as events of that commit.
//!
//! **Cursor.** The monotonic cursor is a commit hash: `changes` reports the
//! Board head as `cursor`, and `--since <cursor>` replays only commits
//! reachable from `HEAD` but not from the cursor (that is, strictly newer
//! commits). A cursor that is not an ancestor of `HEAD` — rewritten history —
//! is rejected so a consumer never silently skips or repeats events.
//!
//! **Actor.** Resolved per event in this order:
//!
//! 1. the commit's `Orbit-Actor:` trailer, which lifecycle mutations
//!    (`claim`, `assign`, `release`, `heartbeat`, `checkpoint`, `transition`,
//!    `done`, `cancel`) record from the caller's `--by`/agent identity;
//! 2. the Issue's own record at that commit — `reporter` for `issue.created`,
//!    the newest comment's author for `issue.commented`, the assignee for
//!    assignee/status changes, `checkpoint.by` for checkpoint writes;
//! 3. the commit's Git author name.
//!
//! **Trace id.** Every event carries `trace_id` = the commit hash that
//! produced it, so all events of one commit (for example a `batch`) share a
//! correlation id. `id` is `<commit>:<ordinal>` and is unique per event.

use std::path::Path;

use serde::Serialize;
use serde_json::{Value, json};

use crate::board::Board;
use crate::error::{OrbitError, Result};
use crate::git;

/// Git trailer lifecycle mutations write so the feed knows the Orbit actor.
pub const ACTOR_TRAILER: &str = "Orbit-Actor";

/// One named event derived from a Board commit.
#[derive(Debug, Clone, Serialize)]
pub struct BoardEvent {
    /// Unique event id: `<commit hash>:<ordinal within the commit>`.
    pub id: String,
    /// Event kind, for example `issue.transitioned` or `board.changed`.
    #[serde(rename = "type")]
    pub kind: String,
    /// The Issue's internal id (the task filename stem) at that commit.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub issue: Option<String>,
    /// The Issue's project key (`ORB-12`) when the file recorded one.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub key: Option<String>,
    /// Who caused the event (see the module docs for resolution order).
    pub actor: String,
    /// Correlation id shared by every event one commit produced: the commit.
    pub trace_id: String,
    /// The Board commit this event was derived from.
    pub commit: String,
    /// Commit timestamp (strict ISO 8601).
    pub at: String,
    /// The commit's Orbit verb (`claim`, `add`, a transition name, ...).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub verb: Option<String>,
    /// Kind-specific detail: status/assignee transitions, changed paths, ...
    pub data: Value,
}

/// The result of [`collect`]: the events plus the cursor to resume from.
pub struct Feed {
    pub events: Vec<BoardEvent>,
    /// The Board head commit; pass to `changes --since` for the next delta.
    pub cursor: String,
}

#[derive(Debug)]
struct CommitInfo {
    hash: String,
    author: String,
    time: String,
    subject: String,
    body: String,
    parents: Vec<String>,
}

impl CommitInfo {
    /// The `Orbit-Actor` trailer value, when the commit recorded one.
    fn trailer_actor(&self) -> Option<&str> {
        // Git trailers live in the last paragraph of the message; a line in the
        // free-text body must not be able to claim an actor.
        let trailers = self.body.trim_end().rsplit("\n\n").next().unwrap_or("");
        trailers
            .lines()
            .filter_map(|line| line.trim().strip_prefix(ACTOR_TRAILER))
            .filter_map(|rest| rest.strip_prefix(':'))
            .map(str::trim)
            .find(|value| !value.is_empty())
    }
}

/// Replays Board history into events. `since` is an optional cursor commit;
/// only commits after it are replayed. Events come back oldest first.
pub fn collect(board: &Board, since: Option<&str>) -> Result<Feed> {
    let root = board.root.as_path();
    let head = git::rev_parse(root, "HEAD")?
        .ok_or_else(|| OrbitError::git("the Board has no commits yet"))?;
    let range = match since {
        Some(since) => {
            let cursor = git::rev_parse(root, since)?
                .ok_or_else(|| invalid_cursor(since, "not a commit on this Board"))?;
            if cursor != head {
                let ancestor = git::merge_base(root, &cursor, &head)?;
                if ancestor.as_deref() != Some(cursor.as_str()) {
                    return Err(invalid_cursor(
                        since,
                        "it is not an ancestor of the Board head; the Board history was likely rewritten — rerun without --since",
                    ));
                }
            }
            format!("{cursor}..{head}")
        }
        None => head.clone(),
    };

    let commits = log_commits(root, &range)?;
    let mut events = Vec::new();
    for info in &commits {
        events.extend(events_for_commit(root, info)?);
    }
    Ok(Feed {
        events,
        cursor: head,
    })
}

fn invalid_cursor(since: &str, why: &str) -> OrbitError {
    OrbitError::invalid_argument(format!(
        "invalid --since cursor `{since}`: {why}; pass the `cursor` value a previous `orbit changes` returned"
    ))
}

fn git_text(root: &Path, args: &[&str]) -> Result<String> {
    git::git_output(root, args).map_err(OrbitError::git)
}

/// `git log` records separated by \x1e, fields by \x1f:
/// hash, author name, author date (strict ISO), parents, subject, body.
/// `--first-parent` keeps the Board's own chain: a `board sync` merge commit
/// carries the host-side delta in its diff against the first parent, while
/// the host commits themselves stay off the first-parent chain — without the
/// flag every synced change would emit its events twice.
fn log_commits(root: &Path, range: &str) -> Result<Vec<CommitInfo>> {
    let format = "%x1e%H%x1f%an%x1f%aI%x1f%P%x1f%s%x1f%b";
    let out = git_text(
        root,
        &[
            "log",
            "--reverse",
            "--first-parent",
            &format!("--format={format}"),
            range,
        ],
    )?;
    let mut commits = Vec::new();
    for record in out.split('\x1e') {
        let record = record.trim_matches('\n');
        if record.trim().is_empty() {
            continue;
        }
        let mut fields = record.split('\x1f');
        let (Some(hash), Some(author), Some(time), Some(parents), Some(subject)) = (
            fields.next(),
            fields.next(),
            fields.next(),
            fields.next(),
            fields.next(),
        ) else {
            continue;
        };
        commits.push(CommitInfo {
            hash: hash.trim().to_string(),
            author: author.trim().to_string(),
            time: time.trim().to_string(),
            parents: parents.split_whitespace().map(str::to_string).collect(),
            subject: subject.trim_end().to_string(),
            body: fields.collect::<Vec<_>>().join("\x1f"),
        });
    }
    Ok(commits)
}

/// One changed path in a commit: status letter plus the new path.
struct Change {
    status: char,
    path: String,
}

/// Diffs a commit against its first parent — for a merge commit that is the
/// net change the merge brought in, so host-side edits replay as events.
/// A root commit diffs against the empty tree.
fn changed_files(root: &Path, info: &CommitInfo) -> Result<Vec<Change>> {
    let args: Vec<String> = if let Some(parent) = info.parents.first() {
        vec![
            "diff".into(),
            "--no-renames".into(),
            "--name-status".into(),
            "-z".into(),
            parent.clone(),
            info.hash.clone(),
        ]
    } else {
        vec![
            "diff-tree".into(),
            "--root".into(),
            "--no-commit-id".into(),
            "-r".into(),
            "-z".into(),
            "--name-status".into(),
            info.hash.clone(),
        ]
    };
    let arg_refs: Vec<&str> = args.iter().map(String::as_str).collect();
    let out = git_text(root, &arg_refs)?;
    Ok(parse_name_status_z(&out))
}

/// Parses `git diff --name-status -z`: `STATUS\0path\0`, renames/copies add a
/// second path (`R100\0old\0new\0`) of which the last is the new path.
fn parse_name_status_z(out: &str) -> Vec<Change> {
    let mut tokens = out.split('\0').filter(|token| !token.is_empty());
    let mut changes = Vec::new();
    while let Some(status) = tokens.next() {
        let Some(kind) = status.chars().next() else {
            continue;
        };
        let copies_two = matches!(kind, 'R' | 'C');
        let mut path = None;
        for _ in 0..(if copies_two { 2 } else { 1 }) {
            if let Some(next) = tokens.next() {
                path = Some(next.to_string());
            }
        }
        if let Some(path) = path {
            changes.push(Change { status: kind, path });
        }
    }
    changes
}

/// The commit's Orbit verb and detail: `orbit: claim orb-x` → (`claim`,
/// `orb-x`). Non-Orbit subjects produce no events.
fn orbit_verb(subject: &str) -> Option<(String, String)> {
    let rest = subject.strip_prefix("orbit:")?.trim();
    let (verb, detail) = rest.split_once(char::is_whitespace).unwrap_or((rest, ""));
    if verb.is_empty() {
        return None;
    }
    Some((verb.to_string(), detail.trim().to_string()))
}

fn events_for_commit(root: &Path, info: &CommitInfo) -> Result<Vec<BoardEvent>> {
    let Some((verb, detail)) = orbit_verb(&info.subject) else {
        return Ok(Vec::new());
    };
    let changes = changed_files(root, info)?;
    let mut events = Vec::new();
    let mut board_paths: Vec<String> = Vec::new();

    for change in &changes {
        let path = change.path.as_str();
        if let Some(id) = task_file_id(path) {
            events.push(task_event(root, info, &verb, &detail, id, change.status)?);
        } else if let Some(id) = comment_file_id(path) {
            events.push(comment_event(
                root,
                info,
                &verb,
                &detail,
                id,
                change.status,
            )?);
        } else {
            board_paths.push(path.to_string());
        }
    }
    if !board_paths.is_empty() {
        board_paths.sort();
        board_paths.dedup();
        events.push(BoardEvent {
            id: String::new(),
            kind: "board.changed".to_string(),
            issue: None,
            key: None,
            actor: actor_for(info, "board.changed", None, None, None),
            trace_id: info.hash.clone(),
            commit: info.hash.clone(),
            at: info.time.clone(),
            verb: Some(verb.clone()),
            data: json!({
                "paths": board_paths,
                "detail": detail,
            }),
        });
    }
    for (ordinal, event) in events.iter_mut().enumerate() {
        event.id = format!("{}:{ordinal}", info.hash);
    }
    Ok(events)
}

/// `tasks/<id>.md` → `<id>`; anything else under `tasks/` is not a Task.
fn task_file_id(path: &str) -> Option<&str> {
    let name = path.strip_prefix("tasks/")?;
    name.strip_suffix(".md")
        .filter(|id| !id.is_empty() && !id.starts_with('.'))
}

/// `comments/<id>.md` → `<id>`.
fn comment_file_id(path: &str) -> Option<&str> {
    let name = path.strip_prefix("comments/")?;
    name.strip_suffix(".md")
        .filter(|id| !id.is_empty() && !id.starts_with('.'))
}

/// The fields of a Task file the feed compares between revisions. Read with a
/// lenient line scanner — history may hold revisions the current schema
/// rejects, and the feed must still replay them.
#[derive(Debug, Default, Clone)]
struct TaskSnapshot {
    title: Option<String>,
    status: Option<String>,
    key: Option<String>,
    reporter: Option<String>,
    assignee: Option<String>,
    checkpoint_by: Option<String>,
}

fn snapshot(content: Option<String>) -> TaskSnapshot {
    let Some(content) = content else {
        return TaskSnapshot::default();
    };
    let mut snap = TaskSnapshot::default();
    let mut in_assignee = false;
    let mut in_checkpoint = false;
    let mut in_frontmatter = false;
    for line in content.lines() {
        let trimmed = line.trim_end();
        if !in_frontmatter {
            if trimmed == "---" {
                in_frontmatter = true;
            }
            continue;
        }
        if trimmed == "---" {
            break;
        }
        let top_level = !line.starts_with(' ') && !line.starts_with('\t');
        if top_level {
            in_assignee = trimmed.starts_with("assignee:");
            in_checkpoint = trimmed.starts_with("checkpoint:");
            if let Some(value) = scalar_value(trimmed, "title") {
                snap.title = Some(value);
            } else if let Some(value) = scalar_value(trimmed, "status") {
                snap.status = Some(value);
            } else if let Some(value) = scalar_value(trimmed, "key") {
                snap.key = Some(value.to_uppercase());
            } else if let Some(value) = scalar_value(trimmed, "reporter") {
                snap.reporter = Some(value);
            }
            continue;
        }
        if in_assignee
            && snap.assignee.is_none()
            && let Some(value) = scalar_value(trimmed, "name")
        {
            snap.assignee = Some(value);
        }
        if in_checkpoint
            && snap.checkpoint_by.is_none()
            && let Some(value) = scalar_value(trimmed, "by")
        {
            snap.checkpoint_by = Some(value);
        }
    }
    snap
}

/// `key: value` → the unquoted value; `None` when the line is not `key:` or
/// the value is empty.
fn scalar_value(line: &str, key: &str) -> Option<String> {
    let rest = line.trim_start().strip_prefix(key)?.strip_prefix(':')?;
    let value = rest.trim();
    if value.is_empty() {
        return None;
    }
    let value = value
        .strip_prefix('"')
        .and_then(|v| v.strip_suffix('"'))
        .or_else(|| value.strip_prefix('\'').and_then(|v| v.strip_suffix('\'')))
        .unwrap_or(value);
    Some(value.to_string())
}

fn task_event(
    root: &Path,
    info: &CommitInfo,
    verb: &str,
    detail: &str,
    id: &str,
    status: char,
) -> Result<BoardEvent> {
    let path = format!("tasks/{id}.md");
    let parent = format!("{}^", info.hash);
    let (old, new) = match status {
        'A' => (
            TaskSnapshot::default(),
            snapshot(git::show_file(root, &info.hash, &path)?),
        ),
        'D' => (
            snapshot(git::show_file(root, &parent, &path)?),
            TaskSnapshot::default(),
        ),
        _ => (
            snapshot(git::show_file(root, &parent, &path)?),
            snapshot(git::show_file(root, &info.hash, &path)?),
        ),
    };

    let (kind, mut data) = classify(&old, &new, status);
    data["verb"] = json!(verb);
    if !detail.is_empty() {
        data["detail"] = json!(detail);
    }

    Ok(BoardEvent {
        id: String::new(), // ordinal assigned by the caller
        kind: kind.to_string(),
        issue: Some(id.to_string()),
        key: new.key.clone().or(old.key.clone()),
        actor: actor_for(info, kind, Some(&old), Some(&new), None),
        trace_id: info.hash.clone(),
        commit: info.hash.clone(),
        at: info.time.clone(),
        verb: Some(verb.to_string()),
        data,
    })
}

/// Picks the event kind for a Task file change, most significant first, and
/// collects every detected field change into `data`.
fn classify(old: &TaskSnapshot, new: &TaskSnapshot, status: char) -> (&'static str, Value) {
    let mut data = serde_json::Map::new();
    match status {
        'A' => {
            if let Some(title) = &new.title {
                data.insert("title".into(), json!(title));
            }
            if let Some(status) = &new.status {
                data.insert("status".into(), json!(status));
            }
            return ("issue.created", Value::Object(data));
        }
        'D' => {
            if let Some(title) = &old.title {
                data.insert("title".into(), json!(title));
            }
            return ("issue.deleted", Value::Object(data));
        }
        'M' => {}
        // Renames, type changes, and copies carry no field semantics.
        _ => return ("issue.updated", Value::Object(data)),
    }
    if old.title != new.title
        && let (Some(from), Some(to)) = (&old.title, &new.title)
    {
        data.insert("title".into(), json!({ "from": from, "to": to }));
    }
    if old.status != new.status
        && let (Some(from), Some(to)) = (&old.status, &new.status)
    {
        data.insert("status".into(), json!({ "from": from, "to": to }));
    }
    if old.assignee != new.assignee {
        data.insert(
            "assignee".into(),
            json!({ "from": old.assignee, "to": new.assignee }),
        );
    }
    if old.checkpoint_by != new.checkpoint_by {
        data.insert(
            "checkpoint_by".into(),
            json!({ "from": old.checkpoint_by, "to": new.checkpoint_by }),
        );
    }
    if data.contains_key("status") {
        ("issue.transitioned", Value::Object(data))
    } else if data.contains_key("assignee") {
        ("issue.assigned", Value::Object(data))
    } else if data.contains_key("checkpoint_by") {
        ("issue.checkpointed", Value::Object(data))
    } else {
        ("issue.updated", Value::Object(data))
    }
}

fn comment_event(
    root: &Path,
    info: &CommitInfo,
    verb: &str,
    detail: &str,
    id: &str,
    status: char,
) -> Result<BoardEvent> {
    let path = format!("comments/{id}.md");
    let new_content = if status == 'D' {
        None
    } else {
        git::show_file(root, &info.hash, &path)?
    };
    let author = new_content
        .as_deref()
        .and_then(|content| crate::comment::parse_comments(content).ok())
        .and_then(|comments| comments.last().map(|comment| comment.author.clone()));
    // The comments sidecar carries no key; read the Task's project key at
    // this commit so `history <KEY>` finds comment events too.
    let key = git::show_file(root, &info.hash, &format!("tasks/{id}.md"))?
        .and_then(|content| snapshot(Some(content)).key);
    let kind = if status == 'D' {
        "issue.comments_deleted"
    } else {
        "issue.commented"
    };
    Ok(BoardEvent {
        id: String::new(),
        kind: kind.to_string(),
        issue: Some(id.to_string()),
        key,
        actor: actor_for(info, kind, None, None, author.as_deref()),
        trace_id: info.hash.clone(),
        commit: info.hash.clone(),
        at: info.time.clone(),
        verb: Some(verb.to_string()),
        data: json!({
            "verb": verb,
            "detail": detail,
            "author": author,
        }),
    })
}

/// Resolves the event actor: `Orbit-Actor` trailer, then the Issue's own
/// record, then the commit's Git author.
fn actor_for(
    info: &CommitInfo,
    kind: &str,
    old: Option<&TaskSnapshot>,
    new: Option<&TaskSnapshot>,
    comment_author: Option<&str>,
) -> String {
    if let Some(actor) = info.trailer_actor() {
        return actor.to_string();
    }
    let inferred = match kind {
        "issue.created" => new.and_then(|snap| snap.reporter.clone()),
        "issue.commented" | "issue.comments_deleted" => comment_author.map(str::to_string),
        "issue.assigned" | "issue.transitioned" | "issue.checkpointed" => new
            .and_then(|snap| snap.assignee.clone())
            .or_else(|| old.and_then(|snap| snap.assignee.clone())),
        _ => None,
    };
    if let Some(actor) = inferred.filter(|actor| !actor.trim().is_empty()) {
        return actor;
    }
    if !info.author.trim().is_empty() {
        return info.author.trim().to_string();
    }
    "unknown".to_string()
}

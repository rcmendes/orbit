//! Review, evidence, and escalation state (ADR 0022).
//!
//! Orbit stores review bookkeeping inside the reserved `orbit` Property —
//! the same namespace the policy engine uses for per-rule attempt counters —
//! and submit evidence under the `evidence` Property owner, which policy
//! rules can require with `require_properties`. Nothing here is interpreted
//! by readiness except the open Ask, which blocks the Issue until answered.
//!
//! Shapes (all inside JSON Properties, so they never touch frontmatter):
//!
//! - `evidence`: `{"items": [{"by", "kind", "at", "commands": [...],
//!   "exit_code", "commit", "files": [...], "note"}]}` — appended by
//!   `orbit submit`, newest last.
//! - `orbit.review`: `{"submitted_by", "submitted_kind", "submitted_at",
//!   "rounds", "verdict", "reason", "reviewed_by", "reviewed_kind",
//!   "reviewed_at"}` — the latest submit and verdict.
//! - `orbit.ask`: `{"question", "by", "at", "return_to", "answer",
//!   "answered_by", "answered_at"}` — at most one outstanding Ask; an Ask
//!   without `answered_at` blocks readiness, claim, `done`, and the review
//!   transitions until `orbit answer` records the reply.
//! - `orbit.escalation`: `{"reason", "transition", "rules", "by", "at"}` —
//!   set when a governed transition hits a policy attempt limit and the
//!   workflow's `escalate` edge reroutes the Issue instead of failing.

use chrono::{DateTime, Utc};
use serde_json::{Map, Value, json};

use crate::error::{OrbitError, Result};
use crate::task::{IdentityKind, Task};

/// Property owner holding submit evidence items.
pub const EVIDENCE_OWNER: &str = "evidence";
/// Property owner Orbit reserves for its own bookkeeping (`attempts`,
/// `review`, `ask`, `escalation`).
pub const ORBIT_OWNER: &str = "orbit";

/// One recorded piece of submit evidence.
#[derive(Debug, Clone, Default)]
pub struct Evidence {
    pub commands: Vec<String>,
    pub exit_code: Option<i64>,
    pub commit: Option<String>,
    pub files: Vec<String>,
    pub note: Option<String>,
}

impl Evidence {
    /// At least one evidence field must be present: `submit` exists to
    /// record proof, not to move status silently.
    pub fn is_empty(&self) -> bool {
        self.commands.is_empty()
            && self.exit_code.is_none()
            && self.commit.is_none()
            && self.files.is_empty()
            && self.note.is_none()
    }
}

fn orbit_map(task: &Task) -> Map<String, Value> {
    task.front
        .properties
        .get(ORBIT_OWNER)
        .and_then(Value::as_object)
        .cloned()
        .unwrap_or_default()
}

fn write_orbit(task: &mut Task, orbit: Map<String, Value>) {
    let value = Value::Object(orbit);
    // The `orbit` owner is Orbit's own bookkeeping; shapes written here are
    // small and always valid objects.
    if crate::task::validate_property(ORBIT_OWNER, &value).is_ok() {
        task.front.properties.insert(ORBIT_OWNER.to_string(), value);
    }
}

/// The `orbit` Property object, or an empty map when absent.
pub fn orbit_property(task: &Task) -> Map<String, Value> {
    orbit_map(task)
}

/// The name of the Task's author for reviewer-distinctness checks: the last
/// recorded submitter, falling back to the reporter when the work never
/// went through `orbit submit`.
pub fn author_name(task: &Task) -> Option<String> {
    orbit_map(task)
        .get("review")
        .and_then(|review| review.get("submitted_by"))
        .and_then(Value::as_str)
        .map(str::to_string)
        .or_else(|| task.front.reporter.clone())
}

/// The latest review record (`orbit.review`), when a submit or verdict was
/// recorded.
pub fn review_record(task: &Task) -> Option<Value> {
    orbit_map(task).get("review").cloned()
}

/// The recorded submitter's name, when `orbit submit` ran.
pub fn submitted_by(task: &Task) -> Option<String> {
    review_record(task).and_then(|review| review.get("submitted_by")?.as_str().map(str::to_string))
}

/// The recorded submitter's identity kind (`agent` default).
pub fn submitted_kind(task: &Task) -> IdentityKind {
    let kind = review_record(task)
        .and_then(|review| review.get("submitted_kind")?.as_str().map(str::to_string));
    if kind.as_deref() == Some("human") {
        IdentityKind::Human
    } else {
        IdentityKind::Agent
    }
}

/// Records a submit: bumps the round counter and stores who submitted and
/// when. The assignee is intentionally left untouched — `in_review` is an
/// unassigned review queue.
pub fn record_submit(task: &mut Task, by: &str, kind: IdentityKind, now: DateTime<Utc>) {
    let mut orbit = orbit_map(task);
    let mut review = orbit
        .get("review")
        .and_then(Value::as_object)
        .cloned()
        .unwrap_or_default();
    let rounds = review.get("rounds").and_then(Value::as_u64).unwrap_or(0) + 1;
    review.insert("submitted_by".to_string(), json!(by));
    review.insert("submitted_kind".to_string(), json!(kind.as_str()));
    review.insert("submitted_at".to_string(), json!(now.to_rfc3339()));
    review.insert("rounds".to_string(), json!(rounds));
    // A new submit supersedes the previous verdict.
    for key in [
        "verdict",
        "reason",
        "reviewed_by",
        "reviewed_kind",
        "reviewed_at",
    ] {
        review.remove(key);
    }
    orbit.insert("review".to_string(), Value::Object(review));
    write_orbit(task, orbit);
}

/// The verdict names `orbit review` accepts, mapped to workflow transition
/// names by the caller.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Verdict {
    Approve,
    RequestChanges,
}

impl Verdict {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Approve => "approve",
            Self::RequestChanges => "request-changes",
        }
    }

    pub fn transition(self) -> &'static str {
        match self {
            Self::Approve => crate::workflow::TRANSITION_APPROVE,
            Self::RequestChanges => crate::workflow::TRANSITION_REQUEST_CHANGES,
        }
    }
}

/// Records a review verdict on `orbit.review`.
pub fn record_verdict(
    task: &mut Task,
    verdict: Verdict,
    reason: Option<&str>,
    by: &str,
    kind: IdentityKind,
    now: DateTime<Utc>,
) {
    let mut orbit = orbit_map(task);
    let mut review = orbit
        .get("review")
        .and_then(Value::as_object)
        .cloned()
        .unwrap_or_default();
    review.insert("verdict".to_string(), json!(verdict.as_str()));
    match reason {
        Some(reason) => {
            review.insert("reason".to_string(), json!(reason));
        }
        None => {
            review.remove("reason");
        }
    }
    review.insert("reviewed_by".to_string(), json!(by));
    review.insert("reviewed_kind".to_string(), json!(kind.as_str()));
    review.insert("reviewed_at".to_string(), json!(now.to_rfc3339()));
    orbit.insert("review".to_string(), Value::Object(review));
    write_orbit(task, orbit);
}

/// Appends one evidence item under the `evidence` Property owner.
pub fn append_evidence(
    task: &mut Task,
    by: &str,
    kind: IdentityKind,
    evidence: &Evidence,
    now: DateTime<Utc>,
) -> Result<()> {
    let mut value = task
        .front
        .properties
        .get(EVIDENCE_OWNER)
        .and_then(Value::as_object)
        .cloned()
        .unwrap_or_default();
    let mut items = value
        .get("items")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    let mut item = Map::new();
    item.insert("by".to_string(), json!(by));
    item.insert("kind".to_string(), json!(kind.as_str()));
    item.insert("at".to_string(), json!(now.to_rfc3339()));
    if !evidence.commands.is_empty() {
        item.insert("commands".to_string(), json!(evidence.commands));
    }
    if let Some(exit_code) = evidence.exit_code {
        item.insert("exit_code".to_string(), json!(exit_code));
    }
    if let Some(commit) = &evidence.commit {
        item.insert("commit".to_string(), json!(commit));
    }
    if !evidence.files.is_empty() {
        item.insert("files".to_string(), json!(evidence.files));
    }
    if let Some(note) = &evidence.note {
        item.insert("note".to_string(), json!(note));
    }
    items.push(Value::Object(item));
    value.insert("items".to_string(), Value::Array(items));
    let value = Value::Object(value);
    crate::task::validate_property(EVIDENCE_OWNER, &value)?;
    task.front
        .properties
        .insert(EVIDENCE_OWNER.to_string(), value);
    Ok(())
}

/// The open Ask (`orbit.ask` without `answered_at`), when one exists.
pub fn open_ask(task: &Task) -> Option<Value> {
    let ask = orbit_map(task).get("ask")?.clone();
    ask.get("answered_at").is_none().then_some(ask)
}

/// Whether the Task has an unanswered Ask — the readiness/claim/close block.
pub fn has_open_ask(task: &Task) -> bool {
    open_ask(task).is_some()
}

/// Records an Ask on `orbit.ask`, returning the status the Issue should
/// return to when answered. Refuses a second open Ask.
pub fn record_ask(
    task: &mut Task,
    question: &str,
    by: &str,
    return_to: &str,
    now: DateTime<Utc>,
) -> Result<()> {
    if has_open_ask(task) {
        return Err(OrbitError::new(
            crate::error::code::ASK_OPEN,
            format!(
                "{} already has an open ask; answer it with `orbit answer` first",
                task.id
            ),
        ));
    }
    let mut orbit = orbit_map(task);
    let mut ask = Map::new();
    ask.insert("question".to_string(), json!(question));
    ask.insert("by".to_string(), json!(by));
    ask.insert("at".to_string(), json!(now.to_rfc3339()));
    ask.insert("return_to".to_string(), json!(return_to));
    orbit.insert("ask".to_string(), Value::Object(ask));
    write_orbit(task, orbit);
    Ok(())
}

/// The `return_to` status recorded on the open Ask, when present and still
/// a usable open status name.
pub fn ask_return_to(task: &Task) -> Option<String> {
    open_ask(task)?
        .get("return_to")
        .and_then(Value::as_str)
        .map(str::to_string)
}

/// Marks the open Ask answered, returning the question that was answered.
pub fn answer_ask(task: &mut Task, answer: &str, by: &str, now: DateTime<Utc>) -> Result<String> {
    let question = open_ask(task)
        .and_then(|ask| ask.get("question")?.as_str().map(str::to_string))
        .ok_or_else(|| {
            OrbitError::invalid_argument(format!("{} has no open ask to answer", task.id))
        })?;
    let mut orbit = orbit_map(task);
    if let Some(ask) = orbit.get_mut("ask").and_then(Value::as_object_mut) {
        ask.insert("answer".to_string(), json!(answer));
        ask.insert("answered_by".to_string(), json!(by));
        ask.insert("answered_at".to_string(), json!(now.to_rfc3339()));
    }
    write_orbit(task, orbit);
    Ok(question)
}

/// Records an attempt-limit escalation on `orbit.escalation`.
pub fn record_escalation(
    task: &mut Task,
    reason: &str,
    transition: &str,
    rules: &[String],
    by: &str,
    now: DateTime<Utc>,
) {
    let mut orbit = orbit_map(task);
    orbit.insert(
        "escalation".to_string(),
        json!({
            "reason": reason,
            "transition": transition,
            "rules": rules,
            "by": by,
            "at": now.to_rfc3339(),
        }),
    );
    write_orbit(task, orbit);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::task::TaskFrontmatter;
    use chrono::TimeZone;
    use std::collections::BTreeMap;

    fn now() -> DateTime<Utc> {
        Utc.with_ymd_and_hms(2026, 11, 16, 12, 0, 0).unwrap()
    }

    fn task() -> Task {
        Task {
            id: "orb-aaaaaa".to_string(),
            front: TaskFrontmatter {
                title: "t".to_string(),
                status: "in_progress".to_string(),
                key: None,
                key_aliases: Vec::new(),
                resolution: None,
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
                reporter: Some("alice".to_string()),
                created_at: now(),
                updated_at: now(),
                source_key: None,
                properties: BTreeMap::new(),
                extra: BTreeMap::new(),
            },
            body: String::new(),
        }
    }

    #[test]
    fn submit_records_evidence_and_review_state() {
        let mut t = task();
        append_evidence(
            &mut t,
            "devin",
            IdentityKind::Agent,
            &Evidence {
                commands: vec!["cargo test".to_string()],
                exit_code: Some(0),
                commit: Some("abc123".to_string()),
                files: vec!["src/lib.rs".to_string()],
                note: None,
            },
            now(),
        )
        .unwrap();
        record_submit(&mut t, "devin", IdentityKind::Agent, now());

        let evidence = &t.front.properties["evidence"];
        assert_eq!(evidence["items"][0]["exit_code"], 0);
        assert_eq!(evidence["items"][0]["commit"], "abc123");
        assert_eq!(author_name(&t).as_deref(), Some("devin"));
        assert_eq!(submitted_by(&t).as_deref(), Some("devin"));
    }

    #[test]
    fn ask_blocks_until_answered() {
        let mut t = task();
        record_ask(&mut t, "which env?", "devin", "in_progress", now()).unwrap();
        assert!(has_open_ask(&t));
        assert_eq!(ask_return_to(&t).as_deref(), Some("in_progress"));
        // A second open ask is refused.
        assert!(record_ask(&mut t, "again?", "devin", "in_progress", now()).is_err());
        answer_ask(&mut t, "staging", "alice", now()).unwrap();
        assert!(!has_open_ask(&t));
        assert!(answer_ask(&mut t, "x", "alice", now()).is_err());
    }

    #[test]
    fn author_falls_back_to_reporter() {
        let t = task();
        assert_eq!(author_name(&t).as_deref(), Some("alice"));
    }
}

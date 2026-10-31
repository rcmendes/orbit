use chrono::{DateTime, Utc};
use clap::ValueEnum;
use rand::Rng;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

use crate::error::{OrbitError, Result};
use crate::github::link::GitHubLink;

pub const ID_ALPHABET: &[u8] = b"0123456789abcdefghjkmnpqrstvwxyz";
pub const DEFAULT_PREFIX: &str = "orb-";
pub const DEFAULT_ID_LENGTH: usize = 6;
pub const MAX_LABEL_LENGTH: usize = 32;
pub const MAX_VERIFICATION_LENGTH: usize = 2000;

/// Stable classification of where work primarily lands in the repository.
/// Shared metadata for agents and filters; not personal or session-local state.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ValueEnum)]
#[serde(rename_all = "snake_case")]
pub enum TaskArea {
    Board,
    Cli,
    Mcp,
    Setup,
    Docs,
    Release,
}

impl TaskArea {
    pub fn as_str(self) -> &'static str {
        match self {
            TaskArea::Board => "board",
            TaskArea::Cli => "cli",
            TaskArea::Mcp => "mcp",
            TaskArea::Setup => "setup",
            TaskArea::Docs => "docs",
            TaskArea::Release => "release",
        }
    }

    pub fn all() -> &'static [TaskArea] {
        &[
            TaskArea::Board,
            TaskArea::Cli,
            TaskArea::Mcp,
            TaskArea::Setup,
            TaskArea::Docs,
            TaskArea::Release,
        ]
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ValueEnum)]
#[serde(rename_all = "snake_case")]
pub enum Status {
    Todo,
    #[value(name = "in_progress", alias = "in-progress")]
    InProgress,
    Done,
    Cancelled,
}

impl Status {
    pub fn is_terminal(self) -> bool {
        matches!(self, Status::Done | Status::Cancelled)
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Status::Todo => "todo",
            Status::InProgress => "in_progress",
            Status::Done => "done",
            Status::Cancelled => "cancelled",
        }
    }
}

/// Structural shape of a Task. `kind` remains derived (`task` or `subtask`);
/// `type` is metadata and does not affect readiness or lifecycle.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ValueEnum, Default)]
#[serde(rename_all = "lowercase")]
pub enum TaskType {
    #[default]
    Task,
    Epic,
    Spike,
    Chore,
    Decision,
}

impl TaskType {
    pub fn as_str(self) -> &'static str {
        match self {
            TaskType::Task => "task",
            TaskType::Epic => "epic",
            TaskType::Spike => "spike",
            TaskType::Chore => "chore",
            TaskType::Decision => "decision",
        }
    }
}

pub fn default_priority() -> u8 {
    2
}

/// Who holds a Task and until when. Times are optional so hand-edited files with a
/// partial claim still load; `orbit doctor` reports them as incomplete.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Claim {
    pub holder: String,
    pub claimed_at: Option<DateTime<Utc>>,
    pub lease_until: Option<DateTime<Utc>>,
}

impl Claim {
    pub fn is_live(&self, now: DateTime<Utc>) -> bool {
        self.lease_until.is_some_and(|until| until > now)
    }

    pub fn is_stale(&self, now: DateTime<Utc>) -> bool {
        self.lease_until.is_some_and(|until| until <= now)
    }

    pub fn has_times(&self) -> bool {
        self.claimed_at.is_some() || self.lease_until.is_some()
    }
}

/// Claim times found in a file without a `claimed_by`. Kept so such hand-edited files
/// rewrite unchanged and `orbit doctor` can still report them.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StrayClaimTimes {
    pub claimed_at: Option<DateTime<Utc>>,
    pub lease_until: Option<DateTime<Utc>>,
}

/// The latest handoff note from the claim holder: where the work stands, what to
/// do next, and what is blocking it. Replaced on each update; retained on the
/// Task as archived history after a terminal transition.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Checkpoint {
    pub by: String,
    pub at: DateTime<Utc>,
    pub progress: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub next: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub blocker: Option<String>,
}

impl Checkpoint {
    pub fn validate(&self) -> std::result::Result<(), String> {
        if self.by.trim().is_empty() {
            return Err("checkpoint author must not be empty".to_string());
        }
        if self.progress.trim().is_empty() {
            return Err("checkpoint progress must not be empty".to_string());
        }
        if self
            .next
            .as_ref()
            .is_some_and(|next| next.trim().is_empty())
        {
            return Err("checkpoint next action must not be empty".to_string());
        }
        if self
            .blocker
            .as_ref()
            .is_some_and(|blocker| blocker.trim().is_empty())
        {
            return Err("checkpoint blocker must not be empty".to_string());
        }
        Ok(())
    }
}

/// On-disk shape: the Claim is stored as three flat keys (see docs/SPEC.md).
#[derive(Serialize, Deserialize)]
struct RawFrontmatter {
    title: String,
    status: Status,
    #[serde(default, rename = "type")]
    task_type: TaskType,
    #[serde(default = "default_priority")]
    priority: u8,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    labels: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    depends_on: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    parent: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    waiting_on: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    checkpoint: Option<Checkpoint>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    area: Option<TaskArea>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    verification: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    github: Option<GitHubLink>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    claimed_by: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    claimed_at: Option<DateTime<Utc>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    lease_until: Option<DateTime<Utc>>,
    created_at: DateTime<Utc>,
    updated_at: DateTime<Utc>,
    #[serde(flatten)]
    extra: BTreeMap<String, serde_yaml_ng::Value>,
}

impl From<RawFrontmatter> for TaskFrontmatter {
    fn from(raw: RawFrontmatter) -> Self {
        let (claimed_at, lease_until) = (raw.claimed_at, raw.lease_until);
        let claim = raw.claimed_by.map(|holder| Claim {
            holder,
            claimed_at,
            lease_until,
        });
        let stray_claim_times = (claim.is_none()
            && (claimed_at.is_some() || lease_until.is_some()))
        .then_some(StrayClaimTimes {
            claimed_at,
            lease_until,
        });
        Self {
            title: raw.title,
            status: raw.status,
            task_type: raw.task_type,
            priority: raw.priority,
            labels: raw.labels,
            depends_on: raw.depends_on,
            parent: raw.parent,
            waiting_on: raw.waiting_on,
            checkpoint: raw.checkpoint,
            area: raw.area,
            verification: raw.verification,
            github: raw.github,
            claim,
            stray_claim_times,
            created_at: raw.created_at,
            updated_at: raw.updated_at,
            extra: raw.extra,
        }
    }
}

impl From<TaskFrontmatter> for RawFrontmatter {
    fn from(front: TaskFrontmatter) -> Self {
        let (claimed_by, claimed_at, lease_until) = match (front.claim, front.stray_claim_times) {
            (Some(claim), _) => (Some(claim.holder), claim.claimed_at, claim.lease_until),
            (None, Some(stray)) => (None, stray.claimed_at, stray.lease_until),
            (None, None) => (None, None, None),
        };
        Self {
            title: front.title,
            status: front.status,
            task_type: front.task_type,
            priority: front.priority,
            labels: front.labels,
            depends_on: front.depends_on,
            parent: front.parent,
            waiting_on: front.waiting_on,
            checkpoint: front.checkpoint,
            area: front.area,
            verification: front.verification.clone(),
            github: front.github,
            claimed_by,
            claimed_at,
            lease_until,
            created_at: front.created_at,
            updated_at: front.updated_at,
            extra: front.extra,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(from = "RawFrontmatter", into = "RawFrontmatter")]
pub struct TaskFrontmatter {
    pub title: String,
    pub status: Status,
    pub task_type: TaskType,
    pub priority: u8,
    pub labels: Vec<String>,
    pub depends_on: Vec<String>,
    pub parent: Option<String>,
    pub waiting_on: Option<String>,
    pub checkpoint: Option<Checkpoint>,
    pub area: Option<TaskArea>,
    pub verification: Option<String>,
    pub github: Option<GitHubLink>,
    pub claim: Option<Claim>,
    pub stray_claim_times: Option<StrayClaimTimes>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    pub extra: BTreeMap<String, serde_yaml_ng::Value>,
}

#[derive(Debug, Clone)]
pub struct Task {
    pub id: String,
    pub front: TaskFrontmatter,
    pub body: String,
}

impl Task {
    pub fn parent_id(&self) -> Option<&str> {
        self.front.parent.as_deref()
    }

    pub fn is_subtask(&self) -> bool {
        self.front.parent.is_some()
    }

    pub fn kind(&self) -> &'static str {
        if self.is_subtask() { "subtask" } else { "task" }
    }

    pub fn task_type(&self) -> TaskType {
        self.front.task_type
    }

    pub fn github_link(&self) -> Option<&GitHubLink> {
        self.front.github.as_ref().filter(|link| link.active)
    }

    pub fn has_labels(&self, labels: &[String]) -> bool {
        labels
            .iter()
            .all(|label| self.front.labels.iter().any(|candidate| candidate == label))
    }

    pub fn has_any_label(&self, labels: &[String]) -> bool {
        labels
            .iter()
            .any(|label| self.front.labels.iter().any(|candidate| candidate == label))
    }

    /// Marks the Task as modified at `now`. Every field mutation ends here so
    /// callers never write `front.updated_at` directly.
    pub fn touch(&mut self, now: DateTime<Utc>) {
        self.front.updated_at = now;
    }

    /// Adds labels, preserving the sorted, deduplicated invariant the parser
    /// establishes. Returns true when the set changed; only then does the
    /// Task record a modification.
    pub fn add_labels(&mut self, labels: &[String], now: DateTime<Utc>) -> bool {
        let before = self.front.labels.clone();
        self.front.labels.extend(labels.iter().cloned());
        self.front.labels.sort();
        self.front.labels.dedup();
        let changed = self.front.labels != before;
        if changed {
            self.touch(now);
        }
        changed
    }

    /// Removes labels. Returns true when the set changed.
    pub fn remove_labels(&mut self, labels: &[String], now: DateTime<Utc>) -> bool {
        let before = self.front.labels.clone();
        self.front.labels.retain(|label| !labels.contains(label));
        let changed = self.front.labels != before;
        if changed {
            self.touch(now);
        }
        changed
    }

    /// Adds a dependency edge. Returns true when the edge is new; an existing
    /// edge leaves the Task untouched.
    pub fn add_dependency(&mut self, dep: &str, now: DateTime<Utc>) -> bool {
        if self.front.depends_on.iter().any(|existing| existing == dep) {
            return false;
        }
        self.front.depends_on.push(dep.to_string());
        self.touch(now);
        true
    }

    /// Removes a dependency edge. Returns true when an edge was removed.
    pub fn remove_dependency(&mut self, dep: &str, now: DateTime<Utc>) -> bool {
        let before = self.front.depends_on.len();
        self.front.depends_on.retain(|existing| existing != dep);
        let changed = self.front.depends_on.len() != before;
        if changed {
            self.touch(now);
        }
        changed
    }
}

pub fn normalize_verification(verification: Option<String>) -> Result<Option<String>> {
    match verification {
        None => Ok(None),
        Some(value) => {
            let value = value.trim().to_string();
            if value.is_empty() {
                return Err(OrbitError::invalid_argument(
                    "verification must not be empty",
                ));
            }
            if value.len() > MAX_VERIFICATION_LENGTH {
                return Err(OrbitError::invalid_argument(format!(
                    "verification must be at most {MAX_VERIFICATION_LENGTH} characters"
                )));
            }
            Ok(Some(value))
        }
    }
}

pub fn parse_task(id: String, content: &str) -> Result<Task> {
    let (frontmatter, body) = split_frontmatter(content).ok_or_else(|| {
        OrbitError::corrupt_task(format!("task {id} has no complete YAML frontmatter block"))
    })?;

    let mut front: TaskFrontmatter = serde_yaml_ng::from_str(frontmatter).map_err(|error| {
        OrbitError::corrupt_task(format!("task {id} has invalid frontmatter: {error}"))
    })?;

    if front.title.trim().is_empty() {
        return Err(OrbitError::corrupt_task(format!(
            "task {id} has an empty title"
        )));
    }
    if front.priority > 3 {
        return Err(OrbitError::corrupt_task(format!(
            "task {id} has priority {}; expected 0-3",
            front.priority
        )));
    }
    if front.depends_on.iter().any(|dep| dep.trim().is_empty()) {
        return Err(OrbitError::corrupt_task(format!(
            "task {id} has an empty dependency id"
        )));
    }
    if front
        .parent
        .as_deref()
        .is_some_and(|parent| parent.trim().is_empty())
    {
        return Err(OrbitError::corrupt_task(format!(
            "task {id} has an empty parent id"
        )));
    }
    if let Some(checkpoint) = &front.checkpoint
        && let Err(reason) = checkpoint.validate()
    {
        return Err(OrbitError::corrupt_task(format!("task {id}: {reason}")));
    }
    if let Some(link) = &front.github {
        link.validate().map_err(|error| {
            OrbitError::corrupt_task(format!(
                "task {id} has an invalid github link: {}",
                error.message
            ))
        })?;
    }

    let mut labels = Vec::with_capacity(front.labels.len());
    for label in &front.labels {
        let normalized = normalize_label(label).map_err(|error| {
            OrbitError::corrupt_task(format!("task {id} has an invalid label: {}", error.message))
        })?;
        labels.push(normalized);
    }
    labels.sort();
    labels.dedup();

    let verification = normalize_verification(front.verification.take()).map_err(|error| {
        OrbitError::corrupt_task(format!(
            "task {id} has invalid verification: {}",
            error.message
        ))
    })?;

    let mut front = front;
    front.labels = labels;
    front.verification = verification;

    Ok(Task {
        id,
        front,
        body: body.to_string(),
    })
}

pub fn render_task(task: &Task) -> Result<String> {
    let mut yaml = serde_yaml_ng::to_string(&task.front).map_err(|error| {
        OrbitError::corrupt_task(format!("task {} could not be serialized: {error}", task.id))
    })?;

    if let Some(stripped) = yaml.strip_prefix("---\n") {
        yaml = stripped.to_string();
    }
    if !yaml.ends_with('\n') {
        yaml.push('\n');
    }

    Ok(format!("---\n{yaml}---\n\n{}", task.body))
}

/// A YAML document delimiter must start at column 0. Indented `---` lines are
/// content, for example a Markdown horizontal rule inside a block scalar such
/// as `github.last_sync.body`.
fn is_frontmatter_delimiter(line: &str) -> bool {
    line.trim_end_matches(['\r', ' ', '\t']) == "---"
}

pub fn split_frontmatter(content: &str) -> Option<(&str, &str)> {
    let content = content.strip_prefix('\u{feff}').unwrap_or(content);
    let first_end = content.find('\n')?;
    let first = &content[..first_end];
    if !is_frontmatter_delimiter(first) {
        return None;
    }

    let mut cursor = first_end + 1;
    while cursor <= content.len() {
        let rest = &content[cursor..];
        let (line, next) = match rest.find('\n') {
            Some(index) => (&rest[..index], cursor + index + 1),
            None => (rest, content.len()),
        };

        if is_frontmatter_delimiter(line) {
            let frontmatter = &content[first_end + 1..cursor];
            let body = &content[next..];
            let body = body
                .strip_prefix("\r\n")
                .or_else(|| body.strip_prefix('\n'))
                .unwrap_or(body);
            return Some((frontmatter, body));
        }

        if next == content.len() {
            break;
        }
        cursor = next;
    }

    None
}

pub fn is_valid_id(id: &str, prefix: &str) -> bool {
    let Some(rest) = id.strip_prefix(prefix) else {
        return false;
    };

    match rest.split_once('.') {
        None => is_valid_rand(rest),
        Some((parent, child)) => {
            !parent.is_empty()
                && !child.is_empty()
                && !child.contains('.')
                && is_valid_rand(parent)
                && is_valid_rand(child)
        }
    }
}

fn is_valid_rand(value: &str) -> bool {
    (6..=8).contains(&value.len()) && value.bytes().all(|byte| ID_ALPHABET.contains(&byte))
}

/// Returns the Parent Task encoded in a SubTask ID, if the ID is valid.
pub fn parent_id_from_id(id: &str, prefix: &str) -> Option<String> {
    let rest = id.strip_prefix(prefix)?;
    let (parent, child) = rest.split_once('.')?;
    if parent.is_empty()
        || child.is_empty()
        || child.contains('.')
        || !is_valid_rand(parent)
        || !is_valid_rand(child)
    {
        return None;
    }
    Some(format!("{prefix}{parent}"))
}

pub fn is_valid_label(label: &str) -> bool {
    !label.is_empty()
        && label.len() <= MAX_LABEL_LENGTH
        && label.chars().all(|character| {
            character.is_ascii_lowercase()
                || character.is_ascii_digit()
                || character == '-'
                || character == ':'
        })
        && label
            .chars()
            .next()
            .is_some_and(|character| character.is_ascii_alphanumeric())
}

pub fn normalize_label(input: &str) -> Result<String> {
    let label = input.trim().to_ascii_lowercase();
    if !is_valid_label(&label) {
        return Err(OrbitError::new(
            crate::error::code::INVALID_LABEL,
            format!(
                "invalid label `{input}`; use lowercase letters, digits, `-`, or `:`, at most {MAX_LABEL_LENGTH} characters"
            ),
        ));
    }
    Ok(label)
}

pub fn generate_id(prefix: &str, len: usize) -> String {
    format!("{prefix}{}", generate_rand(len))
}

pub fn generate_subtask_id(parent_id: &str, len: usize) -> String {
    format!("{parent_id}.{}", generate_rand(len))
}

fn generate_rand(len: usize) -> String {
    let mut rng = rand::rng();
    let mut id = String::with_capacity(len);
    for _ in 0..len {
        let index = rng.random_range(0..ID_ALPHABET.len());
        id.push(ID_ALPHABET[index] as char);
    }
    id
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;

    fn now() -> DateTime<Utc> {
        Utc.with_ymd_and_hms(2026, 10, 23, 12, 0, 0).unwrap()
    }

    fn sample() -> Task {
        Task {
            id: "orb-a7b2c9".to_string(),
            front: TaskFrontmatter {
                title: "Build the CLI router".to_string(),
                status: Status::Todo,
                task_type: TaskType::Task,
                priority: 1,
                labels: Vec::new(),
                depends_on: vec!["orb-f14c02".to_string()],
                parent: None,
                waiting_on: None,
                checkpoint: None,
                area: None,
                verification: None,
                github: None,
                claim: None,
                stray_claim_times: None,
                created_at: now(),
                updated_at: now(),
                extra: BTreeMap::new(),
            },
            body: "Body line.\n\n---\n\nA horizontal rule is not a delimiter.".to_string(),
        }
    }

    #[test]
    fn round_trips_frontmatter_and_body() {
        let task = sample();
        let rendered = render_task(&task).unwrap();
        let parsed = parse_task(task.id.clone(), &rendered).unwrap();

        assert_eq!(parsed.id, task.id);
        assert_eq!(parsed.front.title, task.front.title);
        assert_eq!(parsed.front.status, task.front.status);
        assert_eq!(parsed.front.priority, task.front.priority);
        assert_eq!(parsed.front.depends_on, task.front.depends_on);
        assert_eq!(parsed.body, task.body);
    }

    #[test]
    fn preserves_unknown_frontmatter_fields() {
        let content = r#"---
title: Keep me
status: todo
priority: 2
created_at: 2026-10-23T12:00:00Z
updated_at: 2026-10-23T12:00:00Z
future_field: keep this
---

Body.
"#;
        let task = parse_task("orb-a7b2c9".to_string(), content).unwrap();
        assert_eq!(
            task.front.extra.get("future_field"),
            Some(&serde_yaml_ng::Value::String("keep this".to_string()))
        );

        let rendered = render_task(&task).unwrap();
        assert!(rendered.contains("future_field: keep this"));
    }

    #[test]
    fn ignores_indented_horizontal_rules_inside_frontmatter() {
        let content = r#"---
title: Synced Task
status: todo
type: task
priority: 2
github:
  repo: acme/app
  number: 42
  last_sync:
    at: 2026-10-29T16:09:48.562851Z
    issue_updated_at: 2026-10-29T16:09:48Z
    orbit_updated_at: 2026-10-29T16:09:48.562851Z
    title: Synced Task
    body: |-
      First paragraph.

      ---
      After the rule.
    labels:
    - repo:api
created_at: 2026-10-24T19:30:38.261121Z
updated_at: 2026-10-29T16:09:48.562851Z
---

Body with a horizontal rule.

---

The end.
"#;
        let task = parse_task("orb-a7b2c9".to_string(), content).unwrap();
        assert_eq!(task.front.title, "Synced Task");
        assert_eq!(
            task.front.created_at.to_rfc3339(),
            "2026-10-24T19:30:38.261121+00:00"
        );
        let link = task.front.github.as_ref().unwrap();
        assert_eq!(link.number, 42);
        assert!(link.last_sync.as_ref().unwrap().body.contains("---"));
        assert!(task.body.starts_with("Body with a horizontal rule."));
    }

    #[test]
    fn parses_crlf_files() {
        let content = "---\r\ntitle: Test\r\nstatus: todo\r\npriority: 2\r\ncreated_at: 2026-10-23T12:00:00Z\r\nupdated_at: 2026-10-23T12:00:00Z\r\n---\r\n\r\nBody.\r\n";
        let task = parse_task("orb-a7b2c9".to_string(), content).unwrap();
        assert_eq!(task.front.title, "Test");
        assert_eq!(task.body, "Body.\r\n");
    }

    #[test]
    fn rejects_invalid_priority() {
        let content = r#"---
title: Bad
status: todo
priority: 9
created_at: 2026-10-23T12:00:00Z
updated_at: 2026-10-23T12:00:00Z
---
"#;
        let error = parse_task("orb-a7b2c9".to_string(), content).unwrap_err();
        assert_eq!(error.code, crate::error::code::CORRUPT_TASK);
    }

    #[test]
    fn ids_are_validated() {
        assert!(is_valid_id("orb-a7b2c9", "orb-"));
        assert!(is_valid_id("orb-a7b2c9d", "orb-"));
        assert!(is_valid_id("orb-a7b2c9.d4b3df", "orb-"));
        assert!(is_valid_id("orb-5q7bbn.d4b3df", "orb-"));
        assert!(!is_valid_id("orb-a7b2c", "orb-"));
        assert!(!is_valid_id("orb-a7b2c9i", "orb-"));
        assert!(!is_valid_id("task-a7b2c9", "orb-"));
        assert!(!is_valid_id("orb-a7b2c9.d4b3df.e5f6a7", "orb-"));
        assert!(!is_valid_id("orb-a7b2c9.", "orb-"));
        assert!(!is_valid_id("orb-.d4b3df", "orb-"));
        assert!(!is_valid_id("orb-a7b2c9..d4b3df", "orb-"));
    }

    #[test]
    fn parent_id_from_id_reads_only_one_level() {
        assert_eq!(
            parent_id_from_id("orb-a7b2c9.d4b3df", "orb-"),
            Some("orb-a7b2c9".to_string())
        );
        assert_eq!(
            parent_id_from_id("orb-5q7bbn.d4b3df", "orb-"),
            Some("orb-5q7bbn".to_string())
        );
        assert_eq!(parent_id_from_id("orb-a7b2c9", "orb-"), None);
        assert_eq!(parent_id_from_id("orb-a7b2c9.", "orb-"), None);
        assert_eq!(parent_id_from_id("orb-a7b2c9.d4b3df.e5f6a7", "orb-"), None);
    }

    #[test]
    fn generated_ids_use_the_alphabet() {
        for _ in 0..100 {
            let id = generate_id("orb-", 6);
            assert!(is_valid_id(&id, "orb-"), "{id}");
            let child = generate_subtask_id(&id, 6);
            assert!(child.starts_with(&format!("{id}.")), "{child}");
            assert!(is_valid_id(&child, "orb-"), "{child}");
        }
    }

    #[test]
    fn labels_are_normalized_sorted_and_deduped() {
        let content = r#"---
title: Labels
status: todo
priority: 2
labels:
- Bug
- needs-triage
- bug
created_at: 2026-10-23T12:00:00Z
updated_at: 2026-10-23T12:00:00Z
---

Body.
"#;
        let task = parse_task("orb-a7b2c9".to_string(), content).unwrap();
        assert_eq!(task.front.labels, vec!["bug", "needs-triage"]);

        let rendered = render_task(&task).unwrap();
        assert!(rendered.contains("labels:\n- bug\n- needs-triage"));
    }

    #[test]
    fn invalid_labels_are_rejected() {
        for label in ["has spaces", "UPPER", "-leading", "emoji-✨", ""] {
            assert!(!is_valid_label(label), "{label}");
        }
        assert!(is_valid_label("ready-for-agent"));
        assert!(is_valid_label("bug"));
        assert_eq!(normalize_label("Needs-Info").unwrap(), "needs-info");
    }

    #[test]
    fn claim_round_trips_byte_identical_and_keeps_unknown_fields() {
        let content = "---\ntitle: t\nstatus: in_progress\ntype: task\npriority: 2\nclaimed_by: alice\nclaimed_at: 2026-10-23T12:00:00Z\nlease_until: 2026-10-23T12:30:00Z\ncreated_at: 2026-10-23T12:00:00Z\nupdated_at: 2026-10-23T12:00:00Z\nowner_note: keep me\n---\n\nBody.";
        let task = parse_task("orb-a7b2c9".to_string(), content).unwrap();
        assert_eq!(task.holder(), Some("alice"));
        assert_eq!(render_task(&task).unwrap(), content);
    }

    #[test]
    fn partial_claim_still_loads() {
        let content = "---\ntitle: t\nstatus: in_progress\nclaimed_by: alice\ncreated_at: 2026-10-23T12:00:00Z\nupdated_at: 2026-10-23T12:00:00Z\n---\n\n";
        let task = parse_task("orb-a7b2c9".to_string(), content).unwrap();
        let claim = task.front.claim.unwrap();
        assert!(!claim.has_times() && !claim.is_live(now()) && !claim.is_stale(now()));
    }

    #[test]
    fn stray_claim_times_survive_a_round_trip() {
        let content = "---\ntitle: t\nstatus: todo\ntype: task\npriority: 2\nlease_until: 2026-10-23T12:30:00Z\ncreated_at: 2026-10-23T12:00:00Z\nupdated_at: 2026-10-23T12:00:00Z\n---\n\n";
        let task = parse_task("orb-a7b2c9".to_string(), content).unwrap();
        assert!(task.front.claim.is_none() && task.front.stray_claim_times.is_some());
        assert_eq!(render_task(&task).unwrap(), content);
    }

    #[test]
    fn checkpoint_round_trips_and_preserves_unknown_fields() {
        let content = "---\ntitle: t\nstatus: in_progress\ntype: task\npriority: 2\ncheckpoint:\n  by: alice\n  at: 2026-10-23T12:00:00Z\n  progress: Parser done\n  next: Wire the CLI\n  blocker: Waiting on review\nclaimed_by: alice\nclaimed_at: 2026-10-23T12:00:00Z\nlease_until: 2026-10-23T12:30:00Z\ncreated_at: 2026-10-23T12:00:00Z\nupdated_at: 2026-10-23T12:00:00Z\nowner_note: keep me\n---\n\nBody.";
        let task = parse_task("orb-a7b2c9".to_string(), content).unwrap();
        let checkpoint = task.front.checkpoint.as_ref().unwrap();
        assert_eq!(checkpoint.by, "alice");
        assert_eq!(checkpoint.progress, "Parser done");
        assert_eq!(checkpoint.next.as_deref(), Some("Wire the CLI"));
        assert_eq!(checkpoint.blocker.as_deref(), Some("Waiting on review"));
        assert_eq!(render_task(&task).unwrap(), content);
    }

    #[test]
    fn rejects_a_checkpoint_without_progress_or_author() {
        for checkpoint in [
            "checkpoint:\n  by: alice\n  at: 2026-10-23T12:00:00Z\n  progress: ' '\n",
            "checkpoint:\n  by: ' '\n  at: 2026-10-23T12:00:00Z\n  progress: Done\n",
        ] {
            let content = format!(
                "---\ntitle: t\nstatus: in_progress\n{checkpoint}created_at: 2026-10-23T12:00:00Z\nupdated_at: 2026-10-23T12:00:00Z\n---\n\n"
            );
            let error = parse_task("orb-a7b2c9".to_string(), &content).unwrap_err();
            assert_eq!(error.code, crate::error::code::CORRUPT_TASK);
        }
    }
}

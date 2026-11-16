use chrono::{DateTime, Utc};
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
pub const MAX_REPOSITORY_LENGTH: usize = 128;

/// The Issue type assumed when a file has no `type:` key. Boards configure
/// the real vocabulary through `board.types`; `task` is only the fallback a
/// pre-types file parses to, and `board.default_type` decides what `add`
/// writes (ADR 0022).
pub fn default_task_type() -> String {
    "task".to_string()
}

/// The Board's default Priority name when `board.default_priority` is absent.
/// Must stay a member of [`default_priorities`].
pub fn default_priority() -> String {
    "medium".to_string()
}

/// The default ordered Priority vocabulary: names from most to least
/// important. A Board replaces or renames it through `board.priorities`.
pub fn default_priorities() -> Vec<String> {
    ["highest", "high", "medium", "low"]
        .iter()
        .map(|name| name.to_string())
        .collect()
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

/// Provider-neutral link between a Task and a Remote Issue (ADR 0018). The
/// Remote Tracker names its adapter (`github` today), the Remote Project is
/// the tracker's container identity (`owner/repo` for GitHub), `id` is the
/// Remote Issue identity in the tracker's own terms, `url` is its canonical
/// link, and `baseline` is the opaque last-sync snapshot owned by the sync
/// engine. `active: false` keeps the identity as provenance without syncing.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RemoteLink {
    pub tracker: String,
    pub project: String,
    pub id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub url: Option<String>,
    #[serde(
        default = "default_remote_active",
        skip_serializing_if = "is_remote_active"
    )]
    pub active: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub baseline: Option<serde_yaml_ng::Value>,
}

fn default_remote_active() -> bool {
    true
}

fn is_remote_active(active: &bool) -> bool {
    *active
}

impl RemoteLink {
    pub fn validate(&self) -> std::result::Result<(), String> {
        for (name, value) in [
            ("tracker", self.tracker.as_str()),
            ("project", self.project.as_str()),
            ("id", self.id.as_str()),
        ] {
            if value.trim().is_empty() {
                return Err(format!("remote.{name} must not be empty"));
            }
        }
        Ok(())
    }
}

/// Largest serialized size of one owner's Property, in bytes.
pub const PROPERTY_MAX_BYTES: usize = 8192;
const PROPERTY_OWNER_MAX: usize = 40;

/// Checks one Property: a lowercase-slug owner and a JSON object of bounded size.
pub fn validate_property(owner: &str, value: &serde_json::Value) -> Result<()> {
    let invalid = |message: String| OrbitError::new(crate::error::code::INVALID_PROPERTY, message);
    let slug = !owner.is_empty()
        && owner.len() <= PROPERTY_OWNER_MAX
        && owner
            .bytes()
            .next()
            .is_some_and(|b| b.is_ascii_lowercase() || b.is_ascii_digit())
        && owner
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-');
    if !slug {
        return Err(invalid(format!(
            "property owner '{owner}' must be a lowercase slug of at most {PROPERTY_OWNER_MAX} characters"
        )));
    }
    if !value.is_object() {
        return Err(invalid(format!("property '{owner}' must be a JSON object")));
    }
    let size = serde_json::to_string(value)
        .map(|text| text.len())
        .unwrap_or(usize::MAX);
    if size > PROPERTY_MAX_BYTES {
        return Err(invalid(format!(
            "property '{owner}' is {size} bytes; the limit is {PROPERTY_MAX_BYTES}"
        )));
    }
    Ok(())
}

/// Longest Source Key, in characters.
pub const SOURCE_KEY_MAX: usize = 200;

/// Canonical link type: the owner Task is blocked by the link's target.
/// `blocks` links block readiness and take part in cycle checks unless
/// `blocking: false` overrides the default.
pub const LINK_TYPE_BLOCKS: &str = "blocks";
/// Symmetric association between two Tasks or a Task and an external reference.
pub const LINK_TYPE_RELATES_TO: &str = "relates-to";
/// The target duplicates the owner Task.
pub const LINK_TYPE_DUPLICATES: &str = "duplicates";
/// The target was discovered from the owner Task.
pub const LINK_TYPE_DISCOVERED_FROM: &str = "discovered-from";
/// Orbit's name for the `waiting_on` convenience: an external blocker recorded
/// through `add --waiting-on` / `update --waiting-on`.
pub const LINK_TYPE_WAITING_ON: &str = "waiting-on";

/// Longest link type name, in characters.
pub const LINK_TYPE_MAX: usize = 64;

/// The other end of a [`Link`]: an Orbit Task ID or an external reference.
/// Serializes untagged: a bare string is a Task ID, a map is external.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum LinkTarget {
    /// An Orbit Task ID.
    Task(String),
    /// A reference outside this Board (another tracker, a URL-backed issue, ...).
    External(ExternalRef),
}

/// An issue living outside the Board. `system` names the tracker or service
/// (`jira`, `github`, ...), `id` its identity there, `url` an optional
/// canonical link.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ExternalRef {
    pub system: String,
    pub id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub url: Option<String>,
}

impl ExternalRef {
    pub fn validate(&self) -> std::result::Result<(), String> {
        for (name, value) in [("system", self.system.as_str()), ("id", self.id.as_str())] {
            if value.trim().is_empty() {
                return Err(format!("link target external.{name} must not be empty"));
            }
        }
        if self.url.as_ref().is_some_and(|url| url.trim().is_empty()) {
            return Err("link target external.url must not be empty".to_string());
        }
        Ok(())
    }
}

impl LinkTarget {
    /// The Orbit Task ID when the target is internal.
    pub fn task_id(&self) -> Option<&str> {
        match self {
            Self::Task(id) => Some(id),
            Self::External(_) => None,
        }
    }

    /// Target equality for link operations: external references match on
    /// `system` + `id`, ignoring `url` (a URL is metadata, not identity).
    pub fn same_reference(&self, other: &Self) -> bool {
        match (self, other) {
            (Self::Task(a), Self::Task(b)) => a == b,
            (Self::External(a), Self::External(b)) => a.system == b.system && a.id == b.id,
            _ => false,
        }
    }

    /// Display label: the Task ID, or `system:id` for an external reference.
    pub fn label(&self) -> String {
        match self {
            Self::Task(id) => id.clone(),
            Self::External(reference) => format!("{}:{}", reference.system, reference.id),
        }
    }
}

/// A typed relation between this Task and another Task or an external
/// reference. The link is stored on the inward end: `type` names the
/// relation the *target* has toward the owner, so `blocks` on Task T
/// targeting D reads "D blocks T" — the same edge `depends_on` used to hold.
///
/// `blocking` overrides the type's default: `blocks` defaults to blocking,
/// every other type to non-blocking. Blocking links drive readiness and
/// cycle checks; non-blocking links never do. `attributes` carries arbitrary
/// Board- or tool-owned data.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Link {
    /// Link type: `blocks`, `relates-to`, `duplicates`, `discovered-from`,
    /// `waiting-on`, or a Board-defined custom slug. Serialized as `type`.
    #[serde(rename = "type")]
    pub kind: String,
    pub target: LinkTarget,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub blocking: Option<bool>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub attributes: BTreeMap<String, serde_yaml_ng::Value>,
}

impl Link {
    /// A blocking `blocks` link to an Orbit Task — what `depends_on` stored.
    pub fn dependency(target: &str) -> Self {
        Self {
            kind: LINK_TYPE_BLOCKS.to_string(),
            target: LinkTarget::Task(target.to_string()),
            blocking: None,
            attributes: BTreeMap::new(),
        }
    }

    /// A blocking link to an external reference — what `waiting_on` stored.
    /// `text` is the free-form blocker description.
    pub fn waiting_on(text: &str) -> Self {
        Self {
            kind: LINK_TYPE_WAITING_ON.to_string(),
            target: LinkTarget::External(ExternalRef {
                system: "external".to_string(),
                id: text.to_string(),
                url: None,
            }),
            blocking: Some(true),
            attributes: BTreeMap::new(),
        }
    }

    /// Whether the link participates in readiness and cycle checks.
    pub fn is_blocking(&self) -> bool {
        self.blocking
            .unwrap_or_else(|| self.kind == LINK_TYPE_BLOCKS)
    }

    /// The internal Task ID this link points at, when it has one.
    pub fn target_task(&self) -> Option<&str> {
        self.target.task_id()
    }

    /// Whether the target is an external reference.
    pub fn is_external(&self) -> bool {
        matches!(self.target, LinkTarget::External(_))
    }

    /// How the owner Task sees the link (the inward name of the type).
    pub fn inward_name(&self) -> String {
        match self.kind.as_str() {
            LINK_TYPE_BLOCKS => "is blocked by".to_string(),
            LINK_TYPE_RELATES_TO => "relates to".to_string(),
            LINK_TYPE_DUPLICATES => "is duplicated by".to_string(),
            LINK_TYPE_DISCOVERED_FROM => "is the origin of".to_string(),
            _ => self.kind.clone(),
        }
    }

    /// How the target sees the link (the outward name of the type).
    pub fn outward_name(&self) -> String {
        match self.kind.as_str() {
            LINK_TYPE_BLOCKS => "blocks".to_string(),
            LINK_TYPE_RELATES_TO => "relates to".to_string(),
            LINK_TYPE_DUPLICATES => "duplicates".to_string(),
            LINK_TYPE_DISCOVERED_FROM => "discovered from".to_string(),
            _ => self.kind.clone(),
        }
    }

    /// JSON shape returned by `show`/`list`: the stored fields plus the
    /// resolved blocking flag and both directional names.
    pub fn to_json(&self) -> serde_json::Value {
        serde_json::json!({
            "type": self.kind,
            "target": self.target,
            "blocking": self.is_blocking(),
            "inward": self.inward_name(),
            "outward": self.outward_name(),
            "attributes": self.attributes,
        })
    }
}

/// Normalizes a link type to its canonical lowercase kebab slug and checks
/// it is a valid type name. Accepts the spaced display forms ("relates to",
/// "discovered from") and maps them onto the canonical slugs.
pub fn normalize_link_kind(input: &str) -> Result<String> {
    let kind = input
        .trim()
        .to_lowercase()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join("-")
        .replace('_', "-");
    let valid = !kind.is_empty()
        && kind.len() <= LINK_TYPE_MAX
        && kind
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
        && kind
            .bytes()
            .next()
            .is_some_and(|b| b.is_ascii_lowercase() || b.is_ascii_digit());
    if !valid {
        return Err(OrbitError::new(
            crate::error::code::INVALID_ARGUMENT,
            format!(
                "invalid link type `{input}`; use a short lowercase slug such as `blocks` or `relates-to`"
            ),
        ));
    }
    Ok(kind)
}

/// Longest External ID, in characters.
pub const EXTERNAL_ID_MAX: usize = 200;

/// Checks an External ID: the Task's identity in an external system, unique
/// per Board. Same shape rules as a Source Key.
pub fn validate_external_id(id: &str) -> Result<()> {
    if id.is_empty() || id.len() > EXTERNAL_ID_MAX || !id.bytes().all(|b| b.is_ascii_graphic()) {
        return Err(OrbitError::new(
            crate::error::code::INVALID_EXTERNAL_ID,
            format!(
                "external id must be 1 to {EXTERNAL_ID_MAX} printable ASCII characters without spaces"
            ),
        ));
    }
    Ok(())
}

/// Checks a Source Key: 1 to 200 printable, non-whitespace ASCII characters.
pub fn validate_source_key(key: &str) -> Result<()> {
    if key.is_empty() || key.len() > SOURCE_KEY_MAX || !key.bytes().all(|b| b.is_ascii_graphic()) {
        return Err(OrbitError::new(
            crate::error::code::INVALID_SOURCE_KEY,
            format!(
                "source key must be 1 to {SOURCE_KEY_MAX} printable ASCII characters without spaces"
            ),
        ));
    }
    Ok(())
}

/// On-disk shape: the Claim is stored as three flat keys (see docs/SPEC.md).
#[derive(Serialize, Deserialize)]
struct RawFrontmatter {
    title: String,
    status: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    resolution: Option<String>,
    #[serde(default = "default_task_type", rename = "type")]
    task_type: String,
    #[serde(default = "default_priority")]
    priority: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    labels: Vec<String>,
    /// Legacy dependency list, read once into `links` and never written back.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    depends_on: Vec<String>,
    /// Legacy free-text external blocker, read once into `links`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    waiting_on: Option<String>,
    /// Typed links replacing `depends_on`/`waiting_on` (ADR 0022).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    links: Vec<Link>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    parent: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    checkpoint: Option<Checkpoint>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    component: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    verification: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    github: Option<GitHubLink>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    remote: Option<RemoteLink>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    repository: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    external_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    claimed_by: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    claimed_at: Option<DateTime<Utc>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    lease_until: Option<DateTime<Utc>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    source_key: Option<String>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    properties: BTreeMap<String, serde_json::Value>,
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
        // Legacy `depends_on`/`waiting_on` keys upgrade into `links` on read;
        // `links` itself is written back, the legacy keys never are.
        let mut links = raw.links;
        for dep in raw.depends_on {
            let link = Link::dependency(&dep);
            if !links.contains(&link) {
                links.push(link);
            }
        }
        if let Some(waiting) = raw.waiting_on
            && !waiting.trim().is_empty()
        {
            let link = Link::waiting_on(waiting.trim());
            if !links.contains(&link) {
                links.push(link);
            }
        }
        Self {
            title: raw.title,
            status: raw.status,
            resolution: raw.resolution,
            task_type: raw.task_type,
            priority: raw.priority,
            labels: raw.labels,
            links,
            parent: raw.parent,
            checkpoint: raw.checkpoint,
            component: raw.component,
            verification: raw.verification,
            github: raw.github,
            remote: raw.remote,
            repository: raw.repository,
            external_id: raw.external_id,
            claim,
            stray_claim_times,
            source_key: raw.source_key,
            properties: raw.properties,
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
            resolution: front.resolution,
            task_type: front.task_type,
            priority: front.priority,
            labels: front.labels,
            depends_on: Vec::new(),
            waiting_on: None,
            links: front.links,
            parent: front.parent,
            checkpoint: front.checkpoint,
            component: front.component,
            verification: front.verification.clone(),
            github: front.github,
            remote: front.remote,
            repository: front.repository,
            external_id: front.external_id,
            claimed_by,
            claimed_at,
            lease_until,
            source_key: front.source_key,
            properties: front.properties,
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
    /// The workflow status name; resolved against the Board's
    /// [`crate::workflow::Workflow`] for its category.
    pub status: String,
    /// The resolution recorded by the closing transition that set `status`;
    /// cleared when the Task reopens or transitions to an open status.
    pub resolution: Option<String>,
    /// The Board-configured Issue type name (`epic`, `story`, `task`, `bug`,
    /// `sub-task`, or a custom `board.types` entry). Stored as a string so the
    /// vocabulary is per-Board; membership is checked by `Graph::validate`.
    pub task_type: String,
    /// A Priority name from the Board's `board.priorities` list; Tasks sort
    /// by the name's ordinal in that list, not by the name itself.
    pub priority: String,
    pub labels: Vec<String>,
    /// Typed links to other Tasks or external references (ADR 0022).
    pub links: Vec<Link>,
    pub parent: Option<String>,
    pub checkpoint: Option<Checkpoint>,
    /// Optional work Component slug; each Board restricts it through
    /// `board.components`.
    pub component: Option<String>,
    pub verification: Option<String>,
    /// Legacy GitHub Issue link; `orbit migrate` rewrites it to `remote`.
    pub github: Option<GitHubLink>,
    /// Provider-neutral Remote Issue link (ADR 0018).
    pub remote: Option<RemoteLink>,
    /// The member Repository this Task belongs to; absent means Board-wide.
    pub repository: Option<String>,
    /// The Task's identity in an external system; unique per Board.
    pub external_id: Option<String>,
    pub claim: Option<Claim>,
    pub stray_claim_times: Option<StrayClaimTimes>,
    /// Caller-chosen identity that makes `add` retry-safe; unique per Board.
    pub source_key: Option<String>,
    /// Tool-owned data namespaced by owner slug; Orbit stores it but never
    /// interprets it.
    pub properties: BTreeMap<String, serde_json::Value>,
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

    /// The Task's configured Issue type name.
    pub fn task_type(&self) -> &str {
        &self.front.task_type
    }

    pub fn github_link(&self) -> Option<&GitHubLink> {
        self.front.github.as_ref().filter(|link| link.active)
    }

    /// The Task's active provider-neutral Remote Issue link (`remote:`
    /// frontmatter). Legacy `github:` links are read through
    /// [`crate::remote::link::active_link`] instead.
    pub fn remote_link(&self) -> Option<&RemoteLink> {
        self.front.remote.as_ref().filter(|link| link.active)
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

    /// Adds a typed link. Returns true when the link is new; an identical
    /// link (same type and target) leaves the Task untouched.
    pub fn add_link(&mut self, link: Link, now: DateTime<Utc>) -> bool {
        if self.front.links.iter().any(|existing| {
            existing.kind == link.kind && existing.target.same_reference(&link.target)
        }) {
            return false;
        }
        self.front.links.push(link);
        self.touch(now);
        true
    }

    /// Removes links matching `kind` (when given) and `target`. Returns true
    /// when at least one link was removed.
    pub fn remove_links(
        &mut self,
        kind: Option<&str>,
        target: &LinkTarget,
        now: DateTime<Utc>,
    ) -> bool {
        let before = self.front.links.len();
        self.front.links.retain(|link| {
            !link.target.same_reference(target)
                || (kind.is_some() && Some(link.kind.as_str()) != kind)
        });
        let changed = self.front.links.len() != before;
        if changed {
            self.touch(now);
        }
        changed
    }

    /// Removes every link pointing at `id`, whatever its type.
    pub fn remove_links_to(&mut self, id: &str, now: DateTime<Utc>) -> bool {
        let before = self.front.links.len();
        self.front
            .links
            .retain(|link| link.target_task() != Some(id));
        let changed = self.front.links.len() != before;
        if changed {
            self.touch(now);
        }
        changed
    }

    /// Links that take part in readiness and cycle checks.
    pub fn blocking_links(&self) -> impl Iterator<Item = &Link> {
        self.front.links.iter().filter(|link| link.is_blocking())
    }

    /// Blocking links whose target lives outside the Board.
    pub fn external_blockers(&self) -> impl Iterator<Item = &Link> {
        self.blocking_links().filter(|link| link.is_external())
    }

    /// Display labels for external blockers: the bare id for the `external`
    /// system `--waiting-on` writes, `system:id` for named systems.
    pub fn waiting_labels(&self) -> Vec<String> {
        self.external_blockers()
            .map(|link| match &link.target {
                LinkTarget::External(reference) if reference.system == "external" => {
                    reference.id.clone()
                }
                _ => link.target.label(),
            })
            .collect()
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
    if front.status.trim().is_empty() {
        return Err(OrbitError::corrupt_task(format!(
            "task {id} has an empty status"
        )));
    }
    front.priority = front.priority.trim().to_lowercase();
    if !is_valid_label(&front.priority) {
        return Err(OrbitError::corrupt_task(format!(
            "task {id} has invalid priority `{}`",
            front.priority
        )));
    }
    front.task_type = front.task_type.trim().to_lowercase();
    if !is_valid_label(&front.task_type) {
        return Err(OrbitError::corrupt_task(format!(
            "task {id} has invalid type `{}`",
            front.task_type
        )));
    }
    for link in &mut front.links {
        link.kind = normalize_link_kind(&link.kind).map_err(|error| {
            OrbitError::corrupt_task(format!("task {id} has an invalid link: {}", error.message))
        })?;
        if let Err(reason) = validate_link(link) {
            return Err(OrbitError::corrupt_task(format!("task {id}: {reason}")));
        }
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
    if let Some(link) = &front.remote
        && let Err(reason) = link.validate()
    {
        return Err(OrbitError::corrupt_task(format!(
            "task {id} has an invalid remote link: {reason}"
        )));
    }
    if let Some(repository) = &front.repository
        && !is_valid_repository(repository)
    {
        return Err(OrbitError::corrupt_task(format!(
            "task {id} has an invalid repository `{repository}`"
        )));
    }
    if let Some(component) = &front.component
        && !is_valid_component(component)
    {
        return Err(OrbitError::corrupt_task(format!(
            "task {id} has an invalid component `{component}`"
        )));
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

/// Checks one stored link: a usable type name, a non-empty target, and a
/// bounded attributes map.
fn validate_link(link: &Link) -> std::result::Result<(), String> {
    match &link.target {
        LinkTarget::Task(id) => {
            if id.trim().is_empty() {
                return Err("link target task id must not be empty".to_string());
            }
        }
        LinkTarget::External(reference) => reference.validate()?,
    }
    let attributes_size = serde_yaml_ng::to_string(&link.attributes)
        .map(|text| text.len())
        .unwrap_or(usize::MAX);
    if attributes_size > PROPERTY_MAX_BYTES {
        return Err(format!(
            "link attributes are {attributes_size} bytes; the limit is {PROPERTY_MAX_BYTES}"
        ));
    }
    Ok(())
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

    // Dotted IDs remain readable for Boards written before IDs decoupled
    // from `parent` (ADR 0022); `orbit slug migrate` rewrites them flat.
    rest.split('.').all(is_valid_rand)
}

fn is_valid_rand(value: &str) -> bool {
    (6..=8).contains(&value.len()) && value.bytes().all(|byte| ID_ALPHABET.contains(&byte))
}

/// A Task's Component: a short lowercase slug. Boards that configure
/// `board.components` restrict values to that list; others accept any valid
/// name.
pub fn is_valid_component(component: &str) -> bool {
    !component.is_empty()
        && component.len() <= MAX_LABEL_LENGTH
        && component
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
        && component
            .chars()
            .next()
            .is_some_and(|c| c.is_ascii_alphanumeric())
}

/// Normalizes a Component value to its lowercase slug and validates it against
/// the Board's `board.components` list. An empty `allowed` list means the
/// Board defines no Components, so any short lowercase slug is accepted.
pub fn normalize_component(input: &str, allowed: &[String]) -> Result<String> {
    let component = input.trim().to_ascii_lowercase();
    if !is_valid_component(&component) {
        return Err(OrbitError::new(
            crate::error::code::INVALID_COMPONENT,
            format!(
                "invalid component `{input}`; use a short lowercase slug of letters, digits, and `-`"
            ),
        ));
    }
    if !allowed.is_empty() && !allowed.iter().any(|candidate| candidate == &component) {
        return Err(OrbitError::new(
            crate::error::code::INVALID_COMPONENT,
            format!(
                "unknown component `{component}`; expected one of: {}",
                allowed.join(", ")
            ),
        ));
    }
    Ok(component)
}

/// A Task's member Repository: a short name or `owner/name` identity. Kept
/// permissive so local folder names and remote `owner/repo` forms both fit.
pub fn is_valid_repository(repository: &str) -> bool {
    !repository.is_empty()
        && repository.len() <= MAX_REPOSITORY_LENGTH
        && repository
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.' | '/'))
        && repository
            .chars()
            .next()
            .is_some_and(|c| c.is_ascii_alphanumeric())
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
                status: "todo".to_string(),
                resolution: None,
                task_type: "task".to_string(),
                priority: "high".to_string(),
                labels: Vec::new(),
                links: vec![Link::dependency("orb-f14c02")],
                parent: None,
                checkpoint: None,
                component: None,
                verification: None,
                github: None,
                remote: None,
                repository: None,
                external_id: None,
                claim: None,
                stray_claim_times: None,
                created_at: now(),
                updated_at: now(),
                source_key: None,
                properties: BTreeMap::new(),
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
        assert_eq!(parsed.front.links, task.front.links);
        assert_eq!(parsed.body, task.body);
    }

    #[test]
    fn preserves_unknown_frontmatter_fields() {
        let content = r#"---
title: Keep me
status: todo
priority: medium
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
priority: medium
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
        let content = "---\r\ntitle: Test\r\nstatus: todo\r\npriority: medium\r\ncreated_at: 2026-10-23T12:00:00Z\r\nupdated_at: 2026-10-23T12:00:00Z\r\n---\r\n\r\nBody.\r\n";
        let task = parse_task("orb-a7b2c9".to_string(), content).unwrap();
        assert_eq!(task.front.title, "Test");
        assert_eq!(task.body, "Body.\r\n");
    }

    #[test]
    fn rejects_invalid_priority() {
        let content = r#"---
title: Bad
status: todo
priority: "not a priority!"
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
        assert!(is_valid_id("orb-a7b2c9.d4b3df.e5f6a7", "orb-"));
        assert!(is_valid_id("orb-a7b2c9.d4b3df.e5f6a7.h8j9km", "orb-"));
        assert!(!is_valid_id("orb-a7b2c", "orb-"));
        assert!(!is_valid_id("orb-a7b2c9i", "orb-"));
        assert!(!is_valid_id("task-a7b2c9", "orb-"));
        assert!(!is_valid_id("orb-a7b2c9.", "orb-"));
        assert!(!is_valid_id("orb-.d4b3df", "orb-"));
        assert!(!is_valid_id("orb-a7b2c9..d4b3df", "orb-"));
        assert!(!is_valid_id("orb-a7b2c9.d4b3df.", "orb-"));
        assert!(!is_valid_id("orb-a7b2c9.d4b3df.e5f6a", "orb-"));
        assert!(!is_valid_id("orb-a7b2c9.d4b3df.e5f6a7i", "orb-"));
    }

    #[test]
    fn generated_ids_use_the_alphabet() {
        for _ in 0..100 {
            let id = generate_id("orb-", 6);
            assert!(is_valid_id(&id, "orb-"), "{id}");
            // IDs never encode the parent (ADR 0022): generated IDs are flat.
            assert!(!id.contains('.'));
        }
    }

    #[test]
    fn labels_are_normalized_sorted_and_deduped() {
        let content = r#"---
title: Labels
status: todo
priority: medium
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
        let content = "---\ntitle: t\nstatus: in_progress\ntype: task\npriority: medium\nclaimed_by: alice\nclaimed_at: 2026-10-23T12:00:00Z\nlease_until: 2026-10-23T12:30:00Z\ncreated_at: 2026-10-23T12:00:00Z\nupdated_at: 2026-10-23T12:00:00Z\nowner_note: keep me\n---\n\nBody.";
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
        let content = "---\ntitle: t\nstatus: todo\ntype: task\npriority: medium\nlease_until: 2026-10-23T12:30:00Z\ncreated_at: 2026-10-23T12:00:00Z\nupdated_at: 2026-10-23T12:00:00Z\n---\n\n";
        let task = parse_task("orb-a7b2c9".to_string(), content).unwrap();
        assert!(task.front.claim.is_none() && task.front.stray_claim_times.is_some());
        assert_eq!(render_task(&task).unwrap(), content);
    }

    #[test]
    fn checkpoint_round_trips_and_preserves_unknown_fields() {
        let content = "---\ntitle: t\nstatus: in_progress\ntype: task\npriority: medium\ncheckpoint:\n  by: alice\n  at: 2026-10-23T12:00:00Z\n  progress: Parser done\n  next: Wire the CLI\n  blocker: Waiting on review\nclaimed_by: alice\nclaimed_at: 2026-10-23T12:00:00Z\nlease_until: 2026-10-23T12:30:00Z\ncreated_at: 2026-10-23T12:00:00Z\nupdated_at: 2026-10-23T12:00:00Z\nowner_note: keep me\n---\n\nBody.";
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

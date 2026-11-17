//! `orbit board sync`: fetch the Board host, merge Orbit-aware, push.
//!
//! Merge rules (ADR 0019): Tasks and fields merge three-way; Comments union;
//! a same-field conflict stops until `--resolve FIELD=local|host`; the earlier
//! of two Claims wins and the loser is told on the Task; history is merged,
//! never rebased or force-pushed.

use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::board::{Board, Config};
use crate::board_host;
use crate::comment::{self, Comment};
use crate::error::{OrbitError, Result, code};
use crate::git::{self, CommitOutcome};
use crate::task::{self, Identity, Lease, Task, TaskFrontmatter};
use crate::workflow::Workflow;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MergeSide {
    Local,
    Host,
}

/// Parse `--resolve FIELD=local|host` values into a resolution map.
pub fn parse_resolutions(values: &[String]) -> Result<BTreeMap<String, MergeSide>> {
    let mut resolutions = BTreeMap::new();
    for value in values {
        let (field, choice) = value.split_once('=').ok_or_else(|| {
            OrbitError::invalid_argument(format!(
                "invalid conflict resolution `{value}`; expected FIELD=local or FIELD=host"
            ))
        })?;
        let side = match choice {
            "local" => MergeSide::Local,
            "host" => MergeSide::Host,
            _ => {
                return Err(OrbitError::invalid_argument(format!(
                    "invalid conflict resolution `{value}`; choose `local` or `host`"
                )));
            }
        };
        if field.trim().is_empty() {
            return Err(OrbitError::invalid_argument(format!(
                "invalid conflict resolution `{value}`; the field must not be empty"
            )));
        }
        if resolutions.insert(field.to_string(), side).is_some() {
            return Err(OrbitError::invalid_argument(format!(
                "field `{field}` has more than one resolution"
            )));
        }
    }
    Ok(resolutions)
}

/// A same-field divergence between the local Board and the host.
#[derive(Debug, Clone)]
pub struct FieldConflict {
    /// Resolution key: `<task-id>.<field>` for Task fields, `<task-id>` for a
    /// whole Task, `<task-id>.comments` for a comment sidecar, `config` for
    /// config.toml, or the repo-relative path for any other file.
    pub field: String,
    pub local: String,
    pub host: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum SyncState {
    UpToDate,
    Published,
    FastForwarded,
    Merged,
}

impl std::fmt::Display for SyncState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::UpToDate => "up_to_date",
            Self::Published => "published",
            Self::FastForwarded => "fast_forwarded",
            Self::Merged => "merged",
        })
    }
}

#[derive(Debug)]
pub struct SyncReport {
    pub state: SyncState,
    pub remote_tip: Option<String>,
    pub merged_files: usize,
    pub conflicts_resolved: Vec<String>,
    pub notices: Vec<String>,
    pub unused_resolutions: Vec<String>,
}

impl SyncReport {
    fn terminal(state: SyncState, remote_tip: Option<String>) -> Self {
        Self {
            state,
            remote_tip,
            merged_files: 0,
            conflicts_resolved: Vec::new(),
            notices: Vec::new(),
            unused_resolutions: Vec::new(),
        }
    }
}

#[derive(Clone)]
struct ClaimNotice {
    task: String,
    body: String,
}

struct MergeOutcome {
    files: BTreeMap<String, Option<String>>,
    conflicts: Vec<FieldConflict>,
    notices: Vec<ClaimNotice>,
    resolutions: BTreeMap<String, MergeSide>,
    used_resolutions: BTreeSet<String>,
}

/// Fetch, merge, and push the Board branch on its host.
///
/// The caller must not hold the Board lock: it is taken only around the
/// local commit, merge and reset, never across the network fetch or push, so
/// a slow host never blocks other agents on the Board.
pub fn sync(board: &Board, resolution_args: &[String]) -> Result<SyncReport> {
    match sync_once(board, resolution_args) {
        // Another process synced between our fetch and push (rejected push,
        // locked ref or config): one fresh attempt picks up its result.
        Err(error) if error.code == code::GIT_ERROR => sync_once(board, resolution_args),
        result => result,
    }
}

fn sync_once(board: &Board, resolution_args: &[String]) -> Result<SyncReport> {
    // Load fresh: `orbit board host` may have changed the config mid-command.
    let config = Config::load(&board.root)?;
    crate::modules::validate(&config)?;
    let spec = config.board.host.clone().ok_or_else(|| {
        OrbitError::new(
            code::BOARD_HOST_NOT_CONFIGURED,
            "the Board has no host; run `orbit board host <repo|url>` or `orbit init --host`",
        )
    })?;
    let url = board_host::resolve_host_url(board, &spec)?;
    let branch = config.board.branch.clone();
    let resolutions = parse_resolutions(resolution_args)?;

    let remote_tip = board_host::fetch_tip(&board.root, &url, &branch)?;
    // One Board per host: the host branch may have been replaced since
    // `orbit board host` checked it.
    if let Some(tip) = &remote_tip {
        let slug = crate::slug::slug_of_prefix(&config.id.prefix).to_string();
        board_host::check_host_slug(&board.root, tip, &slug)?;
    }

    let (report, push) = {
        let _lock = board.lock_exclusive()?;
        reconcile(board, remote_tip, &resolutions)?
    };
    if let Some(commit) = push {
        let refspec = format!("{commit}:refs/heads/{branch}");
        git::push(&board.root, board_host::HOST_REMOTE, &refspec)?;
    }
    Ok(report)
}

/// The locked part of `sync`: commit pending local changes, then publish,
/// fast-forward, or merge against `remote_tip`. Returns the report and the
/// commit to push, if any.
fn reconcile(
    board: &Board,
    remote_tip: Option<String>,
    resolutions: &BTreeMap<String, MergeSide>,
) -> Result<(SyncReport, Option<String>)> {
    // Persist pending local changes (heartbeats and friends) first.
    if let CommitOutcome::Failed(message) = board.commit("orbit: sync", true)? {
        return Err(OrbitError::git(message));
    }
    let local_head = git::rev_parse(&board.root, "HEAD")?
        .ok_or_else(|| OrbitError::git("the Board repository has no HEAD commit"))?;

    let Some(remote_tip) = remote_tip else {
        return Ok((
            SyncReport::terminal(SyncState::Published, None),
            Some(local_head),
        ));
    };
    if remote_tip == local_head {
        return Ok((
            SyncReport::terminal(SyncState::UpToDate, Some(remote_tip)),
            None,
        ));
    }

    let base = git::merge_base(&board.root, &local_head, &remote_tip)?;
    if base.as_deref() == Some(remote_tip.as_str()) {
        // The host is strictly behind: publish local history.
        return Ok((
            SyncReport::terminal(SyncState::Published, Some(remote_tip)),
            Some(local_head),
        ));
    }
    if base.as_deref() == Some(local_head.as_str()) {
        // The local Board is strictly behind: fast-forward, never rebase.
        git::reset_hard(&board.root, &remote_tip)?;
        return Ok((
            SyncReport::terminal(SyncState::FastForwarded, Some(remote_tip)),
            None,
        ));
    }

    let base_tree = match base.as_deref() {
        Some(base) => read_tree(&board.root, base)?,
        None => BTreeMap::new(),
    };
    let local_tree = read_tree(&board.root, &local_head)?;
    let remote_tree = read_tree(&board.root, &remote_tip)?;
    let outcome = merge_trees(
        &base_tree,
        &local_tree,
        &remote_tree,
        resolutions,
        Utc::now(),
        &board.workflow()?,
        &crate::key::load(board)?,
    )?;
    if !outcome.conflicts.is_empty() {
        return Err(OrbitError::new(
            code::BOARD_SYNC_CONFLICT,
            "board sync has unresolved field conflicts; resolve each with --resolve FIELD=local|host",
        )
        .with_details(serde_json::json!({
            "conflicts": outcome.conflicts.iter().map(|conflict| serde_json::json!({
                "field": conflict.field,
                "local": conflict.local,
                "host": conflict.host,
            })).collect::<Vec<_>>(),
        })));
    }

    let merged_files = outcome.files.len();
    let notices: Vec<String> = outcome.notices.iter().map(|n| n.body.clone()).collect();
    let commit = match write_merge(&board.root, &outcome.files, &local_head, &remote_tip) {
        Ok(commit) => commit,
        Err(error) => {
            rollback_merge(&board.root, &outcome.files, &local_head);
            return Err(error);
        }
    };
    git::reset_hard(&board.root, &commit)?;

    let conflicts_resolved: Vec<String> = resolutions
        .keys()
        .filter(|field| outcome.used_resolutions.contains(*field))
        .cloned()
        .collect();
    let unused_resolutions: Vec<String> = resolutions
        .keys()
        .filter(|field| !outcome.used_resolutions.contains(*field))
        .cloned()
        .collect();

    Ok((
        SyncReport {
            state: SyncState::Merged,
            remote_tip: Some(remote_tip),
            merged_files,
            conflicts_resolved,
            notices,
            unused_resolutions,
        },
        Some(commit),
    ))
}

/// Writes the merged files and records them as a merge commit of both tips.
fn write_merge(
    root: &Path,
    files: &BTreeMap<String, Option<String>>,
    local_head: &str,
    remote_tip: &str,
) -> Result<String> {
    apply_files(root, files)?;
    git::stage_all(root)?;
    let tree = git::write_tree(root)?;
    git::commit_tree_parents(
        root,
        &tree,
        &[local_head, remote_tip],
        "orbit: merge board host",
    )
}

/// Undoes a half-applied merge so the next commit cannot sweep it up: the
/// tree was clean at `local_head`, so reset to it and drop the files the
/// merge added. Best effort; the original error is what the caller reports.
fn rollback_merge(root: &Path, files: &BTreeMap<String, Option<String>>, local_head: &str) {
    let _ = git::reset_hard(root, local_head);
    for path in files.keys() {
        if matches!(git::show_file(root, local_head, path), Ok(None)) {
            let _ = std::fs::remove_file(root.join(path));
        }
    }
}

fn apply_files(root: &Path, files: &BTreeMap<String, Option<String>>) -> Result<()> {
    for (path, content) in files {
        let full = root.join(path);
        match content {
            Some(text) => {
                if let Some(parent) = full.parent() {
                    std::fs::create_dir_all(parent).map_err(|error| {
                        OrbitError::io(format!("create {}", parent.display()), error)
                    })?;
                }
                crate::atomic::write_atomic(&full, text.as_bytes(), true)?;
            }
            None => match std::fs::remove_file(&full) {
                Ok(()) => {}
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                Err(error) => {
                    return Err(OrbitError::io(format!("delete {}", full.display()), error));
                }
            },
        }
    }
    Ok(())
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum FileKind {
    Task,
    Comments,
    Config,
    /// `keys.toml`: the Issue-key allocator state; merges by maximum.
    Keys,
    Other,
}

fn classify(path: &str) -> (FileKind, String) {
    if path == "config.toml" {
        return (FileKind::Config, "config".to_string());
    }
    if path == crate::key::STATE_FILE {
        return (FileKind::Keys, "keys".to_string());
    }
    for (prefix, kind) in [
        ("tasks/", FileKind::Task),
        ("comments/", FileKind::Comments),
    ] {
        if let Some(rest) = path.strip_prefix(prefix)
            && let Some(stem) = rest.strip_suffix(".md")
            && !stem.contains('/')
            && !stem.starts_with('.')
        {
            let key = if kind == FileKind::Task {
                stem.to_string()
            } else {
                format!("{stem}.comments")
            };
            return (kind, key);
        }
    }
    (FileKind::Other, path.to_string())
}

fn describe_content(content: &Option<String>) -> String {
    match content {
        None => "(deleted)".to_string(),
        Some(text) => {
            let flat: String = text
                .lines()
                .next()
                .unwrap_or_default()
                .chars()
                .take(120)
                .collect();
            if flat.is_empty() {
                "(empty)".to_string()
            } else {
                flat
            }
        }
    }
}

fn yaml_desc<T: Serialize>(value: &T) -> String {
    let rendered = serde_yaml_ng::to_string(value).unwrap_or_default();
    let rendered = rendered.trim().trim_start_matches("---").trim();
    rendered.chars().take(200).collect()
}

/// Three-way field merge. `base` is the field's value in the merge base
/// (`None` when the Task itself is new on both sides).
#[allow(clippy::too_many_arguments)]
fn pick<T>(base: Option<T>, local: T, remote: T, field: &str, outcome: &mut MergeOutcome) -> T
where
    T: PartialEq + Clone + Serialize,
{
    if local == remote {
        return local;
    }
    if let Some(base) = &base {
        if *base == local {
            return remote;
        }
        if *base == remote {
            return local;
        }
    }
    match outcome.resolutions.get(field).copied() {
        Some(MergeSide::Local) => {
            outcome.used_resolutions.insert(field.to_string());
            local
        }
        Some(MergeSide::Host) => {
            outcome.used_resolutions.insert(field.to_string());
            remote
        }
        None => {
            outcome.conflicts.push(FieldConflict {
                field: field.to_string(),
                local: yaml_desc(&local),
                host: yaml_desc(&remote),
            });
            local
        }
    }
}

/// Identity of a link for merging: type plus target. Attributes and the
/// blocking override are the link's value, so editing them merges as a
/// three-way field change while two sides adding different links union.
fn link_key(link: &crate::task::Link) -> String {
    let target = serde_json::to_string(&link.target).unwrap_or_default();
    format!("{}|{target}", link.kind)
}

fn find_link(links: &[crate::task::Link], key: &str) -> Option<crate::task::Link> {
    links.iter().find(|link| link_key(link) == key).cloned()
}

/// Links merge per (type, target): each side's set of link identities is
/// three-way merged, so concurrent additions union and concurrent edits to
/// the same link conflict like any other field.
fn merge_links(
    base: Option<&[crate::task::Link]>,
    local: &[crate::task::Link],
    remote: &[crate::task::Link],
    field: &str,
    outcome: &mut MergeOutcome,
) -> Vec<crate::task::Link> {
    let mut keys: BTreeSet<String> = BTreeSet::new();
    for links in [base, Some(local), Some(remote)].into_iter().flatten() {
        keys.extend(links.iter().map(link_key));
    }
    let mut merged = Vec::new();
    for key in keys {
        if let Some(link) = pick(
            base.map(|links| find_link(links, &key)),
            find_link(local, &key),
            find_link(remote, &key),
            field,
            outcome,
        ) {
            merged.push(link);
        }
    }
    merged
}

#[derive(Clone)]
struct AssignmentSide {
    assignee: Option<Identity>,
    lease: Option<Lease>,
}

impl AssignmentSide {
    fn of(front: Option<&TaskFrontmatter>) -> Self {
        Self {
            assignee: front.and_then(|front| front.assignee.clone()),
            lease: front.and_then(|front| front.lease.clone()),
        }
    }

    fn eq(&self, other: &Self) -> bool {
        self.assignee == other.assignee && self.lease == other.lease
    }
}

fn later_time(a: Option<DateTime<Utc>>, b: Option<DateTime<Utc>>) -> Option<DateTime<Utc>> {
    match (a, b) {
        (Some(a), Some(b)) => Some(a.max(b)),
        (Some(a), None) => Some(a),
        (None, Some(b)) => Some(b),
        (None, None) => None,
    }
}

fn later_lease(a: &Option<Lease>, b: &Option<Lease>) -> Option<Lease> {
    match (a, b) {
        (Some(a), Some(b)) => {
            let key = |l: &Lease| (l.claimed_at, l.lease_until);
            Some(if key(a) >= key(b) {
                a.clone()
            } else {
                b.clone()
            })
        }
        (Some(a), None) => Some(a.clone()),
        (None, Some(b)) => Some(b.clone()),
        (None, None) => None,
    }
}

/// The earlier of two assignments wins; a missing lease start sorts last
/// and the assignee name breaks exact ties deterministically.
fn earlier_assignment<'a>(
    local: &'a AssignmentSide,
    remote: &'a AssignmentSide,
) -> (&'a AssignmentSide, &'a AssignmentSide) {
    let local_key = (
        local
            .lease
            .as_ref()
            .and_then(|lease| lease.claimed_at)
            .unwrap_or(DateTime::<Utc>::MAX_UTC),
        local
            .assignee
            .as_ref()
            .map(|assignee| assignee.name.as_str())
            .unwrap_or(""),
    );
    let remote_key = (
        remote
            .lease
            .as_ref()
            .and_then(|lease| lease.claimed_at)
            .unwrap_or(DateTime::<Utc>::MAX_UTC),
        remote
            .assignee
            .as_ref()
            .map(|assignee| assignee.name.as_str())
            .unwrap_or(""),
    );
    if local_key <= remote_key {
        (local, remote)
    } else {
        (remote, local)
    }
}

fn assignment_notice(
    task: &str,
    winner: &AssignmentSide,
    loser: Option<&AssignmentSide>,
) -> ClaimNotice {
    let name = winner
        .assignee
        .as_ref()
        .map(|assignee| assignee.name.as_str())
        .unwrap_or("unknown");
    let claimed = winner
        .lease
        .as_ref()
        .and_then(|lease| lease.claimed_at)
        .map(|at| at.to_rfc3339())
        .unwrap_or_else(|| "unknown".into());
    let body = match loser {
        Some(loser) => {
            let loser_name = loser
                .assignee
                .as_ref()
                .map(|assignee| assignee.name.as_str())
                .unwrap_or("unknown");
            let loser_claimed = loser
                .lease
                .as_ref()
                .and_then(|lease| lease.claimed_at)
                .map(|at| at.to_rfc3339())
                .unwrap_or_else(|| "unknown".into());
            format!(
                "claim conflict on {task}: the earlier assignment to {name} (claimed {claimed}) won over {loser_name} (claimed {loser_claimed})"
            )
        }
        None => format!(
            "claim conflict on {task}: kept the assignment to {name} (claimed {claimed}); the other side had released it"
        ),
    };
    ClaimNotice {
        task: task.to_string(),
        body,
    }
}

/// Assignment merge: the earlier assignment wins and the loser is told; an
/// assignment always beats a release when both sides changed it differently.
/// A Task whose merged status is terminal keeps no assignee or lease.
fn merge_assignment(
    id: &str,
    status: &str,
    workflow: &Workflow,
    base: &AssignmentSide,
    local: &AssignmentSide,
    remote: &AssignmentSide,
    outcome: &mut MergeOutcome,
) -> AssignmentSide {
    if workflow.is_terminal(status) {
        // Stray lease times (a lease with no assignee on either side)
        // survive; a real claim does not.
        let lease = if local.assignee.is_none() && remote.assignee.is_none() {
            later_lease(&local.lease, &remote.lease)
        } else {
            None
        };
        return AssignmentSide {
            assignee: None,
            lease,
        };
    }
    // The same assignee on both sides keeps the freshest lease.
    if let (Some(local_assignee), Some(remote_assignee)) = (&local.assignee, &remote.assignee)
        && local_assignee == remote_assignee
    {
        let lease = match (&local.lease, &remote.lease) {
            (Some(local_lease), Some(remote_lease)) => Some(Lease {
                claimed_at: local_lease.claimed_at.or(remote_lease.claimed_at),
                lease_until: later_time(local_lease.lease_until, remote_lease.lease_until),
            }),
            (Some(local_lease), None) => Some(local_lease.clone()),
            (None, Some(remote_lease)) => Some(remote_lease.clone()),
            (None, None) => None,
        };
        return AssignmentSide {
            assignee: local.assignee.clone(),
            lease,
        };
    }
    if local.assignee.is_none() && remote.assignee.is_none() {
        return AssignmentSide {
            assignee: None,
            lease: later_lease(&local.lease, &remote.lease),
        };
    }
    // Ordinary three-way: only one side touched the assignment.
    if local.eq(base) {
        return remote.clone();
    }
    if remote.eq(base) {
        return local.clone();
    }
    // Both sides changed the assignment differently.
    match (&local.assignee, &remote.assignee) {
        (Some(_), Some(_)) => {
            let (winner, loser) = earlier_assignment(local, remote);
            outcome
                .notices
                .push(assignment_notice(id, winner, Some(loser)));
            AssignmentSide {
                assignee: winner.assignee.clone(),
                lease: winner.lease.clone(),
            }
        }
        (Some(_), None) => {
            outcome.notices.push(assignment_notice(id, local, None));
            local.clone()
        }
        (None, Some(_)) => {
            outcome.notices.push(assignment_notice(id, remote, None));
            remote.clone()
        }
        (None, None) => AssignmentSide {
            assignee: None,
            lease: later_lease(&local.lease, &remote.lease),
        },
    }
}

/// Merge one Task file three-way. Returns the rendered file plus any
/// unresolved field conflicts and claim notices.
fn merge_task(
    id: &str,
    base: Option<Task>,
    local: Task,
    remote: Task,
    workflow: &Workflow,
    outcome: &mut MergeOutcome,
) -> Result<String> {
    let base_front = base.as_ref().map(|task| &task.front);
    let key = |field: &str| format!("{id}.{field}");

    let title = pick(
        base_front.map(|f| f.title.clone()),
        local.front.title.clone(),
        remote.front.title.clone(),
        &key("title"),
        outcome,
    );
    // The project key merges like any field; a same-key collision across
    // *different* Tasks is resolved after the merge by re-keying.
    let issue_key = pick(
        base_front.map(|f| f.key.clone()),
        local.front.key.clone(),
        remote.front.key.clone(),
        &key("key"),
        outcome,
    );
    // Key aliases only ever accumulate (a re-key appends, never removes),
    // so the merge is a sorted union of both sides.
    let mut key_aliases: Vec<String> = local
        .front
        .key_aliases
        .iter()
        .chain(remote.front.key_aliases.iter())
        .cloned()
        .collect();
    key_aliases.sort();
    key_aliases.dedup();
    let status = pick(
        base_front.map(|f| f.status.clone()),
        local.front.status.clone(),
        remote.front.status.clone(),
        &key("status"),
        outcome,
    );
    let task_type = pick(
        base_front.map(|f| f.task_type.clone()),
        local.front.task_type.clone(),
        remote.front.task_type.clone(),
        &key("type"),
        outcome,
    );
    let priority = pick(
        base_front.map(|f| f.priority.clone()),
        local.front.priority.clone(),
        remote.front.priority.clone(),
        &key("priority"),
        outcome,
    );
    let labels = pick(
        base_front.map(|f| f.labels.clone()),
        local.front.labels.clone(),
        remote.front.labels.clone(),
        &key("labels"),
        outcome,
    );
    let links = merge_links(
        base_front.map(|f| f.links.as_slice()),
        &local.front.links,
        &remote.front.links,
        &key("links"),
        outcome,
    );
    let parent = pick(
        base_front.map(|f| f.parent.clone()),
        local.front.parent.clone(),
        remote.front.parent.clone(),
        &key("parent"),
        outcome,
    );
    let checkpoint = pick(
        base_front.map(|f| f.checkpoint.clone()),
        local.front.checkpoint.clone(),
        remote.front.checkpoint.clone(),
        &key("checkpoint"),
        outcome,
    );
    let component = pick(
        base_front.map(|f| f.component.clone()),
        local.front.component.clone(),
        remote.front.component.clone(),
        &key("component"),
        outcome,
    );
    let verification = pick(
        base_front.map(|f| f.verification.clone()),
        local.front.verification.clone(),
        remote.front.verification.clone(),
        &key("verification"),
        outcome,
    );
    let github = pick(
        base_front.map(|f| f.github.clone()),
        local.front.github.clone(),
        remote.front.github.clone(),
        &key("github"),
        outcome,
    );
    let remote_link = pick(
        base_front.map(|f| f.remote.clone()),
        local.front.remote.clone(),
        remote.front.remote.clone(),
        &key("remote"),
        outcome,
    );
    let repository = pick(
        base_front.map(|f| f.repository.clone()),
        local.front.repository.clone(),
        remote.front.repository.clone(),
        &key("repository"),
        outcome,
    );
    let external_id = pick(
        base_front.map(|f| f.external_id.clone()),
        local.front.external_id.clone(),
        remote.front.external_id.clone(),
        &key("external_id"),
        outcome,
    );
    let created_at = pick(
        base_front.map(|f| f.created_at),
        local.front.created_at,
        remote.front.created_at,
        &key("created_at"),
        outcome,
    );
    let updated_at = local.front.updated_at.max(remote.front.updated_at);
    let body = pick(
        base.as_ref().map(|task| task.body.clone()),
        local.body.clone(),
        remote.body.clone(),
        &key("body"),
        outcome,
    );

    let resolution = pick(
        base_front.map(|f| f.resolution.clone()),
        local.front.resolution.clone(),
        remote.front.resolution.clone(),
        &key("resolution"),
        outcome,
    );
    let assignment = merge_assignment(
        id,
        &status,
        workflow,
        &AssignmentSide::of(base_front),
        &AssignmentSide::of(Some(&local.front)),
        &AssignmentSide::of(Some(&remote.front)),
        outcome,
    );
    let reporter = pick(
        base_front.map(|f| f.reporter.clone()),
        local.front.reporter.clone(),
        remote.front.reporter.clone(),
        &key("reporter"),
        outcome,
    );

    // Unknown frontmatter keys merge per key.
    let mut extra_keys: BTreeSet<String> = BTreeSet::new();
    for source in [
        base_front.map(|f| f.extra.keys()),
        Some(local.front.extra.keys()),
        Some(remote.front.extra.keys()),
    ]
    .into_iter()
    .flatten()
    {
        extra_keys.extend(source.cloned());
    }
    let mut extra = BTreeMap::new();
    for key_name in extra_keys {
        // `Some(None)` = the base Task exists but lacks the key.
        let base_value = base_front.map(|f| f.extra.get(&key_name).cloned());
        let local_value = local.front.extra.get(&key_name).cloned();
        let remote_value = remote.front.extra.get(&key_name).cloned();
        if let Some(value) = pick(
            base_value,
            local_value,
            remote_value,
            &key(&key_name),
            outcome,
        ) {
            extra.insert(key_name, value);
        }
    }

    let source_key = pick(
        base_front.map(|f| f.source_key.clone()),
        local.front.source_key.clone(),
        remote.front.source_key.clone(),
        &key("source_key"),
        outcome,
    );
    // Properties merge per owner, like unknown keys.
    let mut owners: BTreeSet<String> = BTreeSet::new();
    for source in [
        base_front.map(|f| f.properties.keys()),
        Some(local.front.properties.keys()),
        Some(remote.front.properties.keys()),
    ]
    .into_iter()
    .flatten()
    {
        owners.extend(source.cloned());
    }
    let mut properties = BTreeMap::new();
    for owner in owners {
        let base_value = base_front.map(|f| f.properties.get(&owner).cloned());
        let local_value = local.front.properties.get(&owner).cloned();
        let remote_value = remote.front.properties.get(&owner).cloned();
        if let Some(value) = pick(
            base_value,
            local_value,
            remote_value,
            &key(&format!("properties.{owner}")),
            outcome,
        ) {
            properties.insert(owner, value);
        }
    }

    let merged = Task {
        id: id.to_string(),
        front: task::TaskFrontmatter {
            title,
            status,
            key: issue_key,
            key_aliases,
            resolution,
            task_type,
            priority,
            labels,
            links,
            parent,
            checkpoint,
            component,
            verification,
            github,
            remote: remote_link,
            repository,
            external_id,
            assignee: assignment.assignee,
            lease: assignment.lease,
            reporter,
            source_key,
            properties,
            created_at,
            updated_at,
            extra,
        },
        body,
    };
    task::render_task(&merged)
}

/// Git adapter: materialize a revision before entering the merge engine.
fn read_tree(root: &Path, revision: &str) -> Result<BTreeMap<String, String>> {
    let mut tree = BTreeMap::new();
    for path in git::tree_files(root, revision)? {
        if let Some(content) = git::show_file(root, revision, &path)? {
            tree.insert(path, content);
        }
    }
    Ok(tree)
}

/// Lenient `keys.toml` shape for merging: either field may be absent.
#[derive(Debug, Deserialize)]
struct KeyStatePartial {
    prefix: Option<String>,
    next_key: Option<u64>,
}

fn parse_key_state_partial(content: &str) -> Option<KeyStatePartial> {
    toml::from_str(content).ok()
}

fn render_key_state(prefix: &str, next_key: u64) -> Result<String> {
    toml::to_string_pretty(&crate::key::KeyState {
        prefix: prefix.to_string(),
        next_key,
    })
    .map_err(|error| OrbitError::config(format!("serialize keys.toml: {error}")))
}

/// Merge two diverged `keys.toml` contents: the higher `next_key` wins so no
/// handed-out key is reused, and the prefix must agree — a prefix split is
/// a real conflict.
fn merge_keys_file(local: Option<&str>, remote: Option<&str>) -> std::result::Result<String, ()> {
    let parse = |content: Option<&str>| content.and_then(parse_key_state_partial);
    let (Some(local), Some(remote)) = (parse(local), parse(remote)) else {
        return Err(());
    };
    if let (Some(local_prefix), Some(remote_prefix)) = (&local.prefix, &remote.prefix)
        && local_prefix != remote_prefix
    {
        return Err(());
    }
    let prefix = local
        .prefix
        .or(remote.prefix)
        .unwrap_or_else(|| crate::key::DEFAULT_KEY_PREFIX.to_string());
    let next_key = local
        .next_key
        .into_iter()
        .chain(remote.next_key)
        .max()
        .unwrap_or(1);
    render_key_state(&prefix, next_key).map_err(|_| ())
}

/// The allocator state the merged tree implies: the merged `keys.toml` when
/// present and parseable, else the local Board's loaded state.
fn merged_key_state(outcome: &MergeOutcome, fallback: &crate::key::KeyState) -> (String, u64) {
    if let Some(content) = outcome
        .files
        .get(crate::key::STATE_FILE)
        .and_then(|content| content.as_deref())
        && let Some(partial) = parse_key_state_partial(content)
    {
        let prefix = partial
            .prefix
            .filter(|prefix| !prefix.is_empty())
            .unwrap_or_else(|| fallback.prefix.clone());
        let next_key = partial.next_key.unwrap_or(fallback.next_key);
        return (prefix, next_key);
    }
    (fallback.prefix.clone(), fallback.next_key)
}

/// Re-key Tasks that ended the merge sharing one project key. The Task with
/// the lowest internal ID keeps the key — a deterministic rule both clones
/// compute identically — and every other Task takes the next free number
/// with the contested key recorded as an alias, so the old key keeps
/// resolving (ADR 0022). `keys.toml` is raised to cover the re-keyed
/// numbers.
fn resolve_key_collisions(
    outcome: &mut MergeOutcome,
    key_state: &crate::key::KeyState,
) -> Result<()> {
    // Collect merged Task files carrying a live key.
    let mut by_key: BTreeMap<String, Vec<(String, Task)>> = BTreeMap::new();
    let mut max_number = 0u64;
    for (path, content) in &outcome.files {
        let Some(content) = content else { continue };
        let Some(stem) = path
            .strip_prefix("tasks/")
            .and_then(|rest| rest.strip_suffix(".md"))
        else {
            continue;
        };
        if stem.contains('/') || stem.starts_with('.') {
            continue;
        }
        let Ok(task) = task::parse_task(stem.to_string(), content) else {
            continue;
        };
        for key_str in crate::key::task_keys(&task.front) {
            if let Some(number) = crate::key::key_number(key_str) {
                max_number = max_number.max(number);
            }
        }
        if let Some(key) = &task.front.key {
            by_key
                .entry(crate::key::normalize(key))
                .or_default()
                .push((path.clone(), task));
        }
    }
    if by_key.values().all(|entries| entries.len() < 2) {
        return Ok(());
    }

    let (prefix, mut next) = merged_key_state(outcome, key_state);
    next = next.max(max_number + 1);
    let start_next = next;

    for entries in by_key.into_values() {
        if entries.len() < 2 {
            continue;
        }
        let mut entries = entries;
        entries.sort_by(|left, right| left.1.id.cmp(&right.1.id));
        for (path, mut task) in entries.into_iter().skip(1) {
            let old = task.front.key.clone().unwrap_or_else(|| task.id.clone());
            let new_key = crate::key::rekey(&mut task, &old, next, &prefix);
            next += 1;
            let rendered = task::render_task(&task)?;
            outcome.files.insert(path, Some(rendered));
            outcome.notices.push(ClaimNotice {
                task: task.id.clone(),
                body: format!(
                    "key conflict resolved on board sync: {old} is held by another Issue; this Issue was re-keyed to {new_key} ({old} stays recorded as an alias)"
                ),
            });
        }
    }

    if next > start_next {
        outcome.files.insert(
            crate::key::STATE_FILE.to_string(),
            Some(render_key_state(&prefix, next)?),
        );
    }
    Ok(())
}

/// Merge the trees of `local` and `remote` three-way against `base`
/// (empty for unrelated histories). Every union path resolves to merged
/// content (`Some`) or deletion (`None`); unmerged divergences land in
/// `conflicts` unless a `--resolve` covers them.
fn merge_trees(
    base: &BTreeMap<String, String>,
    local: &BTreeMap<String, String>,
    remote: &BTreeMap<String, String>,
    resolutions: &BTreeMap<String, MergeSide>,
    notice_time: DateTime<Utc>,
    workflow: &Workflow,
    key_state: &crate::key::KeyState,
) -> Result<MergeOutcome> {
    let mut outcome = MergeOutcome {
        files: BTreeMap::new(),
        conflicts: Vec::new(),
        notices: Vec::new(),
        resolutions: resolutions.clone(),
        used_resolutions: BTreeSet::new(),
    };

    let paths: BTreeSet<&String> = local.keys().chain(remote.keys()).collect();
    for path in paths {
        resolve_path(
            path,
            base.get(path).cloned(),
            local.get(path).cloned(),
            remote.get(path).cloned(),
            workflow,
            &mut outcome,
        )?;
    }

    // Diverged clones can hand the same project key to different Issues;
    // re-key all but one (deterministically) and record the old key as an
    // alias so it keeps resolving (ADR 0022).
    resolve_key_collisions(&mut outcome, key_state)?;

    // Tell every claim loser on their Task's comment sidecar. The notices
    // stay on the outcome so the report can surface them too.
    for notice in outcome.notices.clone() {
        let path = format!("comments/{}.md", notice.task);
        if outcome.files.get(&path).is_some_and(Option::is_none) {
            continue; // sidecar deleted on both sides stays deleted
        }
        let existing = outcome
            .files
            .get(&path)
            .and_then(|content| content.clone())
            .unwrap_or_default();
        let comment = Comment {
            author: "orbit".to_string(),
            created_at: notice_time,
            body: notice.body,
        };
        let merged = comment::append_comment(&existing, &notice.task, &comment);
        outcome.files.insert(path, Some(merged));
    }
    Ok(outcome)
}

/// Resolve one path across the three trees. `base`/`local`/`remote` are the
/// file contents (`None` = absent) at that path.
fn resolve_path(
    path: &str,
    base: Option<String>,
    local: Option<String>,
    remote: Option<String>,
    workflow: &Workflow,
    outcome: &mut MergeOutcome,
) -> Result<()> {
    if local == remote {
        outcome.files.insert(path.to_string(), local);
        return Ok(());
    }
    if local == base {
        outcome.files.insert(path.to_string(), remote);
        return Ok(());
    }
    if remote == base {
        outcome.files.insert(path.to_string(), local);
        return Ok(());
    }

    // Both sides changed the path differently. A create/delete mismatch is a
    // file-level conflict regardless of kind.
    if local.is_none() != remote.is_none() {
        let (_, field) = classify(path);
        let chosen = file_conflict(outcome, &field, local, remote);
        outcome.files.insert(path.to_string(), chosen);
        return Ok(());
    }

    let (kind, field) = classify(path);
    match kind {
        FileKind::Task => {
            let id = field.clone();
            let local_task = local
                .as_deref()
                .and_then(|c| task::parse_task(id.clone(), c).ok());
            let remote_task = remote
                .as_deref()
                .and_then(|c| task::parse_task(id.clone(), c).ok());
            let base_task = base
                .as_deref()
                .and_then(|c| task::parse_task(id.clone(), c).ok());
            match (local_task, remote_task) {
                (Some(local_task), Some(remote_task)) => {
                    let content =
                        merge_task(&id, base_task, local_task, remote_task, workflow, outcome)?;
                    outcome.files.insert(path.to_string(), Some(content));
                }
                _ => {
                    // Unparseable on one side: whole-Task conflict.
                    let chosen = file_conflict(outcome, &id, local, remote);
                    outcome.files.insert(path.to_string(), chosen);
                }
            }
        }
        FileKind::Comments => {
            let id = field
                .strip_suffix(".comments")
                .unwrap_or(&field)
                .to_string();
            match merge_comments(&id, local.as_deref(), remote.as_deref()) {
                Ok(content) => {
                    outcome.files.insert(path.to_string(), Some(content));
                }
                Err(()) => {
                    let chosen = file_conflict(outcome, &field, local, remote);
                    outcome.files.insert(path.to_string(), chosen);
                }
            }
        }
        FileKind::Keys => {
            // The allocator high-water mark merges by maximum so neither
            // side's handed-out keys are reused; a prefix divergence is a
            // real conflict.
            match merge_keys_file(local.as_deref(), remote.as_deref()) {
                Ok(content) => {
                    outcome.files.insert(path.to_string(), Some(content));
                }
                Err(()) => {
                    let (_, field) = classify(path);
                    let chosen = file_conflict(outcome, &field, local, remote);
                    outcome.files.insert(path.to_string(), chosen);
                }
            }
        }
        FileKind::Config | FileKind::Other => {
            let chosen = file_conflict(outcome, &field, local, remote);
            outcome.files.insert(path.to_string(), chosen);
        }
    }
    Ok(())
}

/// Record a file-level conflict and return the resolved content (`None` when
/// unresolved or when the winning side deleted the file).
fn file_conflict(
    outcome: &mut MergeOutcome,
    field: &str,
    local: Option<String>,
    remote: Option<String>,
) -> Option<String> {
    let local_desc = describe_content(&local);
    let remote_desc = describe_content(&remote);
    match outcome.resolutions.get(field).copied() {
        Some(MergeSide::Local) => {
            outcome.used_resolutions.insert(field.to_string());
            local
        }
        Some(MergeSide::Host) => {
            outcome.used_resolutions.insert(field.to_string());
            remote
        }
        None => {
            outcome.conflicts.push(FieldConflict {
                field: field.to_string(),
                local: local_desc,
                host: remote_desc,
            });
            local
        }
    }
}

/// Union two comment sidecars: identical comments (same author, timestamp,
/// and body) collapse, the rest sort by `(created_at, author, body)`.
fn merge_comments(
    task_id: &str,
    local: Option<&str>,
    remote: Option<&str>,
) -> std::result::Result<String, ()> {
    let mut comments: Vec<Comment> = Vec::new();
    for content in [local, remote].into_iter().flatten() {
        let parsed = comment::parse_comments(content).map_err(|_| ())?;
        for comment in parsed {
            if !comments.iter().any(|existing| {
                existing.author == comment.author
                    && existing.created_at == comment.created_at
                    && existing.body == comment.body
            }) {
                comments.push(comment);
            }
        }
    }
    comments.sort_by(|a, b| {
        (a.created_at, &a.author, &a.body).cmp(&(b.created_at, &b.author, &b.body))
    });
    Ok(comment::render_comments_file(task_id, &comments))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tree(entries: &[(&str, &str)]) -> BTreeMap<String, String> {
        entries
            .iter()
            .map(|(path, content)| (path.to_string(), content.to_string()))
            .collect()
    }

    fn now() -> DateTime<Utc> {
        "2026-01-01T00:00:00Z".parse().unwrap()
    }

    fn key_state() -> crate::key::KeyState {
        crate::key::KeyState {
            prefix: "ORB".to_string(),
            next_key: 1,
        }
    }

    const TASK_PATH: &str = "tasks/orbit-test.md";

    fn task_content(title: &str, priority: &str, status_and_claim: &str) -> String {
        format!(
            "---\ntitle: {title}\npriority: {priority}\n{status_and_claim}\ncreated_at: 2025-01-01T00:00:00Z\nupdated_at: 2025-01-01T00:00:00Z\n---\n\nTask body\n"
        )
    }

    fn merge_tasks(base: &str, local: &str, host: &str, resolutions: &[String]) -> MergeOutcome {
        merge_trees(
            &tree(&[(TASK_PATH, base)]),
            &tree(&[(TASK_PATH, local)]),
            &tree(&[(TASK_PATH, host)]),
            &parse_resolutions(resolutions).unwrap(),
            now(),
            &Workflow::standard(),
            &key_state(),
        )
        .unwrap()
    }

    fn merged_task(outcome: &MergeOutcome) -> Task {
        task::parse_task(
            "orbit-test".to_string(),
            outcome.files[TASK_PATH].as_deref().unwrap(),
        )
        .unwrap()
    }

    #[test]
    fn different_fields_merge() {
        let base = task_content("Original", "medium", "status: todo");
        let local = task_content("Local title", "medium", "status: todo");
        let host = task_content("Original", "low", "status: todo");
        let merged = merge_tasks(&base, &local, &host, &[]);
        assert!(merged.conflicts.is_empty());
        let task = merged_task(&merged);
        assert_eq!(task.front.title, "Local title");
        assert_eq!(task.front.priority, "low");
    }

    #[test]
    fn same_field_conflicts_until_resolved_to_either_side() {
        let base = task_content("Original", "medium", "status: todo");
        let local = task_content("Local title", "medium", "status: todo");
        let host = task_content("Host title", "medium", "status: todo");
        let unresolved = merge_tasks(&base, &local, &host, &[]);
        assert_eq!(unresolved.conflicts.len(), 1);
        assert_eq!(unresolved.conflicts[0].field, "orbit-test.title");
        for (side, title) in [("local", "Local title"), ("host", "Host title")] {
            let merged = merge_tasks(&base, &local, &host, &[format!("orbit-test.title={side}")]);
            assert!(merged.conflicts.is_empty());
            assert_eq!(merged_task(&merged).front.title, title);
            assert!(merged.used_resolutions.contains("orbit-test.title"));
        }
    }

    fn task_with(properties: &str) -> Task {
        let content = format!(
            "---\ntitle: T\nstatus: todo\n{properties}created_at: 2026-01-01T00:00:00Z\nupdated_at: 2026-01-01T00:00:00Z\n---\nbody\n"
        );
        task::parse_task("orb-aaaaaa".to_string(), &content).unwrap()
    }

    fn outcome() -> MergeOutcome {
        MergeOutcome {
            files: BTreeMap::new(),
            conflicts: Vec::new(),
            notices: Vec::new(),
            resolutions: BTreeMap::new(),
            used_resolutions: BTreeSet::new(),
        }
    }

    #[test]
    fn comments_union_deduplicates_and_orders_entries() {
        let entry = |author: &str, day: &str, body: &str| Comment {
            author: author.to_string(),
            created_at: day.parse().unwrap(),
            body: body.to_string(),
        };
        let shared = entry("shared", "2025-01-01T00:00:00Z", "shared note");
        let early = entry("host", "2025-01-02T00:00:00Z", "host note");
        let late = entry("local", "2025-01-03T00:00:00Z", "local note");
        let render = |comments: &[Comment]| comment::render_comments_file("orbit-test", comments);
        let base = render(std::slice::from_ref(&shared));
        let local = render(&[late, shared.clone()]);
        let host = render(&[early, shared]);
        let path = "comments/orbit-test.md";
        let merged = merge_trees(
            &tree(&[(path, &base)]),
            &tree(&[(path, &local)]),
            &tree(&[(path, &host)]),
            &BTreeMap::new(),
            now(),
            &Workflow::standard(),
            &key_state(),
        )
        .unwrap();
        assert!(merged.conflicts.is_empty());
        let comments = comment::parse_comments(merged.files[path].as_deref().unwrap()).unwrap();
        assert_eq!(
            comments.iter().map(|c| c.body.as_str()).collect::<Vec<_>>(),
            ["shared note", "host note", "local note"]
        );
    }

    #[test]
    fn earlier_claim_wins_and_loser_receives_a_comment() {
        let base = task_content("Claimed", "medium", "status: todo");
        let local = task_content(
            "Claimed",
            "medium",
            "status: in_progress\nclaimed_by: local\nclaimed_at: 2025-03-01T00:00:00Z\nlease_until: 2030-01-01T00:00:00Z",
        );
        let host = task_content(
            "Claimed",
            "medium",
            "status: in_progress\nclaimed_by: host\nclaimed_at: 2025-02-01T00:00:00Z\nlease_until: 2030-01-01T00:00:00Z",
        );
        for (local, host) in [(&local, &host), (&host, &local)] {
            let merged = merge_tasks(&base, local, host, &[]);
            assert!(merged.conflicts.is_empty());
            assert_eq!(merged_task(&merged).front.assignee.unwrap().name, "host");
            assert_eq!(merged.notices.len(), 1);
            let comments =
                comment::parse_comments(merged.files["comments/orbit-test.md"].as_deref().unwrap())
                    .unwrap();
            assert_eq!(comments.len(), 1);
            assert_eq!(comments[0].created_at, now());
            assert!(comments[0].body.contains("won over local"));
        }
    }

    #[test]
    fn finished_task_drops_host_claim() {
        let base = task_content(
            "Finished",
            "medium",
            "status: in_progress\nclaimed_by: local\nclaimed_at: 2025-02-01T00:00:00Z\nlease_until: 2030-01-01T00:00:00Z",
        );
        let local = task_content("Finished", "medium", "status: done");
        let host = base
            .replace("claimed_by: local", "claimed_by: host")
            .replace("2025-02-01", "2025-03-01");
        let merged = merge_tasks(&base, &local, &host, &[]);
        assert!(merged.conflicts.is_empty());
        let task = merged_task(&merged);
        assert_eq!(task.front.status, "done");
        assert!(task.front.assignee.is_none());
    }

    #[test]
    fn edit_delete_conflicts_and_can_choose_deletion() {
        let base = tree(&[(TASK_PATH, "original")]);
        let local = tree(&[(TASK_PATH, "edited")]);
        let host = BTreeMap::new();
        let merged = merge_trees(
            &base,
            &local,
            &host,
            &BTreeMap::new(),
            now(),
            &Workflow::standard(),
            &key_state(),
        )
        .unwrap();
        assert_eq!(merged.conflicts.len(), 1);
        assert_eq!(merged.conflicts[0].field, "orbit-test");
        assert_eq!(merged.conflicts[0].host, "(deleted)");
        let resolutions = parse_resolutions(&["orbit-test=host".to_string()]).unwrap();
        let merged = merge_trees(
            &base,
            &local,
            &host,
            &resolutions,
            now(),
            &Workflow::standard(),
            &key_state(),
        )
        .unwrap();
        assert!(merged.conflicts.is_empty());
        assert_eq!(merged.files[TASK_PATH], None);
        let both_deleted = merge_trees(
            &base,
            &host,
            &host,
            &BTreeMap::new(),
            now(),
            &Workflow::standard(),
            &key_state(),
        )
        .unwrap();
        assert!(!both_deleted.files.contains_key(TASK_PATH));
    }

    #[test]
    fn unparseable_task_conflicts_as_a_whole_task() {
        let base = task_content("Original", "medium", "status: todo");
        let host = task_content("Host", "medium", "status: todo");
        let merged = merge_tasks(&base, "not a task", &host, &[]);
        assert_eq!(merged.conflicts.len(), 1);
        assert_eq!(merged.conflicts[0].field, "orbit-test");
        let merged = merge_tasks(&base, "not a task", &host, &["orbit-test=host".to_string()]);
        assert!(merged.conflicts.is_empty());
        assert_eq!(merged.files[TASK_PATH].as_deref(), Some(host.as_str()));
    }

    #[test]
    fn unrelated_trees_merge_different_tasks() {
        let local = tree(&[("tasks/orbit-local.md", "local task")]);
        let remote = tree(&[("tasks/orbit-host.md", "host task")]);
        let merged = merge_trees(
            &BTreeMap::new(),
            &local,
            &remote,
            &BTreeMap::new(),
            now(),
            &Workflow::standard(),
            &key_state(),
        )
        .unwrap();
        assert!(merged.conflicts.is_empty());
        assert_eq!(
            merged.files["tasks/orbit-local.md"].as_deref(),
            Some("local task")
        );
        assert_eq!(
            merged.files["tasks/orbit-host.md"].as_deref(),
            Some("host task")
        );
    }

    fn keyed_task_content(key: &str) -> String {
        format!(
            "---\ntitle: T\nstatus: todo\nkey: {key}\ncreated_at: 2025-01-01T00:00:00Z\nupdated_at: 2025-01-01T00:00:00Z\n---\n\nbody\n"
        )
    }

    #[test]
    fn diverged_key_assignments_are_rekeyed_with_aliases() {
        // Unrelated histories: each clone handed ORB-1 to a different Task.
        let local = tree(&[("tasks/orb-aaaaaa.md", keyed_task_content("ORB-1").as_str())]);
        let remote = tree(&[("tasks/orb-bbbbbb.md", keyed_task_content("ORB-1").as_str())]);
        let merged = merge_trees(
            &BTreeMap::new(),
            &local,
            &remote,
            &BTreeMap::new(),
            now(),
            &Workflow::standard(),
            &key_state(),
        )
        .unwrap();
        assert!(merged.conflicts.is_empty(), "{:?}", merged.conflicts);

        // `orb-aaaaaa` sorts before `orb-bbbbbb`, so it keeps ORB-1.
        let keeper = task::parse_task(
            "orb-aaaaaa".to_string(),
            merged.files["tasks/orb-aaaaaa.md"].as_deref().unwrap(),
        )
        .unwrap();
        assert_eq!(keeper.front.key.as_deref(), Some("ORB-1"));
        assert!(keeper.front.key_aliases.is_empty());
        let loser = task::parse_task(
            "orb-bbbbbb".to_string(),
            merged.files["tasks/orb-bbbbbb.md"].as_deref().unwrap(),
        )
        .unwrap();
        // The loser takes ORB-2, records ORB-1 as an alias, and keeps its
        // immutable Task ID.
        assert_eq!(loser.id, "orb-bbbbbb");
        assert_eq!(loser.front.key.as_deref(), Some("ORB-2"));
        assert_eq!(loser.front.key_aliases, vec!["ORB-1".to_string()]);
        assert!(merged.files.contains_key(crate::key::STATE_FILE));
        assert!(
            merged
                .notices
                .iter()
                .any(|notice| notice.body.contains("re-keyed"))
        );
    }

    #[test]
    fn merge_keeps_properties_from_both_sides() {
        let base = task_with("");
        let local = task_with("properties:\n  ci:\n    passed: true\n");
        let remote = task_with("properties:\n  planner:\n    order: 1\n");
        let mut outcome = outcome();
        let merged = merge_task(
            "orb-aaaaaa",
            Some(base),
            local,
            remote,
            &Workflow::standard(),
            &mut outcome,
        )
        .unwrap();
        let parsed = task::parse_task("orb-aaaaaa".to_string(), &merged).unwrap();
        assert_eq!(
            serde_json::to_value(&parsed.front.properties).unwrap(),
            serde_json::json!({"ci": {"passed": true}, "planner": {"order": 1}})
        );
        assert!(outcome.conflicts.is_empty());
    }

    #[test]
    fn merge_keeps_a_source_key_set_on_one_side() {
        let base = task_with("");
        let local = task_with("source_key: planner:plan-7:step-1\n");
        let remote = task_with("");
        let mut outcome = outcome();
        let merged = merge_task(
            "orb-aaaaaa",
            Some(base),
            local,
            remote,
            &Workflow::standard(),
            &mut outcome,
        )
        .unwrap();
        let parsed = task::parse_task("orb-aaaaaa".to_string(), &merged).unwrap();
        assert_eq!(
            parsed.front.source_key.as_deref(),
            Some("planner:plan-7:step-1")
        );
    }
}

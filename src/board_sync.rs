//! `orbit board sync`: fetch the Board host, merge Orbit-aware, push.
//!
//! Merge rules (ADR 0019): Tasks and fields merge three-way; Comments union;
//! a same-field conflict stops until `--resolve FIELD=local|host`; the earlier
//! of two Claims wins and the loser is told on the Task; history is merged,
//! never rebased or force-pushed.

use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};

use chrono::{DateTime, Utc};
use serde::Serialize;

use crate::board::{Board, Config, HostSyncMode};
use crate::board_host;
use crate::comment::{self, Comment};
use crate::error::{OrbitError, Result, code};
use crate::git::{self, CommitOutcome};
use crate::task::{self, Claim, Status, StrayClaimTimes, Task, TaskFrontmatter};

static SUPPRESSED: AtomicBool = AtomicBool::new(false);

/// Suppress host auto-sync for the rest of this process (`orbit board sync`
/// performs the sync itself; `--no-sync` sets this too).
pub fn suppress() {
    SUPPRESSED.store(true, Ordering::Relaxed);
}

pub fn suppressed() -> bool {
    if SUPPRESSED.load(Ordering::Relaxed) {
        return true;
    }
    ["ORBIT_NO_AUTO_SYNC", "ORBIT_NO_HOST_SYNC"]
        .iter()
        .filter_map(|key| std::env::var(key).ok())
        .any(|value| {
            matches!(
                value.trim().to_ascii_lowercase().as_str(),
                "1" | "true" | "yes" | "on"
            )
        })
}

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
    let spec = config.board.host.clone().ok_or_else(|| {
        OrbitError::new(
            code::BOARD_HOST_NOT_CONFIGURED,
            "the Board has no host; run `orbit board host <repo|url>` or `orbit init --host`",
        )
    })?;
    let url = board_host::resolve_host_url(board, &spec)?;
    let branch = config.board.branch.clone();
    let resolutions = parse_resolutions(resolution_args)?;

    board_host::ensure_remote(&board.root, &url)?;
    git::fetch(&board.root, board_host::HOST_REMOTE)?;
    let remote_ref = format!("refs/remotes/{}/{}", board_host::HOST_REMOTE, branch);
    let remote_tip = git::rev_parse(&board.root, &remote_ref)?;
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

    let outcome = merge_trees(
        &board.root,
        base.as_deref(),
        &local_head,
        &remote_tip,
        resolutions,
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

/// The `host_sync = auto` hook: sync after a committed mutation or at prime.
/// Failures and contested claims surface as warnings, never errors.
pub fn auto_after(board: &Board) -> Vec<String> {
    if suppressed() {
        return Vec::new();
    }
    let Ok(config) = Config::load(&board.root) else {
        return Vec::new();
    };
    if config.board.host_sync != HostSyncMode::Auto || config.board.host.is_none() {
        return Vec::new();
    }
    match sync(board, &[]) {
        Ok(report) => {
            let mut warnings = report.notices;
            if report.state == SyncState::Merged {
                warnings.push(format!(
                    "board host: merged divergent history and pushed ({} files)",
                    report.merged_files
                ));
            }
            warnings
        }
        Err(error) => vec![format!("BOARD_HOST_SYNC_FAILED: {}", error.message)],
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
    Other,
}

fn classify(path: &str) -> (FileKind, String) {
    if path == "config.toml" {
        return (FileKind::Config, "config".to_string());
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

/// Set-aware variant of `pick` for `depends_on`: order and duplicates do not
/// make two equal dependency lists conflict.
fn pick_depends_on(
    base: Option<Vec<String>>,
    local: Vec<String>,
    remote: Vec<String>,
    field: &str,
    outcome: &mut MergeOutcome,
) -> Vec<String> {
    let canonical = |mut list: Vec<String>| {
        list.sort();
        list.dedup();
        list
    };
    pick(
        base.map(canonical),
        canonical(local),
        canonical(remote),
        field,
        outcome,
    )
}

#[derive(Clone)]
struct ClaimSide {
    claim: Option<Claim>,
    stray: Option<StrayClaimTimes>,
}

impl ClaimSide {
    fn of(front: Option<&TaskFrontmatter>) -> Self {
        Self {
            claim: front.and_then(|front| front.claim.clone()),
            stray: front.and_then(|front| front.stray_claim_times.clone()),
        }
    }

    fn eq(&self, other: &Self) -> bool {
        self.claim == other.claim && self.stray == other.stray
    }
}

fn claim_identity(claim: &Claim) -> (Option<DateTime<Utc>>, &str) {
    (claim.claimed_at, claim.holder.as_str())
}

fn later_time(a: Option<DateTime<Utc>>, b: Option<DateTime<Utc>>) -> Option<DateTime<Utc>> {
    match (a, b) {
        (Some(a), Some(b)) => Some(a.max(b)),
        (Some(a), None) => Some(a),
        (None, Some(b)) => Some(b),
        (None, None) => None,
    }
}

fn later_stray(
    a: &Option<StrayClaimTimes>,
    b: &Option<StrayClaimTimes>,
) -> Option<StrayClaimTimes> {
    match (a, b) {
        (Some(a), Some(b)) => {
            let key = |s: &StrayClaimTimes| (s.claimed_at, s.lease_until);
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

/// The earlier of two Claims wins; a missing timestamp sorts last and the
/// holder name breaks exact ties deterministically.
fn earlier_claim<'a>(local: &'a Claim, remote: &'a Claim) -> (&'a Claim, &'a Claim) {
    let local_key = (
        local.claimed_at.unwrap_or(DateTime::<Utc>::MAX_UTC),
        local.holder.as_str(),
    );
    let remote_key = (
        remote.claimed_at.unwrap_or(DateTime::<Utc>::MAX_UTC),
        remote.holder.as_str(),
    );
    if local_key <= remote_key {
        (local, remote)
    } else {
        (remote, local)
    }
}

fn claim_notice(task: &str, winner: &Claim, loser: Option<&Claim>) -> ClaimNotice {
    let body = match loser {
        Some(loser) => format!(
            "claim conflict on {task}: the earlier claim by {} (claimed {}) won over {} (claimed {})",
            winner.holder,
            winner
                .claimed_at
                .map(|at| at.to_rfc3339())
                .unwrap_or_else(|| "unknown".into()),
            loser.holder,
            loser
                .claimed_at
                .map(|at| at.to_rfc3339())
                .unwrap_or_else(|| "unknown".into()),
        ),
        None => format!(
            "claim conflict on {task}: kept the claim by {} (claimed {}); the other side had released it",
            winner.holder,
            winner
                .claimed_at
                .map(|at| at.to_rfc3339())
                .unwrap_or_else(|| "unknown".into()),
        ),
    };
    ClaimNotice {
        task: task.to_string(),
        body,
    }
}

/// Claim merge: the earlier Claim wins and the loser is told; a Claim always
/// beats a release when both sides changed it differently. A Task whose
/// merged status is terminal keeps no Claim.
fn merge_claim(
    id: &str,
    status: Status,
    base: &ClaimSide,
    local: &ClaimSide,
    remote: &ClaimSide,
    outcome: &mut MergeOutcome,
) -> ClaimSide {
    if status.is_terminal() {
        return ClaimSide {
            claim: None,
            stray: later_stray(&local.stray, &remote.stray),
        };
    }
    // The same claim on both sides keeps the freshest lease.
    if let (Some(local_claim), Some(remote_claim)) = (&local.claim, &remote.claim)
        && claim_identity(local_claim) == claim_identity(remote_claim)
    {
        let mut claim = local_claim.clone();
        claim.lease_until = later_time(local_claim.lease_until, remote_claim.lease_until);
        return ClaimSide {
            claim: Some(claim),
            stray: None,
        };
    }
    if local.claim.is_none() && remote.claim.is_none() {
        return ClaimSide {
            claim: None,
            stray: later_stray(&local.stray, &remote.stray),
        };
    }
    // Ordinary three-way: only one side touched the claim.
    if local.eq(base) {
        return remote.clone();
    }
    if remote.eq(base) {
        return local.clone();
    }
    // Both sides changed the claim differently.
    match (&local.claim, &remote.claim) {
        (Some(local_claim), Some(remote_claim)) => {
            let (winner, loser) = earlier_claim(local_claim, remote_claim);
            outcome.notices.push(claim_notice(id, winner, Some(loser)));
            ClaimSide {
                claim: Some(winner.clone()),
                stray: None,
            }
        }
        (Some(local_claim), None) => {
            outcome.notices.push(claim_notice(id, local_claim, None));
            ClaimSide {
                claim: Some(local_claim.clone()),
                stray: None,
            }
        }
        (None, Some(remote_claim)) => {
            outcome.notices.push(claim_notice(id, remote_claim, None));
            ClaimSide {
                claim: Some(remote_claim.clone()),
                stray: None,
            }
        }
        (None, None) => ClaimSide {
            claim: None,
            stray: later_stray(&local.stray, &remote.stray),
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
    let status = pick(
        base_front.map(|f| f.status),
        local.front.status,
        remote.front.status,
        &key("status"),
        outcome,
    );
    let task_type = pick(
        base_front.map(|f| f.task_type),
        local.front.task_type,
        remote.front.task_type,
        &key("type"),
        outcome,
    );
    let priority = pick(
        base_front.map(|f| f.priority),
        local.front.priority,
        remote.front.priority,
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
    let depends_on = pick_depends_on(
        base_front.map(|f| f.depends_on.clone()),
        local.front.depends_on.clone(),
        remote.front.depends_on.clone(),
        &key("depends_on"),
        outcome,
    );
    let parent = pick(
        base_front.map(|f| f.parent.clone()),
        local.front.parent.clone(),
        remote.front.parent.clone(),
        &key("parent"),
        outcome,
    );
    let waiting_on = pick(
        base_front.map(|f| f.waiting_on.clone()),
        local.front.waiting_on.clone(),
        remote.front.waiting_on.clone(),
        &key("waiting_on"),
        outcome,
    );
    let checkpoint = pick(
        base_front.map(|f| f.checkpoint.clone()),
        local.front.checkpoint.clone(),
        remote.front.checkpoint.clone(),
        &key("checkpoint"),
        outcome,
    );
    let area = pick(
        base_front.map(|f| f.area.clone()),
        local.front.area.clone(),
        remote.front.area.clone(),
        &key("area"),
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
    let repo = pick(
        base_front.map(|f| f.repo.clone()),
        local.front.repo.clone(),
        remote.front.repo.clone(),
        &key("repo"),
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

    let claim = merge_claim(
        id,
        status,
        &ClaimSide::of(base_front),
        &ClaimSide::of(Some(&local.front)),
        &ClaimSide::of(Some(&remote.front)),
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
    // Annotations merge per owner, like unknown keys.
    let mut owners: BTreeSet<String> = BTreeSet::new();
    for source in [
        base_front.map(|f| f.annotations.keys()),
        Some(local.front.annotations.keys()),
        Some(remote.front.annotations.keys()),
    ]
    .into_iter()
    .flatten()
    {
        owners.extend(source.cloned());
    }
    let mut annotations = BTreeMap::new();
    for owner in owners {
        let base_value = base_front.map(|f| f.annotations.get(&owner).cloned());
        let local_value = local.front.annotations.get(&owner).cloned();
        let remote_value = remote.front.annotations.get(&owner).cloned();
        if let Some(value) = pick(
            base_value,
            local_value,
            remote_value,
            &key(&format!("annotations.{owner}")),
            outcome,
        ) {
            annotations.insert(owner, value);
        }
    }

    let merged = Task {
        id: id.to_string(),
        front: task::TaskFrontmatter {
            title,
            status,
            task_type,
            priority,
            labels,
            depends_on,
            parent,
            waiting_on,
            checkpoint,
            area,
            verification,
            github,
            remote: remote_link,
            repo,
            claim: claim.claim,
            stray_claim_times: claim.stray,
            source_key,
            annotations,
            created_at,
            updated_at,
            extra,
        },
        body,
    };
    task::render_task(&merged)
}

/// Merge the trees of `local` and `remote` three-way against `base`
/// (`None` for unrelated histories). Every union path resolves to merged
/// content (`Some`) or deletion (`None`); unmerged divergences land in
/// `conflicts` unless a `--resolve` covers them.
fn merge_trees(
    root: &Path,
    base: Option<&str>,
    local: &str,
    remote: &str,
    resolutions: &BTreeMap<String, MergeSide>,
) -> Result<MergeOutcome> {
    let mut outcome = MergeOutcome {
        files: BTreeMap::new(),
        conflicts: Vec::new(),
        notices: Vec::new(),
        resolutions: resolutions.clone(),
        used_resolutions: BTreeSet::new(),
    };

    let mut paths: BTreeSet<String> = git::tree_files(root, local)?.into_iter().collect();
    paths.extend(git::tree_files(root, remote)?);

    for path in paths {
        let local_content = git::show_file(root, local, &path)?;
        let remote_content = git::show_file(root, remote, &path)?;
        let base_content = match base {
            Some(base) => git::show_file(root, base, &path)?,
            None => None,
        };
        resolve_path(
            &path,
            base_content,
            local_content,
            remote_content,
            &mut outcome,
        )?;
    }

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
            created_at: Utc::now(),
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
                    let content = merge_task(&id, base_task, local_task, remote_task, outcome)?;
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

    fn task_with(annotations: &str) -> Task {
        let content = format!(
            "---\ntitle: T\nstatus: todo\n{annotations}created_at: 2026-01-01T00:00:00Z\nupdated_at: 2026-01-01T00:00:00Z\n---\nbody\n"
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
    fn merge_keeps_annotations_from_both_sides() {
        let base = task_with("");
        let local = task_with("annotations:\n  ci:\n    passed: true\n");
        let remote = task_with("annotations:\n  planner:\n    order: 1\n");
        let mut outcome = outcome();
        let merged = merge_task("orb-aaaaaa", Some(base), local, remote, &mut outcome).unwrap();
        let parsed = task::parse_task("orb-aaaaaa".to_string(), &merged).unwrap();
        assert_eq!(
            serde_json::to_value(&parsed.front.annotations).unwrap(),
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
        let merged = merge_task("orb-aaaaaa", Some(base), local, remote, &mut outcome).unwrap();
        let parsed = task::parse_task("orb-aaaaaa".to_string(), &merged).unwrap();
        assert_eq!(
            parsed.front.source_key.as_deref(),
            Some("planner:plan-7:step-1")
        );
    }
}

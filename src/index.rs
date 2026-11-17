//! The opt-in derived read index (ADR 0022).
//!
//! `[index] enabled = true` in `config.toml` turns on a rebuildable cache at
//! `<orbit>/cache/index.json` — outside the tracked Board files — holding
//! each Task's parsed frontmatter and body stamped with the file's length
//! and modification time. Indexed loads ([`crate::graph::Graph::load_indexed`],
//! used by `orbit list`, `ready`, `search`, `show`, `tree`, `prime`,
//! `digest`, and `export`) serve entries whose fingerprint still matches and
//! re-parse the rest, so the Board pays a full scan only for changed files.
//!
//! Task files remain the only source of truth. A missing, corrupt, or stale
//! index degrades to the plain scan and is rebuilt from the files; `orbit
//! index rebuild` forces a full rebuild. The Board head commit recorded in
//! the file is informational for `orbit index status` — freshness comes from
//! per-file fingerprints, so the index never lags a file edit, whether the
//! edit came from Orbit or not.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::time::UNIX_EPOCH;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::board::{Board, board_cache_dir};
use crate::error::Result;
use crate::task::{self, Task, TaskFrontmatter};

const INDEX_FILE: &str = "index.json";
const INDEX_VERSION: u32 = 1;

// The `[index]` config schema lives in `crate::board` (kernel-owned
// config.toml schema); this module interprets it.
pub use crate::board::IndexConfig;

/// File metadata of `tasks/<id>.md` when the entry was indexed: byte length
/// plus modification time, re-stated on every indexed load. A mismatch marks
/// the entry stale and the file is re-parsed authoritatively.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Fingerprint {
    pub len: u64,
    pub mtime_secs: i64,
    pub mtime_nanos: u32,
}

fn fingerprint(path: &Path) -> Option<Fingerprint> {
    let metadata = std::fs::metadata(path).ok()?;
    let (mtime_secs, mtime_nanos) = metadata
        .modified()
        .ok()
        .and_then(|time| time.duration_since(UNIX_EPOCH).ok())
        .map(|elapsed| (elapsed.as_secs() as i64, elapsed.subsec_nanos()))
        .unwrap_or((0, 0));
    Some(Fingerprint {
        len: metadata.len(),
        mtime_secs,
        mtime_nanos,
    })
}

#[derive(Serialize, Deserialize)]
struct IndexEntry {
    #[serde(flatten)]
    print: Fingerprint,
    front: TaskFrontmatter,
    body: String,
}

#[derive(Serialize, Deserialize)]
struct IndexFile {
    version: u32,
    /// The Board head commit when the index was written; informational for
    /// `orbit index status`, never consulted for freshness.
    head: Option<String>,
    built_at: DateTime<Utc>,
    entries: BTreeMap<String, IndexEntry>,
}

/// The plan [`plan`] computes for one indexed load: cached parses, the ids
/// that need an authoritative re-parse, and the pre-read fingerprints used
/// to stamp [`store`] conservatively. It travels inside
/// [`crate::graph::IndexPlan::state`] so the kernel never sees it.
struct Plan {
    /// Tasks served from still-fresh index entries.
    tasks: HashMap<String, Task>,
    /// Task ids whose entry was absent, stale, or unreadable — the caller
    /// re-reads and re-parses them exactly as [`crate::graph::Graph::load`].
    rescan: Vec<String>,
    /// The fingerprint of every Task file captured before any read, so the
    /// stored index never claims a file fresher than the bytes parsed.
    prints: HashMap<String, Option<Fingerprint>>,
}

/// The `ReadIndex` implementation the application layer hands to
/// [`crate::graph::Graph::load_indexed`]. [`load`] is the entry point: it
/// checks `[index] enabled` and falls back to the plain scan.
pub struct DerivedIndex;

impl crate::graph::ReadIndex for DerivedIndex {
    fn plan(&self, board: &Board, ids: &[String]) -> crate::graph::IndexPlan {
        let mut plan = plan(board, ids);
        crate::graph::IndexPlan {
            tasks: std::mem::take(&mut plan.tasks),
            rescan: std::mem::take(&mut plan.rescan),
            state: Some(Box::new(plan)),
        }
    }

    fn store(&self, board: &Board, tasks: &HashMap<String, Task>, plan: &crate::graph::IndexPlan) {
        let Some(plan) = plan
            .state
            .as_ref()
            .and_then(|state| state.downcast_ref::<Plan>())
        else {
            return;
        };
        store(board, tasks, plan);
    }
}

/// `Graph::load` through the derived index when `[index] enabled = true`,
/// the plain scan otherwise. The kernel decides nothing about the module;
/// the application layer calls this where it used `load_indexed`.
pub fn load(
    board: &Board,
    mode: crate::graph::LoadMode,
) -> Result<(crate::graph::Graph, Vec<crate::graph::LoadIssue>)> {
    let index = board.config.index.enabled.then_some(DerivedIndex);
    crate::graph::Graph::load_indexed(
        board,
        mode,
        index
            .as_ref()
            .map(|index| index as &dyn crate::graph::ReadIndex),
    )
}

/// `cache/index.json` beside the Board folder — untracked, like the other
/// derived caches (Remote catalog, idempotency records).
pub fn index_path(board: &Board) -> PathBuf {
    board_cache_dir(board).join(INDEX_FILE)
}

fn read_index(board: &Board) -> Option<IndexFile> {
    let content = std::fs::read_to_string(index_path(board)).ok()?;
    let file: IndexFile = serde_json::from_str(&content).ok()?;
    (file.version == INDEX_VERSION).then_some(file)
}

/// Plans an indexed load over `ids` (the current `Board::task_ids`): Tasks
/// whose index entry still matches their file's fingerprint are served from
/// the cache; new, changed, and vanished-stat files land in `rescan`.
fn plan(board: &Board, ids: &[String]) -> Plan {
    let mut entries = read_index(board)
        .map(|file| file.entries)
        .unwrap_or_default();
    let mut tasks = HashMap::new();
    let mut rescan = Vec::new();
    let mut prints = HashMap::with_capacity(ids.len());
    for id in ids {
        let print = fingerprint(&board.task_path(id));
        match (print, entries.remove(id)) {
            (Some(print), Some(entry)) if entry.print == print => {
                tasks.insert(
                    id.clone(),
                    Task {
                        id: id.clone(),
                        front: entry.front,
                        body: entry.body,
                    },
                );
            }
            _ => rescan.push(id.clone()),
        }
        prints.insert(id.clone(), print);
    }
    Plan {
        tasks,
        rescan,
        prints,
    }
}

/// Persists the index after an indexed load: one entry per loaded Task,
/// stamped with the fingerprint captured *before* its file was read so a
/// file that changed mid-load rescans next time rather than reading stale
/// forever. Best-effort — a store failure costs a rescan, never data.
fn store(board: &Board, tasks: &HashMap<String, Task>, plan: &Plan) {
    let _ = store_inner(board, tasks, plan);
}

fn store_inner(board: &Board, tasks: &HashMap<String, Task>, plan: &Plan) -> Result<()> {
    let mut entries = BTreeMap::new();
    for (id, task) in tasks {
        let Some(Some(print)) = plan.prints.get(id) else {
            continue;
        };
        entries.insert(
            id.clone(),
            IndexEntry {
                print: *print,
                front: task.front.clone(),
                body: task.body.clone(),
            },
        );
    }
    let file = IndexFile {
        version: INDEX_VERSION,
        head: crate::git::rev_parse(&board.root, "HEAD")?,
        built_at: Utc::now(),
        entries,
    };
    let content = serde_json::to_vec(&file).map_err(|error| {
        crate::error::OrbitError::config(format!("serialize Board index: {error}"))
    })?;
    let path = index_path(board);
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).map_err(|error| {
            crate::error::OrbitError::io(format!("create {}", dir.display()), error)
        })?;
    }
    crate::atomic::write_atomic(&path, &content, true)
}

/// What [`rebuild`] produced.
pub struct Rebuilt {
    /// Task files parsed and cached.
    pub entries: usize,
    /// Files skipped because they did not read or parse.
    pub skipped: usize,
    /// The Board head recorded in the index.
    pub head: Option<String>,
}

/// Full rebuild: every Task file is read and parsed authoritatively, then
/// the index is rewritten from scratch. `orbit index rebuild` runs this;
/// indexed reads rebuild lazily whenever entries go stale.
pub fn rebuild(board: &Board) -> Result<Rebuilt> {
    let ids = board.task_ids()?;
    let mut prints = HashMap::with_capacity(ids.len());
    let mut tasks = HashMap::with_capacity(ids.len());
    let mut skipped = 0usize;
    for id in &ids {
        prints.insert(id.clone(), fingerprint(&board.task_path(id)));
        let Ok(content) = std::fs::read_to_string(board.task_path(id)) else {
            skipped += 1;
            continue;
        };
        match task::parse_task(id.clone(), &content) {
            Ok(task) => {
                tasks.insert(id.clone(), task);
            }
            Err(_) => skipped += 1,
        }
    }
    let plan = Plan {
        tasks: HashMap::new(),
        rescan: Vec::new(),
        prints,
    };
    store_inner(board, &tasks, &plan)?;
    let head = crate::git::rev_parse(&board.root, "HEAD")?;
    Ok(Rebuilt {
        entries: tasks.len(),
        skipped,
        head,
    })
}

/// The state `orbit index status` reports.
pub struct Status {
    pub enabled: bool,
    pub path: PathBuf,
    pub present: bool,
    pub entries: usize,
    /// Entries whose file still matches its fingerprint.
    pub fresh: usize,
    /// Entries behind their file, or pointing at deleted files.
    pub stale: usize,
    /// Task files with no index entry at all.
    pub untracked: usize,
    pub head: Option<String>,
    pub built_at: Option<DateTime<Utc>>,
}

pub fn status(board: &Board) -> Result<Status> {
    let path = index_path(board);
    let mut status = Status {
        enabled: board.config.index.enabled,
        path,
        present: false,
        entries: 0,
        fresh: 0,
        stale: 0,
        untracked: 0,
        head: None,
        built_at: None,
    };
    let Some(file) = read_index(board) else {
        return Ok(status);
    };
    status.present = true;
    status.head = file.head;
    status.built_at = Some(file.built_at);
    status.entries = file.entries.len();
    let ids = board.task_ids()?;
    let live: HashSet<&String> = ids.iter().collect();
    for id in &ids {
        match file.entries.get(id) {
            Some(entry) => match fingerprint(&board.task_path(id)) {
                Some(print) if print == entry.print => status.fresh += 1,
                _ => status.stale += 1,
            },
            None => status.untracked += 1,
        }
    }
    status.stale += file.entries.keys().filter(|id| !live.contains(id)).count();
    Ok(status)
}

/// `orbit index status` / `orbit index rebuild`.
pub fn cmd_index(
    board: &Board,
    command: crate::cli::IndexCommand,
) -> Result<crate::output::CommandOutput> {
    match command {
        crate::cli::IndexCommand::Status => {
            let status = status(board)?;
            let data = serde_json::json!({
                "enabled": status.enabled,
                "path": status.path.display().to_string(),
                "present": status.present,
                "entries": status.entries,
                "fresh": status.fresh,
                "stale": status.stale,
                "untracked": status.untracked,
                "head": status.head,
                "built_at": status.built_at,
            });
            let human = if status.present {
                format!(
                    "index: {} ({} entries, {} fresh, {} stale, {} untracked{})\n  {}",
                    if status.enabled {
                        "enabled"
                    } else {
                        "present but disabled"
                    },
                    status.entries,
                    status.fresh,
                    status.stale,
                    status.untracked,
                    status
                        .head
                        .as_deref()
                        .map(|head| format!(", head {}", &head[..head.len().min(12)]))
                        .unwrap_or_default(),
                    status.path.display()
                )
            } else {
                format!(
                    "index: {} (no index at {})",
                    if status.enabled {
                        "enabled"
                    } else {
                        "disabled"
                    },
                    status.path.display()
                )
            };
            Ok(crate::output::CommandOutput::new(data, human))
        }
        crate::cli::IndexCommand::Rebuild => {
            let rebuilt = rebuild(board)?;
            let data = serde_json::json!({
                "rebuilt": true,
                "entries": rebuilt.entries,
                "skipped": rebuilt.skipped,
                "head": rebuilt.head,
                "path": index_path(board).display().to_string(),
            });
            let human = format!(
                "rebuilt index: {} entries{}",
                rebuilt.entries,
                if rebuilt.skipped > 0 {
                    format!(", {} skipped", rebuilt.skipped)
                } else {
                    String::new()
                }
            );
            Ok(crate::output::CommandOutput::new(data, human))
        }
    }
}

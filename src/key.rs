//! Sequential Issue keys (`ORB-N`) and their allocation state (ADR 0022).
//!
//! Every Issue keeps its immutable internal Task ID (`orb-…`, the file name);
//! the key is the human- and token-cheap handle allocated under the
//! exclusive Board lock. `keys.toml` at the Board root records the key
//! prefix and the next number to hand out, so keys are never reused after a
//! Task is deleted. Board sync merges `keys.toml` by maximum and re-keys
//! Issues whose keys collide, recording the old key as an alias on the Task.

use std::collections::BTreeSet;
use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::board::{Board, IdConfig};
use crate::error::{OrbitError, Result};
use crate::graph::Graph;
use crate::task;

/// Board-root file holding the key allocator state.
pub const STATE_FILE: &str = "keys.toml";

/// Fallback key prefix when `id.prefix` cannot derive one.
pub const DEFAULT_KEY_PREFIX: &str = "ORB";

/// The allocator state persisted in `keys.toml`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct KeyState {
    /// Uppercase key prefix, e.g. `ORB` in `ORB-12`. Empty derives from
    /// `id.prefix` at load.
    #[serde(default)]
    pub prefix: String,
    /// The next sequential number to allocate; one past the highest ever
    /// handed out, so deleted Issues never free a key for reuse.
    #[serde(default)]
    pub next_key: u64,
}

/// Derives the default key prefix from the Board's Task-ID prefix:
/// `orb-` becomes `ORB`, `acme-` becomes `ACME`. Falls back to `ORB` when
/// the prefix has no usable letters.
pub fn derive_prefix(id: &IdConfig) -> String {
    let derived = id.prefix.trim_end_matches('-').to_uppercase();
    if is_valid_prefix(&derived) {
        derived
    } else {
        DEFAULT_KEY_PREFIX.to_string()
    }
}

fn is_valid_prefix(prefix: &str) -> bool {
    !prefix.is_empty()
        && prefix.len() <= 32
        && prefix
            .bytes()
            .all(|b| b.is_ascii_uppercase() || b.is_ascii_digit() || b == b'-')
        && prefix.bytes().any(|b| b.is_ascii_uppercase())
}

/// The state `keys.toml` carries, or the derived defaults when the file is
/// absent (a Board that predates keys or has not added an Issue yet).
pub fn load(board: &Board) -> Result<KeyState> {
    let path = board.root.join(STATE_FILE);
    let content = match std::fs::read_to_string(&path) {
        Ok(content) => content,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Ok(KeyState {
                prefix: derive_prefix(&board.config.id),
                next_key: 1,
            });
        }
        Err(error) => return Err(OrbitError::io(format!("read {}", path.display()), error)),
    };
    let mut state: KeyState = toml::from_str(&content)
        .map_err(|error| OrbitError::config(format!("invalid {}: {error}", path.display())))?;
    if state.prefix.is_empty() {
        state.prefix = derive_prefix(&board.config.id);
    }
    if !is_valid_prefix(&state.prefix) {
        return Err(OrbitError::config(format!(
            "{STATE_FILE} has invalid prefix `{}`",
            state.prefix
        )));
    }
    Ok(state)
}

fn render(state: &KeyState) -> Result<String> {
    toml::to_string_pretty(state)
        .map_err(|error| OrbitError::config(format!("serialize {STATE_FILE}: {error}")))
}

/// Writes `keys.toml` with the given state.
pub fn save(board: &Board, state: &KeyState) -> Result<()> {
    let path = board.root.join(STATE_FILE);
    crate::atomic::write_atomic(&path, render(state)?.as_bytes(), true)
}

/// Writes `keys.toml` when the allocator state changed. The caller holds the
/// exclusive Board lock; the file lands in the same commit as the Task files.
pub fn persist(board: &Board, graph: &Graph) -> Result<()> {
    let mut state = load(board)?;
    let next = state.next_key.max(max_number(graph) + 1);
    if next == state.next_key {
        return Ok(());
    }
    state.next_key = next;
    save(board, &state)
}

/// The next key to allocate: the persisted high-water mark, but never below
/// one past the highest key or alias already present in the Graph (covers a
/// `keys.toml` that lags the Tasks, e.g. hand-built Boards).
pub fn allocate(graph: &Graph, board: &Board) -> Result<String> {
    let state = load(board)?;
    let number = state.next_key.max(max_number(graph) + 1);
    Ok(format!("{}-{number}", state.prefix))
}

/// The trailing number of a key (`ORB-12` → 12), whatever its prefix.
pub fn key_number(key: &str) -> Option<u64> {
    let (_, number) = key.rsplit_once('-')?;
    if number.is_empty() || !number.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    number.parse().ok()
}

/// Whether `value` has the shape of an Issue key: an uppercase prefix of
/// letters/digits/dashes followed by `-` and a decimal number.
pub fn is_key_shaped(value: &str) -> bool {
    let Some((prefix, number)) = value.rsplit_once('-') else {
        return false;
    };
    !prefix.is_empty()
        && prefix.len() <= 32
        && prefix
            .bytes()
            .all(|b| b.is_ascii_uppercase() || b.is_ascii_digit() || b == b'-')
        && prefix.bytes().any(|b| b.is_ascii_uppercase())
        && !number.is_empty()
        && number.len() <= 15
        && number.bytes().all(|b| b.is_ascii_digit())
}

/// Canonical form used by the Graph indices: uppercase, since keys are
/// case-insensitive on input (`orb-12` resolves `ORB-12`).
pub fn normalize(reference: &str) -> String {
    reference.to_uppercase()
}

/// Every key-shaped string a Task carries: its live key plus its aliases.
pub fn task_keys(front: &task::TaskFrontmatter) -> impl Iterator<Item = &str> {
    front
        .key
        .iter()
        .chain(front.key_aliases.iter())
        .map(String::as_str)
}

/// Highest key number across live keys and aliases; 0 when none exist.
pub fn max_number(graph: &Graph) -> u64 {
    graph
        .iter()
        .flat_map(|task| task_keys(&task.front))
        .filter_map(key_number)
        .max()
        .unwrap_or(0)
}

/// Assigns `task` a fresh key and records `old` as an alias — the re-key
/// step Board sync runs on key collisions. Returns the notices to surface.
pub fn rekey(task: &mut task::Task, old: &str, next: u64, prefix: &str) -> String {
    let alias = old.to_string();
    if !task.front.key_aliases.contains(&alias) {
        task.front.key_aliases.push(alias);
        task.front.key_aliases.sort();
        task.front.key_aliases.dedup();
    }
    let new_key = format!("{prefix}-{next}");
    task.front.key = Some(new_key.clone());
    new_key
}

/// The set of live keys held by more than one Task, for validation.
pub fn duplicate_keys(graph: &Graph) -> BTreeSet<String> {
    let mut seen = BTreeSet::new();
    let mut duplicates = BTreeSet::new();
    for task in graph.iter() {
        if let Some(key) = &task.front.key {
            let normalized = normalize(key);
            if !seen.insert(normalized.clone()) {
                duplicates.insert(normalized);
            }
        }
    }
    duplicates
}

/// Alias → Task ID index entries for one Task, in deterministic order.
pub fn alias_entries(task: &task::Task) -> Vec<(String, String)> {
    let mut entries: Vec<(String, String)> = task
        .front
        .key_aliases
        .iter()
        .map(|alias| (normalize(alias), task.id.clone()))
        .collect();
    entries.sort();
    entries
}

/// Live key → Task ID index entries for one Task.
pub fn key_entries(task: &task::Task) -> Vec<(String, String)> {
    task.front
        .key
        .iter()
        .map(|key| (normalize(key), task.id.clone()))
        .collect()
}

/// Path of the allocator state file, for sync's file classifier.
pub fn state_path(root: &Path) -> std::path::PathBuf {
    root.join(STATE_FILE)
}

/// Re-keys every Task in `tasks` that shares a live project key, keeping the
/// lowest internal ID on the contested key — the same deterministic rule
/// Board sync applies, so both sides of a merge compute the same outcome.
/// `next` is the first free number (one past the highest key or alias);
/// it advances past every re-key. Returns `(task ID, old key, new key)`
/// for each re-keyed Task; the caller writes the files and persists
/// `keys.toml`.
pub fn rekey_collisions(
    tasks: &mut std::collections::HashMap<String, task::Task>,
    prefix: &str,
    next: &mut u64,
) -> Vec<(String, String, String)> {
    let mut by_key: std::collections::BTreeMap<String, Vec<String>> =
        std::collections::BTreeMap::new();
    for task in tasks.values() {
        if let Some(key) = &task.front.key {
            by_key
                .entry(normalize(key))
                .or_default()
                .push(task.id.clone());
        }
    }
    let mut rekeyed = Vec::new();
    for ids in by_key.into_values() {
        if ids.len() < 2 {
            continue;
        }
        let mut ids = ids;
        ids.sort();
        for id in ids.into_iter().skip(1) {
            let Some(task) = tasks.get_mut(&id) else {
                continue;
            };
            let old = task.front.key.clone().unwrap_or_else(|| task.id.clone());
            let new_key = rekey(task, &old, *next, prefix);
            *next += 1;
            rekeyed.push((id, old, new_key));
        }
    }
    rekeyed
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn key_numbers_and_shapes() {
        assert_eq!(key_number("ORB-12"), Some(12));
        assert_eq!(key_number("orb-3"), Some(3));
        assert_eq!(key_number("ORB-"), None);
        assert_eq!(key_number("nope"), None);
        assert!(is_key_shaped("ORB-12"));
        assert!(is_key_shaped("ACME-1"));
        assert!(!is_key_shaped("orb-12"));
        assert!(!is_key_shaped("ORB-x"));
        assert!(!is_key_shaped("12"));
    }

    #[test]
    fn derive_prefix_uppercases_the_id_prefix() {
        let id = IdConfig {
            prefix: "orb-".to_string(),
            length: 6,
            accepted_prefixes: Vec::new(),
        };
        assert_eq!(derive_prefix(&id), "ORB");
        let odd = IdConfig {
            prefix: "123-".to_string(),
            length: 6,
            accepted_prefixes: Vec::new(),
        };
        assert_eq!(derive_prefix(&odd), DEFAULT_KEY_PREFIX);
    }

    #[test]
    fn rekey_collisions_keeps_the_lowest_id_and_records_the_alias() {
        let mut tasks = std::collections::HashMap::new();
        for (id, key) in [
            ("orb-bbbbbb".to_string(), "ORB-7"),
            ("orb-aaaaaa".to_string(), "ORB-7"),
            ("orb-cccccc".to_string(), "ORB-9"),
        ] {
            let front = task::TaskFrontmatter {
                title: id.clone(),
                status: "todo".to_string(),
                key: Some(key.to_string()),
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
                reporter: None,
                created_at: "2025-01-01T00:00:00Z".parse().unwrap(),
                updated_at: "2025-01-01T00:00:00Z".parse().unwrap(),
                source_key: None,
                properties: Default::default(),
                extra: Default::default(),
            };
            tasks.insert(
                id.clone(),
                task::Task {
                    id,
                    front,
                    body: String::new(),
                },
            );
        }
        let mut next = 10;
        let rekeyed = rekey_collisions(&mut tasks, "ORB", &mut next);
        assert_eq!(rekeyed.len(), 1);
        let (id, old, new) = &rekeyed[0];
        assert_eq!(id, "orb-bbbbbb");
        assert_eq!(old, "ORB-7");
        assert_eq!(new, "ORB-10");
        assert_eq!(next, 11);
        let loser = &tasks["orb-bbbbbb"];
        assert_eq!(loser.front.key.as_deref(), Some("ORB-10"));
        assert_eq!(loser.front.key_aliases, vec!["ORB-7".to_string()]);
        assert_eq!(tasks["orb-aaaaaa"].front.key.as_deref(), Some("ORB-7"));
        assert_eq!(tasks["orb-cccccc"].front.key.as_deref(), Some("ORB-9"));
    }
}

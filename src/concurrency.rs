//! Optimistic concurrency (`--if-version`) and write idempotency
//! (`--idempotency-key`) shared by every Board mutation (ADR 0022).
//!
//! A Task's version is its `updated_at` timestamp — the value `orbit show`
//! reports. `--if-version` compares it under the exclusive Board lock before
//! the mutation runs, so a stale version fails with a retryable
//! `VERSION_CONFLICT` instead of silently overwriting a newer write.
//!
//! `--idempotency-key` makes a write retry-safe: the first successful result
//! is recorded under the Board cache directory (`<orbit>/cache/idempotency/`,
//! outside the tracked tree) and a repeat call with the same key replays that
//! recorded result instead of running again. Only successful outputs are
//! recorded; a failed call leaves the key free for a real retry.

use std::path::PathBuf;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};

use crate::board::{Board, board_cache_dir};
use crate::error::{OrbitError, Result, code};
use crate::graph::{Graph, LoadMode};
use crate::output::CommandOutput;

/// Longest idempotency key, in characters — the same shape rules as a
/// Source Key.
pub const IDEMPOTENCY_KEY_MAX: usize = 200;

/// Checks an idempotency key: 1 to 200 printable, non-whitespace ASCII
/// characters.
pub fn validate_idempotency_key(key: &str) -> Result<()> {
    if key.is_empty()
        || key.len() > IDEMPOTENCY_KEY_MAX
        || !key.bytes().all(|b| b.is_ascii_graphic())
    {
        return Err(OrbitError::new(
            code::INVALID_IDEMPOTENCY_KEY,
            format!(
                "idempotency key must be 1 to {IDEMPOTENCY_KEY_MAX} printable ASCII characters without spaces"
            ),
        ));
    }
    Ok(())
}

/// Parses an `--if-version` value: an RFC 3339 timestamp such as the
/// `updated_at` `orbit show` prints.
pub fn parse_version(value: &str) -> Result<DateTime<Utc>> {
    DateTime::parse_from_rfc3339(value.trim())
        .map(|parsed| parsed.with_timezone(&Utc))
        .map_err(|_| {
            OrbitError::invalid_argument(format!(
                "invalid --if-version `{value}`; pass the Task's `updated_at` from `orbit show <id>`"
            ))
        })
}

/// The retryable conflict a stale `--if-version` produces.
pub fn version_conflict(id: &str, expected: DateTime<Utc>, actual: DateTime<Utc>) -> OrbitError {
    OrbitError::new(
        code::VERSION_CONFLICT,
        format!(
            "{id} changed since version {}; its current version is {}",
            expected.to_rfc3339(),
            actual.to_rfc3339()
        ),
    )
    .retryable()
    .with_details(json!({
        "issue": id,
        "expected_version": expected.to_rfc3339(),
        "actual_version": actual.to_rfc3339(),
    }))
}

/// Verifies `reference`'s Task still carries `expected` as its `updated_at`.
/// Runs under the caller's exclusive Board lock, after resolution so project
/// keys and aliases work like everywhere else.
pub fn check_version(board: &Board, reference: &str, expected: &str) -> Result<()> {
    let expected_at = parse_version(expected)?;
    let (graph, _) = Graph::load(board, LoadMode::Lenient)?;
    let id = graph.resolve(reference)?;
    let task = graph.get(&id)?;
    if task.front.updated_at != expected_at {
        return Err(version_conflict(&id, expected_at, task.front.updated_at));
    }
    Ok(())
}

// --- Idempotency records -----------------------------------------------------

/// The stored form of a successful command result.
#[derive(Debug, Clone, Serialize, Deserialize)]
struct StoredResult {
    data: Value,
    human: String,
    #[serde(default)]
    meta: Option<Value>,
    #[serde(default)]
    warnings: Vec<String>,
    #[serde(default)]
    exit_code: i32,
}

#[derive(Debug, Serialize, Deserialize)]
struct IdempotencyRecord {
    /// Record format version; bump when the shape changes.
    version: u32,
    key: String,
    /// SHA-256 of the request that produced `result`; a reused key with a
    /// different request is rejected instead of replayed. Empty on records
    /// written before the fingerprint existed (those replay unchecked).
    #[serde(default)]
    fingerprint: String,
    created_at: DateTime<Utc>,
    result: StoredResult,
}

const RECORD_VERSION: u32 = 1;

fn records_dir(board: &Board) -> PathBuf {
    board_cache_dir(board).join("idempotency")
}

/// The record path for `key`: a hash keeps arbitrary keys filesystem-safe.
fn record_path(board: &Board, key: &str) -> PathBuf {
    records_dir(board).join(format!("{}.json", hex(&Sha256::digest(key.as_bytes()))))
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

/// Fingerprint of the invoking command line minus the idempotency flag, so the
/// same key can only ever replay the same request (CLI and MCP both reach
/// orbit through argv).
fn request_fingerprint() -> String {
    let mut hasher = Sha256::new();
    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        if arg == "--idempotency-key" {
            args.next();
            continue;
        }
        if arg.starts_with("--idempotency-key=") {
            continue;
        }
        hasher.update((arg.len() as u64).to_le_bytes());
        hasher.update(arg.as_bytes());
    }
    hex(&hasher.finalize())
}

fn read_record(path: &std::path::Path) -> Result<Option<IdempotencyRecord>> {
    let text = match std::fs::read_to_string(path) {
        Ok(text) => text,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(OrbitError::io(format!("read {}", path.display()), error)),
    };
    let record: IdempotencyRecord = serde_json::from_str(&text).map_err(|error| {
        OrbitError::new(
            code::IO_ERROR,
            format!("corrupt idempotency record {}: {error}", path.display()),
        )
    })?;
    Ok(Some(record))
}

/// Returns the recorded result for `key`, marked as a replay, or `None` when
/// the key was never used. A corrupt record is an error, not a silent miss:
/// replaying wrongly is worse than failing.
pub fn lookup(board: &Board, key: &str) -> Result<Option<CommandOutput>> {
    validate_idempotency_key(key)?;
    let Some(record) = read_record(&record_path(board, key))? else {
        return Ok(None);
    };
    if record.key != key || record.version != RECORD_VERSION {
        // A hash collision or a format change reads as a miss; the new call
        // simply records over the stale file.
        return Ok(None);
    }
    if !record.fingerprint.is_empty() && record.fingerprint != request_fingerprint() {
        return Err(OrbitError::new(
            code::IDEMPOTENCY_KEY_REUSED,
            format!("idempotency key `{key}` was already used for a different request"),
        ));
    }
    let stored = record.result;
    let mut output = CommandOutput::new(stored.data, stored.human);
    output.meta = stored.meta;
    output.warnings = stored.warnings;
    output.exit_code = stored.exit_code;
    let mut meta = match output.meta.take() {
        Some(Value::Object(map)) => map,
        Some(value) => serde_json::Map::from_iter([("value".to_string(), value)]),
        None => serde_json::Map::new(),
    };
    meta.insert("idempotent_replay".to_string(), Value::Bool(true));
    output.meta = Some(Value::Object(meta));
    Ok(Some(output))
}

/// Records `output` under `key`. Call only after the mutation committed;
/// failures surface as a warning because the write itself already happened.
pub fn store(board: &Board, key: &str, output: &CommandOutput) -> Result<()> {
    validate_idempotency_key(key)?;
    let record = IdempotencyRecord {
        version: RECORD_VERSION,
        key: key.to_string(),
        fingerprint: request_fingerprint(),
        created_at: Utc::now(),
        result: StoredResult {
            data: output.data.clone(),
            human: output.human.clone(),
            meta: output.meta.clone(),
            warnings: output.warnings.clone(),
            exit_code: output.exit_code,
        },
    };
    let text = serde_json::to_string_pretty(&record)
        .map_err(|error| OrbitError::config(format!("serialize idempotency record: {error}")))?;
    let path = record_path(board, key);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|error| OrbitError::io(format!("create {}", parent.display()), error))?;
    }
    crate::atomic::write_atomic(&path, format!("{text}\n").as_bytes(), true)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn version_parses_rfc3339() {
        let parsed = parse_version("2026-11-16T14:58:25.460851Z").unwrap();
        assert_eq!(parsed.to_rfc3339(), "2026-11-16T14:58:25.460851+00:00");
        assert!(parse_version("not-a-date").is_err());
        assert!(parse_version("").is_err());
    }

    #[test]
    fn idempotency_key_validation() {
        assert!(validate_idempotency_key("retry-1").is_ok());
        assert!(validate_idempotency_key("").is_err());
        assert!(validate_idempotency_key("has space").is_err());
        assert!(validate_idempotency_key(&"x".repeat(201)).is_err());
    }
}

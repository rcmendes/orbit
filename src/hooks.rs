//! Board event hooks (ADR 0022 opt-in module).
//!
//! `[[hooks]]` rules in `config.toml` bind feed event kinds to local shell
//! commands. After a write command commits, the events its commits produced
//! are replayed and every matching rule fires: the command is spawned
//! detached through `sh -c` with the event in `ORBIT_EVENT_*` variables.
//!
//! Trust: `config.toml` is synced with the Board, so hook commands run only on
//! machines that opt in with `ORBIT_TRUST_HOOKS=1`; otherwise a write warns
//! `HOOKS_UNTRUSTED` and runs nothing (ADR 0022).
//!
//! Hooks are strictly best-effort. They run detached so they never block a
//! write, their exit status is ignored, and a spawn failure degrades to a
//! command warning rather than an error. Hook stdout and stderr append to
//! `cache/hooks.log` so a hook can log without holding the caller's pipes.
//!
//! ```toml
//! [[hooks]]
//! events = ["issue.transitioned", "issue.created"]
//! command = "echo $ORBIT_EVENT_TYPE $ORBIT_EVENT_KEY >> /tmp/orbit.log"
//!
//! [[hooks]]
//! events = ["*"]
//! command = "$HOME/bin/orbit-notify"
//! ```

use std::process::{Command, Stdio};

use crate::board::{Board, board_cache_dir};
use crate::error::{OrbitError, Result};
use crate::feed::{self, BoardEvent};

/// The event kinds a hook rule may subscribe to (plus `*` for every event),
/// mirroring the kinds [`feed::collect`] derives from Board commits.
pub const EVENT_KINDS: &[&str] = &[
    "issue.created",
    "issue.updated",
    "issue.transitioned",
    "issue.assigned",
    "issue.checkpointed",
    "issue.deleted",
    "issue.commented",
    "issue.comments_deleted",
    "board.changed",
];

// The `[[hooks]]` rule schema lives in `crate::board` (kernel-owned
// config.toml schema); this module matches, validates, and spawns rules.
pub use crate::board::HookRule;

impl HookRule {
    /// Whether this rule fires for `kind`.
    fn matches(&self, kind: &str) -> bool {
        self.events
            .iter()
            .any(|event| event == "*" || event == kind)
    }
}

/// Validates `[[hooks]]` rules; called from `crate::modules::validate` in
/// the application layer so a misspelled event name or empty rule fails
/// any Board command loudly.
pub fn validate(rules: &[HookRule]) -> Result<()> {
    for (index, rule) in rules.iter().enumerate() {
        if rule.events.is_empty() {
            return Err(OrbitError::config(format!(
                "hooks[{index}].events must not be empty; list event kinds or \"*\""
            )));
        }
        for event in &rule.events {
            if event != "*" && !EVENT_KINDS.contains(&event.as_str()) {
                return Err(OrbitError::config(format!(
                    "hooks[{index}].events names unknown event kind `{event}`; expected `*` or one of: {}",
                    EVENT_KINDS.join(", ")
                )));
            }
        }
        if rule.command.trim().is_empty() {
            return Err(OrbitError::config(format!(
                "hooks[{index}].command must not be empty"
            )));
        }
    }
    Ok(())
}

/// Per-machine opt-in for running Board-defined hook commands.
pub const TRUST_ENV: &str = "ORBIT_TRUST_HOOKS";

fn trusted() -> bool {
    std::env::var(TRUST_ENV).is_ok_and(|value| value == "1")
}

/// Whether the Board has any hook rules configured.
pub fn enabled(board: &Board) -> bool {
    !board.config.hooks.is_empty()
}

/// Replays the events produced since `since` — the Board head captured
/// before the write — and fires every matching rule detached. Returns only
/// warnings: a hook never blocks or fails a write.
pub fn fire_since(board: &Board, since: &str) -> Vec<String> {
    if !enabled(board) {
        return Vec::new();
    }
    // `config.toml` travels with the Board (pushed and pulled), so a hook rule is
    // untrusted input on any machine that did not write it: run nothing until the
    // local user opts in.
    if !trusted() {
        return vec![format!(
            "HOOKS_UNTRUSTED: this Board defines [[hooks]] but {TRUST_ENV}=1 is not set on this machine; no hook ran"
        )];
    }
    let events = match feed::collect(board, Some(since)) {
        Ok(feed) => feed.events,
        Err(error) => {
            return vec![format!(
                "HOOKS_SKIPPED: could not read Board events: {}",
                error.message
            )];
        }
    };
    let mut warnings = Vec::new();
    for event in &events {
        for rule in &board.config.hooks {
            if rule.matches(&event.kind)
                && let Err(warning) = spawn(board, rule, event)
            {
                warnings.push(warning);
            }
        }
    }
    warnings
}

/// Spawns `rule.command` detached for `event`. Stdin is closed, output goes
/// to `cache/hooks.log`, and the caller never waits — a slow or failing
/// hook cannot block the write that fired it.
fn spawn(board: &Board, rule: &HookRule, event: &BoardEvent) -> std::result::Result<(), String> {
    let event_json = serde_json::to_string(event).unwrap_or_default();
    let mut command = shell(&rule.command);
    command
        .current_dir(&board.root)
        .env("ORBIT_EVENT_JSON", &event_json)
        .env("ORBIT_EVENT_ID", &event.id)
        .env("ORBIT_EVENT_TYPE", &event.kind)
        .env("ORBIT_EVENT_ACTOR", &event.actor)
        .env("ORBIT_EVENT_COMMIT", &event.commit)
        .env("ORBIT_EVENT_AT", &event.at)
        .env("ORBIT_BOARD_ROOT", &board.root)
        .stdin(Stdio::null());
    if let Some(issue) = &event.issue {
        command.env("ORBIT_EVENT_ISSUE", issue);
    }
    if let Some(key) = &event.key {
        command.env("ORBIT_EVENT_KEY", key);
    }
    if let Some(verb) = &event.verb {
        command.env("ORBIT_EVENT_VERB", verb);
    }
    match hook_log(board) {
        Ok(log) => {
            if let Ok(stderr) = log.try_clone() {
                command.stderr(stderr);
            }
            command.stdout(log);
        }
        Err(_) => {
            command.stdout(Stdio::null()).stderr(Stdio::null());
        }
    }
    command
        .spawn()
        .map(|mut child| {
            // Reap the child so a long-lived `orbit mcp` does not accumulate zombies.
            std::thread::spawn(move || {
                let _ = child.wait();
            });
        })
        .map_err(|error| {
            format!(
                "HOOK_SPAWN_FAILED: `{}` could not start: {error}",
                rule.command
            )
        })
}

/// `/bin/sh -c <command>` on Unix (the absolute path survives a stripped
/// `PATH`, as in agent harnesses); `cmd /C` elsewhere.
fn shell(command: &str) -> Command {
    #[cfg(unix)]
    {
        let mut shell = Command::new("/bin/sh");
        shell.arg("-c").arg(command);
        shell
    }
    #[cfg(not(unix))]
    {
        let mut shell = Command::new("cmd");
        shell.arg("/C").arg(command);
        shell
    }
}

/// Opens `cache/hooks.log` for appending, creating the cache directory when
/// needed. The Board cache sits outside the tracked files, so hook chatter
/// never dirties the Board.
fn hook_log(board: &Board) -> std::io::Result<std::fs::File> {
    let cache = board_cache_dir(board);
    std::fs::create_dir_all(&cache)?;
    std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(cache.join("hooks.log"))
}

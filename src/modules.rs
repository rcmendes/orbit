//! Application-layer glue for ADR 0022's opt-in modules.
//!
//! The kernel (`board.rs`, `graph.rs`, `ops.rs`, …) owns the `config.toml`
//! *schema* for every section — including the module-owned `[scheduler]`,
//! `[[hooks]]`, `[index]`, and `[[profiles]]` — but never interprets or
//! validates their semantics, so nothing in the kernel depends on a module.
//! Semantic validation happens here, where depending on modules is allowed:
//! the command layer calls [`validate`] right after opening a Board, and
//! callers that validate a `Config` directly (migrations, setup) call it
//! alongside `Config::validate`.

use crate::board::Config;
use crate::error::Result;

/// Runs every opt-in module's config validation over a loaded `Config`.
/// Module sections are skipped entirely when absent.
pub fn validate(config: &Config) -> Result<()> {
    config.scheduler.validate()?;
    crate::hooks::validate(&config.hooks)?;
    Ok(())
}

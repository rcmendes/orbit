//! `orbit changes` and `orbit history`: the named event feed derived from
//! Board Git history (ADR 0022, `crate::feed`). Both are read-only.

use serde_json::{Value, json};

use crate::board::Board;
use crate::cli::{ChangesArgs, HistoryArgs};
use crate::feed::BoardEvent;

use super::*;

/// `orbit changes [--since CURSOR] [--type KIND]... [--limit N]`.
pub(super) fn cmd_changes(
    board: &Board,
    args: ChangesArgs,
) -> Result<crate::output::CommandOutput> {
    let feed = crate::feed::collect(board, args.since.as_deref())?;
    let kinds: Vec<&str> = args.kinds.iter().map(String::as_str).collect();
    let mut events: Vec<BoardEvent> = feed
        .events
        .into_iter()
        .filter(|event| kinds.is_empty() || kinds.contains(&event.kind.as_str()))
        .collect();
    // `--limit` keeps the newest events; the cursor still reports the Board
    // head, so the window stays honest about what was skipped.
    if let Some(limit) = args.limit
        && events.len() > limit
    {
        events.drain(..events.len() - limit);
    }
    let count = events.len();
    let human = if events.is_empty() {
        "no change events".to_string()
    } else {
        events
            .iter()
            .map(format_event_line)
            .collect::<Vec<_>>()
            .join("\n")
    };
    Ok(crate::output::CommandOutput::new(
        json!({
            "events": events,
            "cursor": feed.cursor,
            "count": count,
        }),
        human,
    )
    .with_meta(json!({ "count": count })))
}

/// `orbit history <id> [--limit N]`: one Issue's timeline, oldest first.
///
/// Matching is by the event's recorded Task ID or project key, so the
/// timeline still resolves after the Issue is deleted or its key changed.
pub(super) fn cmd_history(
    board: &Board,
    args: HistoryArgs,
) -> Result<crate::output::CommandOutput> {
    let wanted = args.id.trim();
    if wanted.is_empty() {
        return Err(OrbitError::invalid_argument(
            "history needs an Issue ID or key",
        ));
    }
    let feed = crate::feed::collect(board, None)?;
    let mut events: Vec<BoardEvent> = feed
        .events
        .into_iter()
        .filter(|event| {
            event.issue.as_deref().is_some_and(|id| id == wanted)
                || event
                    .key
                    .as_deref()
                    .is_some_and(|key| key.eq_ignore_ascii_case(wanted))
        })
        .collect();
    if let Some(limit) = args.limit
        && events.len() > limit
    {
        events.drain(..events.len() - limit);
    }
    let count = events.len();
    let reference = events
        .iter()
        .find_map(|event| event.key.clone().or_else(|| event.issue.clone()))
        .unwrap_or_else(|| wanted.to_string());
    let human = if events.is_empty() {
        format!("no recorded events for {wanted}")
    } else {
        events
            .iter()
            .map(format_event_line)
            .collect::<Vec<_>>()
            .join("\n")
    };
    Ok(crate::output::CommandOutput::new(
        json!({
            "issue": reference,
            "events": events,
            "count": count,
        }),
        human,
    )
    .with_meta(json!({ "count": count })))
}

/// One human-readable feed line: `at  type  actor  ref  hint`.
fn format_event_line(event: &BoardEvent) -> String {
    let reference = event
        .key
        .clone()
        .or_else(|| event.issue.clone())
        .unwrap_or_else(|| "board".to_string());
    let mut line = format!(
        "{}  {:<24} {:<20} {}",
        event.at, event.kind, event.actor, reference
    );
    if let Some(hint) = event_hint(event) {
        line.push_str("  ");
        line.push_str(&hint);
    }
    line
}

/// A short detail suffix for the kinds that carry one.
fn event_hint(event: &BoardEvent) -> Option<String> {
    let data = &event.data;
    if let Some(status) = data.get("status") {
        if let Some(object) = status.as_object() {
            return Some(format!(
                "{} → {}",
                object["from"].as_str().unwrap_or("?"),
                object["to"].as_str().unwrap_or("?")
            ));
        }
        if let Some(status) = status.as_str() {
            return Some(status.to_string());
        }
    }
    if let Some(title) = data.get("title") {
        if let Some(object) = title.as_object() {
            return Some(format!(
                "\"{}\" → \"{}\"",
                object["from"].as_str().unwrap_or("?"),
                object["to"].as_str().unwrap_or("?")
            ));
        }
        if let Some(title) = title.as_str() {
            return Some(format!("\"{title}\""));
        }
    }
    data.get("detail")
        .and_then(Value::as_str)
        .map(str::to_string)
        .filter(|detail| !detail.is_empty())
}

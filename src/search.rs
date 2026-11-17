//! Query language, JSON filters, field projection, and the pagination
//! cursor codec behind `orbit search`, `orbit list --filter/--fields/--cursor`,
//! and `orbit show --fields` (ADR 0022, "Query adds qualifier search
//! (`status:`, `assignee:`, `label:`), JSON filters, `--fields`, cursor
//! pagination").
//!
//! **Search query.** [`parse_query`] splits a `orbit search` argument into
//! `key:value` qualifiers and free-text terms. Qualifiers: `status:`,
//! `assignee:`, `label:` (repeatable, every one must match), `type:`,
//! `priority:`, `component:`, and `repository:`/`repo:`. Scalar qualifiers
//! are repeatable with any-of semantics; free-text terms must all appear in
//! the Issue's searchable text (title, body, ID, project key, labels,
//! status, type, assignee, component, repository — case-insensitive).
//!
//! **JSON filter.** [`matches_filter`] evaluates a `--filter` JSON object
//! against an Issue's summary object: every key must match (AND). A scalar
//! expected value requires equality — or membership when the actual value is
//! an array (`{"labels": "backend"}` matches an Issue carrying that label);
//! an array expected value is an any-of for scalar fields
//! (`{"status": ["ready", "in_progress"]}`) and an all-of for array fields
//! (`{"labels": ["a", "b"]}`).
//!
//! **Field projection.** [`project_fields`] reduces each output object to
//! the requested top-level or dotted paths (`--fields id,title,status`).
//!
//! **Cursor.** [`encode_cursor`]/[`decode_cursor`] are an opaque hex codec
//! for the JSON payload a page returns as `next_cursor`; the sort-key
//! comparison lives with the commands so a cursor only ever resumes the
//! ordering that produced it.

use std::collections::BTreeMap;

use serde_json::{Map, Value};

use crate::error::{OrbitError, Result};
use crate::task::Task;

/// Qualifier keys `orbit search` understands, mapped to their canonical
/// spelling. `label` may repeat; scalar qualifiers accept repeats with
/// any-of semantics.
const QUALIFIERS: &[(&str, &str)] = &[
    ("status", "status"),
    ("assignee", "assignee"),
    ("claimed-by", "assignee"),
    ("label", "label"),
    ("type", "type"),
    ("priority", "priority"),
    ("component", "component"),
    ("repo", "repository"),
    ("repository", "repository"),
];

/// A parsed `orbit search` query: qualifiers plus free-text terms.
#[derive(Debug, Default)]
pub struct SearchQuery {
    /// Free-text terms, lowercased; every term must match the Issue.
    pub text: Vec<String>,
    pub status: Vec<String>,
    pub assignee: Vec<String>,
    /// Every listed label must be present (AND).
    pub labels: Vec<String>,
    pub types: Vec<String>,
    pub priorities: Vec<String>,
    pub components: Vec<String>,
    pub repository: Option<String>,
}

impl SearchQuery {
    /// Whether the query carries any constraint at all.
    pub fn is_empty(&self) -> bool {
        self.text.is_empty()
            && self.status.is_empty()
            && self.assignee.is_empty()
            && self.labels.is_empty()
            && self.types.is_empty()
            && self.priorities.is_empty()
            && self.components.is_empty()
            && self.repository.is_none()
    }
}

/// Splits a search argument into `key:value` qualifiers and free-text terms.
/// A token counts as a qualifier only when its prefix is a known qualifier
/// name, so `status:done` filters while `fix: login` stays text.
pub fn parse_query(input: &str) -> Result<SearchQuery> {
    let mut query = SearchQuery::default();
    for token in input.split_whitespace() {
        let (key, value) = match token.split_once(':') {
            Some((key, value)) if !value.is_empty() => (key.to_lowercase(), value),
            _ => {
                query.text.push(token.to_lowercase());
                continue;
            }
        };
        let Some((_, canonical)) = QUALIFIERS.iter().find(|(name, _)| *name == key) else {
            // `word:` with an unknown qualifier is text, not an error — the
            // colon may be part of the phrase being searched.
            query.text.push(token.to_lowercase());
            continue;
        };
        let value = value.trim_matches(|c| c == '"' || c == '\'');
        if value.is_empty() {
            return Err(OrbitError::invalid_argument(format!(
                "empty value for `{key}:` qualifier in search query `{input}`"
            )));
        }
        match *canonical {
            "status" => query.status.push(value.to_string()),
            "assignee" => query.assignee.push(value.to_string()),
            "label" => query.labels.push(value.to_string()),
            "type" => query.types.push(value.to_string()),
            "priority" => query.priorities.push(value.to_string()),
            "component" => query.components.push(value.to_string()),
            "repository" => query.repository = Some(value.to_string()),
            _ => unreachable!("canonical qualifier"),
        }
    }
    Ok(query)
}

/// The fields a free-text term may match, lowercased and concatenated.
fn haystack(task: &Task) -> String {
    let mut parts = vec![task.front.title.clone(), task.body.clone(), task.id.clone()];
    if let Some(key) = &task.front.key {
        parts.push(key.clone());
    }
    parts.extend(task.front.key_aliases.iter().cloned());
    parts.extend(task.front.labels.iter().cloned());
    parts.push(task.front.status.clone());
    parts.push(task.front.task_type.clone());
    if let Some(name) = task.assignee_name() {
        parts.push(name.to_string());
    }
    if let Some(component) = &task.front.component {
        parts.push(component.clone());
    }
    if let Some(repository) = &task.front.repository {
        parts.push(repository.clone());
    }
    parts.join("\n").to_lowercase()
}

/// The subset of the haystack used for metadata ranking: identifiers and
/// classifications, not prose.
fn metadata_text(task: &Task) -> String {
    let mut parts = vec![task.id.clone()];
    if let Some(key) = &task.front.key {
        parts.push(key.clone());
    }
    parts.extend(task.front.key_aliases.iter().cloned());
    parts.extend(task.front.labels.iter().cloned());
    parts.push(task.front.status.clone());
    parts.push(task.front.task_type.clone());
    if let Some(name) = task.assignee_name() {
        parts.push(name.to_string());
    }
    if let Some(component) = &task.front.component {
        parts.push(component.clone());
    }
    if let Some(repository) = &task.front.repository {
        parts.push(repository.clone());
    }
    parts.join("\n").to_lowercase()
}

/// Whether `task` satisfies every qualifier and every text term.
pub fn matches(task: &Task, query: &SearchQuery) -> bool {
    if !query.status.is_empty() && !query.status.iter().any(|s| s == &task.front.status) {
        return false;
    }
    if !query.assignee.is_empty() {
        let assignee = task.assignee_name();
        if !query
            .assignee
            .iter()
            .any(|name| Some(name.as_str()) == assignee)
        {
            return false;
        }
    }
    if !task.has_labels(&query.labels) {
        return false;
    }
    if !query.types.is_empty() && !query.types.iter().any(|t| t == &task.front.task_type) {
        return false;
    }
    if !query.priorities.is_empty()
        && !query
            .priorities
            .iter()
            .any(|p| p.eq_ignore_ascii_case(&task.front.priority))
    {
        return false;
    }
    if !query.components.is_empty() {
        let Some(component) = task.front.component.as_deref() else {
            return false;
        };
        if !query
            .components
            .iter()
            .any(|candidate| candidate == component)
        {
            return false;
        }
    }
    if let Some(repository) = &query.repository
        && !task
            .front
            .repository
            .as_deref()
            .is_some_and(|name| name.eq_ignore_ascii_case(repository))
    {
        return false;
    }
    if !query.text.is_empty() {
        let haystack = haystack(task);
        if !query.text.iter().all(|term| haystack.contains(term)) {
            return false;
        }
    }
    true
}

/// Relevance rank for ordering: 0 = a term hit the title (or no text
/// terms), 1 = a term hit only metadata (ID, key, labels, status, ...),
/// 2 = a term hit only the body.
pub fn rank(task: &Task, query: &SearchQuery) -> i64 {
    if query.text.is_empty() {
        return 0;
    }
    let title = task.front.title.to_lowercase();
    if query.text.iter().any(|term| title.contains(term)) {
        return 0;
    }
    let metadata = metadata_text(task);
    if query.text.iter().any(|term| metadata.contains(term)) {
        return 1;
    }
    2
}

const SNIPPET_WINDOW: usize = 200;

/// A short excerpt for one search hit: a window around the first matching
/// term, preferring the title, then the body; the title itself when the
/// query had no text terms.
pub fn snippet(task: &Task, query: &SearchQuery) -> String {
    if query.text.is_empty() {
        return truncate_chars(&task.front.title, SNIPPET_WINDOW);
    }
    let title = task.front.title.to_lowercase();
    if let Some(index) = query.text.iter().filter_map(|term| title.find(term)).min() {
        // Byte index → char-safe slice of the original title.
        return excerpt(&task.front.title, index, SNIPPET_WINDOW);
    }
    let body = task.body.to_lowercase();
    if let Some(index) = query.text.iter().filter_map(|term| body.find(term)).min() {
        return excerpt(&task.body, index, SNIPPET_WINDOW);
    }
    truncate_chars(&task.front.title, SNIPPET_WINDOW)
}

/// `~max` characters of `text` centered on byte `index`, with ellipses where
/// the window was clipped. `index` must be a char boundary (it comes from a
/// `str::find` on the same text).
fn excerpt(text: &str, index: usize, max: usize) -> String {
    let chars: Vec<(usize, char)> = text.char_indices().collect();
    let total = chars.len();
    if total <= max {
        return text.to_string();
    }
    // Find the char position of the match.
    let match_pos = chars.partition_point(|(byte, _)| *byte < index);
    let start = match_pos.saturating_sub(max / 3);
    let end = (start + max).min(total);
    let mut out = String::new();
    if start > 0 {
        out.push('…');
    }
    out.extend(chars[start..end].iter().map(|(_, c)| c));
    if end < total {
        out.push('…');
    }
    out
}

fn truncate_chars(text: &str, max: usize) -> String {
    let mut out: String = text.chars().take(max).collect();
    if text.chars().count() > max {
        out.push('…');
    }
    out
}

/// Parses the `--filter` JSON document. The value must be an object.
pub fn parse_filter(input: &str) -> Result<Value> {
    let value: Value = serde_json::from_str(input).map_err(|error| {
        OrbitError::invalid_argument(format!("--filter is not valid JSON: {error}"))
    })?;
    if !value.is_object() {
        return Err(OrbitError::invalid_argument(
            "--filter must be a JSON object, for example '{\"status\": \"ready\"}'",
        ));
    }
    Ok(value)
}

/// Whether `summary` satisfies every entry of `filter` (AND across keys).
/// Scalar actual values match by equality or by membership in an expected
/// array; array actuals match a scalar by containment and an expected array
/// by containing every element.
pub fn matches_filter(summary: &Value, filter: &Value) -> bool {
    let Some(expected) = filter.as_object() else {
        return false;
    };
    expected.iter().all(|(key, want)| {
        let Some(actual) = summary.get(key) else {
            return false;
        };
        match (actual, want) {
            (Value::Array(items), Value::Array(wants)) => wants
                .iter()
                .all(|want| items.iter().any(|item| item == want)),
            (Value::Array(items), want) => items.iter().any(|item| item == want),
            (actual, Value::Array(wants)) => wants.iter().any(|want| want == actual),
            (actual, want) => actual == want,
        }
    })
}

/// The top-level field names a `--filter` object may reference: the keys of
/// one Issue summary. Used to reject typos instead of silently matching
/// nothing.
pub fn filter_field_names(summary: &Value) -> Vec<String> {
    summary
        .as_object()
        .map(|object| object.keys().cloned().collect())
        .unwrap_or_default()
}

/// Validates every `--filter` key against the Issue summary schema.
pub fn validate_filter_fields(filter: &Value, known: &[String]) -> Result<()> {
    let Some(entries) = filter.as_object() else {
        return Ok(());
    };
    for key in entries.keys() {
        if !known.iter().any(|name| name == key) {
            let mut names = known.to_vec();
            names.sort();
            return Err(OrbitError::invalid_argument(format!(
                "unknown --filter field `{key}`; valid fields: {}",
                names.join(", ")
            )));
        }
    }
    Ok(())
}

/// Parses `--fields id,title,status` into dotted paths.
pub fn parse_fields(input: &str) -> Result<Vec<Vec<String>>> {
    let mut fields = Vec::new();
    for part in input.split(',') {
        let path: Vec<String> = part
            .trim()
            .split('.')
            .filter(|segment| !segment.is_empty())
            .map(str::to_string)
            .collect();
        if path.is_empty() || path.iter().any(|segment| segment.trim() != segment) {
            return Err(OrbitError::invalid_argument(format!(
                "invalid --fields entry `{}`; use comma-separated field names like `id,title,status`",
                part.trim()
            )));
        }
        fields.push(path);
    }
    if fields.is_empty() {
        return Err(OrbitError::invalid_argument("--fields must not be empty"));
    }
    Ok(fields)
}

/// Reduces every object in `value` (a single object or an array of objects)
/// to the requested dotted paths. A path that resolves nowhere is simply
/// absent; callers validate top-level names against the schema first.
pub fn project_fields(value: &mut Value, fields: &[Vec<String>]) {
    match value {
        Value::Object(_) => project_object(value, fields),
        Value::Array(items) => {
            for item in items {
                if item.is_object() {
                    project_object(item, fields);
                }
            }
        }
        _ => {}
    }
}

fn project_object(object: &mut Value, fields: &[Vec<String>]) {
    let Some(map) = object.as_object() else {
        return;
    };
    // Group requested paths by their first segment.
    let mut groups: BTreeMap<String, Vec<Vec<String>>> = BTreeMap::new();
    for path in fields {
        let mut path = path.clone();
        let head = path.remove(0);
        groups.entry(head).or_default().push(path);
    }
    let original = map.clone();
    let mut projected = Map::new();
    for (head, subpaths) in groups {
        if let Some(child) = original.get(&head) {
            if subpaths.iter().all(|path| path.is_empty()) {
                projected.insert(head, child.clone());
            } else {
                let mut child = child.clone();
                project_fields(&mut child, &subpaths);
                projected.insert(head, child);
            }
        }
    }
    *object = Value::Object(projected);
}

/// The top-level names `fields` requests, for schema validation.
pub fn field_root_names(fields: &[Vec<String>]) -> Vec<String> {
    fields
        .iter()
        .filter_map(|path| path.first().cloned())
        .collect()
}

/// Validates that every requested top-level field exists on at least one of
/// the objects being projected.
pub fn validate_field_names(fields: &[Vec<String>], objects: &[&Value]) -> Result<()> {
    let roots = field_root_names(fields);
    for root in roots {
        if !objects.iter().any(|object| object.get(&root).is_some()) {
            return Err(OrbitError::invalid_argument(format!(
                "unknown --fields entry `{root}`; it is not a field of the output object"
            )));
        }
    }
    Ok(())
}

/// Hex codec for the opaque `next_cursor` token: JSON payload → hex string.
pub fn encode_cursor(payload: &Value) -> String {
    let bytes = serde_json::to_vec(payload).unwrap_or_default();
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut out = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        out.push(HEX[(byte >> 4) as usize] as char);
        out.push(HEX[(byte & 0x0f) as usize] as char);
    }
    out
}

/// Inverse of [`encode_cursor`]; rejects anything that is not a hex-encoded
/// JSON value so a stale or foreign cursor fails loudly.
pub fn decode_cursor(token: &str) -> Result<Value> {
    let invalid = || {
        OrbitError::invalid_argument(
            "invalid cursor; pass the `next_cursor` value a previous page returned",
        )
    };
    if token.is_empty()
        || !token.len().is_multiple_of(2)
        || !token.bytes().all(|b| b.is_ascii_hexdigit())
    {
        return Err(invalid());
    }
    let bytes: Vec<u8> = (0..token.len())
        .step_by(2)
        .map(|index| u8::from_str_radix(&token[index..index + 2], 16).unwrap_or(0))
        .collect();
    serde_json::from_slice(&bytes).map_err(|_| invalid())
}

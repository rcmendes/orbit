use crate::error::{OrbitError, code};
use serde_json::{Map, Value, json};

pub struct CommandOutput {
    pub data: Value,
    pub human: String,
    pub meta: Option<Value>,
    pub warnings: Vec<String>,
    pub exit_code: i32,
    pub raw_output: Option<String>,
    pub silent: bool,
}

impl CommandOutput {
    pub fn new(data: Value, human: impl Into<String>) -> Self {
        Self {
            data,
            human: human.into(),
            meta: None,
            warnings: Vec::new(),
            exit_code: 0,
            raw_output: None,
            silent: false,
        }
    }

    pub fn with_meta(mut self, meta: Value) -> Self {
        self.meta = Some(meta);
        self
    }

    pub fn with_warning(mut self, warning: impl Into<String>) -> Self {
        self.warnings.push(warning.into());
        self
    }

    pub fn with_exit_code(mut self, exit_code: i32) -> Self {
        self.exit_code = exit_code;
        self
    }

    pub fn with_silent(mut self) -> Self {
        self.silent = true;
        self
    }
}

pub fn print_success(output: CommandOutput, json_mode: bool) {
    if output.silent {
        return;
    }

    if let Some(raw) = &output.raw_output {
        println!("{raw}");
        for warning in &output.warnings {
            eprintln!("warning: {warning}");
        }
        return;
    }

    if json_mode {
        for warning in &output.warnings {
            eprintln!("warning: {warning}");
        }
        let mut body = Map::new();
        body.insert("ok".to_string(), Value::Bool(true));
        body.insert("data".to_string(), output.data);

        let mut meta = match output.meta {
            Some(Value::Object(map)) => map,
            Some(value) => {
                let mut map = Map::new();
                map.insert("value".to_string(), value);
                map
            }
            None => Map::new(),
        };
        if !output.warnings.is_empty() {
            meta.insert("warnings".to_string(), json!(output.warnings));
        }
        if !meta.is_empty() {
            body.insert("meta".to_string(), Value::Object(meta));
        }

        println!("{}", Value::Object(body));
    } else {
        print!("{}", output.human);
        if !output.human.ends_with('\n') {
            println!();
        }
        for warning in output.warnings {
            eprintln!("warning: {warning}");
        }
    }
}

/// The RFC 9457 `type` member: a stable, namespaced problem identifier
/// derived from the error code (`DEPENDENCY_BLOCKED` →
/// `orbit://problems/dependency-blocked`).
pub(crate) fn problem_type(error_code: &str) -> String {
    format!(
        "orbit://problems/{}",
        error_code.to_lowercase().replace('_', "-")
    )
}

/// The RFC 9457 `title` member: a short human-readable summary derived from
/// the error code (`BOARD_SYNC_CONFLICT` → `Board Sync Conflict`).
pub(crate) fn problem_title(error_code: &str) -> String {
    error_code
        .split('_')
        .filter(|word| !word.is_empty())
        .map(|word| {
            let lower = word.to_lowercase();
            let mut chars = lower.chars();
            match chars.next() {
                Some(first) => first.to_uppercase().collect::<String>() + chars.as_str(),
                None => String::new(),
            }
        })
        .collect::<Vec<_>>()
        .join(" ")
}

/// The RFC 9457-style `hint` member: what the caller can do next. Errors with
/// a dedicated remediation use it; every other code gets a category default.
pub(crate) fn default_hint(error_code: &str) -> String {
    match error_code {
        code::LOCKED => {
            "Retry the command; another Orbit process holds the Board lock.".to_string()
        }
        code::BOARD_NOT_FOUND => {
            "Run `orbit init` to create a Board, or pass --dir/ORBIT_DIR.".to_string()
        }
        code::NOT_FOUND | code::LINK_NOT_FOUND => {
            "Check the ID or project key; `orbit list` shows valid Issues.".to_string()
        }
        code::CONTRACT_VERSION_UNSUPPORTED => {
            "Upgrade orbit to a version that supports the Board's contract.".to_string()
        }
        code::PROFILE_UNMET => {
            "Upgrade Orbit, or apply a profile whose requirements this binary meets (`orbit setup <plugin>`).".to_string()
        }
        code::VERSION_CONFLICT => {
            "Re-read the Task (`orbit show <id>`) and retry with its current `updated_at` as --if-version.".to_string()
        }
        code::MODEL_NOT_MIGRATED => {
            "Run `orbit migrate model` to preview, then `orbit migrate model --apply`.".to_string()
        }
        code::IDEMPOTENCY_KEY_REUSED => {
            "Use a fresh --idempotency-key for a different request, or repeat the original request unchanged.".to_string()
        }
        code::CYCLE_DETECTED => {
            "Remove or reverse one of the blocking links in the cycle (`orbit link list <id>`).".to_string()
        }
        code::EPHEMERAL_BIN => {
            "Install orbit to a stable path (not a temp or build directory) and run it from there.".to_string()
        }
        code::DEPENDENCY_BLOCKED | code::SUBTASKS_OPEN | code::LABEL_BLOCKED => {
            "Resolve the blocking Issues listed in details first.".to_string()
        }
        code::ALREADY_CLAIMED | code::CLAIM_STALE | code::NOT_CLAIM_HOLDER => {
            "Check `orbit show <id>` for the current assignee and lease.".to_string()
        }
        code::INVALID_TRANSITION => {
            "Run `orbit show <id>` for the Issue's allowed_transitions.".to_string()
        }
        code::POLICY_VIOLATION => {
            "Fix the fields, properties, or labels listed in details.violations, or adjust the `[policy]` rule in config.toml.".to_string()
        }
        code::CONFIG_ERROR | code::BOARD_CORRUPT | code::CORRUPT_TASK => {
            "Fix the Board files or config.toml; `orbit doctor` reports problems.".to_string()
        }
        code::GIT_ERROR | code::IO_ERROR | code::ID_COLLISION => {
            "Check the repository and filesystem state, then retry.".to_string()
        }
        _ if error_code.starts_with("GITHUB_") || error_code.starts_with("REMOTE_") => {
            "Run `orbit remote status` to inspect Remote Tracker health.".to_string()
        }
        _ if error_code.starts_with("INVALID_")
            || error_code.starts_with("DUPLICATE_")
            || error_code.starts_with("MISSING_") =>
        {
            "Fix the reported value and retry; `orbit <command> --help` lists valid inputs."
                .to_string()
        }
        _ => "Retry; if it persists run `orbit doctor`.".to_string(),
    }
}

pub fn print_error(error: &OrbitError, json_mode: bool) {
    let (message, detail, remediation, details) = actionable_error(error);
    let hint = remediation.unwrap_or_else(|| default_hint(error.code));
    if json_mode {
        let mut body = Map::new();
        body.insert("type".to_string(), json!(problem_type(error.code)));
        body.insert("title".to_string(), json!(problem_title(error.code)));
        body.insert("code".to_string(), json!(error.code));
        body.insert("message".to_string(), json!(message));
        body.insert("detail".to_string(), json!(detail));
        body.insert("retryable".to_string(), json!(error.retryable));
        body.insert("hint".to_string(), json!(hint));
        if let Some(details) = details {
            body.insert("details".to_string(), details);
        }
        let payload = json!({ "ok": false, "error": Value::Object(body) });
        println!("{payload}");
    } else {
        eprintln!("error [{}]: {}", error.code, message);
        eprintln!("hint: {hint}");
        if let Some(details) = details
            && let Ok(pretty) = serde_json::to_string_pretty(&details)
        {
            eprintln!("{pretty}");
        }
    }
}

/// Returns `(message, detail, remediation, details)`: `message` is the
/// legacy envelope text (cause plus remediation suffix), `detail` is the
/// RFC 9457 detail (the cause alone), `remediation` is the per-code hint
/// when one exists, and `details` is the structured payload.
fn actionable_error(error: &OrbitError) -> (String, String, Option<String>, Option<Value>) {
    let cause = if error.code.starts_with("GITHUB_") {
        redact_github_secrets(&error.message)
    } else {
        error.message.clone()
    };
    let mut details = match error.details.clone() {
        Some(Value::Object(details)) => details,
        Some(value) => Map::from_iter([("value".to_string(), value)]),
        None => Map::new(),
    };
    let remediation = match error.code {
        crate::error::code::GITHUB_NOT_CONFIGURED => Some(
            "Run `orbit remote connect` from the repository checkout with the intended GitHub origin. If `gh` is not authenticated, run `gh auth login` first.".to_string(),
        ),
        crate::error::code::GITHUB_SETUP_INCOMPLETE => Some(
            "Run `orbit remote connect --fields` to resolve and store the configured Issue Field ids. If the named field is missing, create it in the organization at Settings > Projects > Issue fields, or run `orbit remote connect --fields --create-fields` as an organization administrator; if the organization is unset, run `orbit remote connect`.".to_string(),
        ),
        crate::error::code::GITHUB_REPO_NOT_LINKED => {
            let repository = details
                .get("repository")
                .and_then(Value::as_str)
                .unwrap_or("OWNER/REPO");
            Some(format!(
                "Link this repository to the Board with `orbit remote connect {repository}`, then retry."
            ))
        }
        crate::error::code::REMOTE_PROJECT_NOT_LINKED => {
            let tracker = details
                .get("tracker")
                .and_then(Value::as_str)
                .unwrap_or("<tracker>")
                .to_string();
            let project = details
                .get("project")
                .and_then(Value::as_str)
                .unwrap_or("<project>")
                .to_string();
            Some(format!(
                "Remote Project `{project}` on tracker `{tracker}` is not linked to this Board. Link it with `orbit remote connect {tracker}:{project}`, then retry."
            ))
        }
        crate::error::code::REMOTE_ISSUE_CONFLICT => {
            let task = details
                .get("issue")
                .and_then(Value::as_str)
                .unwrap_or("<task-id>")
                .to_string();
            let tracker = details
                .get("tracker")
                .and_then(Value::as_str)
                .unwrap_or("<tracker>")
                .to_string();
            let project = details
                .get("project")
                .and_then(Value::as_str)
                .unwrap_or("<project>")
                .to_string();
            let id = details
                .get("id")
                .and_then(Value::as_str)
                .unwrap_or("<issue-id>")
                .to_string();
            let fields = details
                .get("conflicts")
                .and_then(Value::as_array)
                .map(|conflicts| {
                    conflicts
                        .iter()
                        .filter_map(|conflict| conflict.get("field").and_then(Value::as_str))
                        .collect::<Vec<_>>()
                        .join(", ")
                })
                .filter(|fields| !fields.is_empty())
                .unwrap_or_else(|| "one or more fields".to_string());
            details.insert("conflicting_fields".to_string(), json!(fields));
            let sync_command = format!("orbit remote sync {task}");
            Some(format!(
                "Task {task} conflicts with {tracker} Issue {project}#{id} in {fields}. Review with `{sync_command}`, choose each side with `--resolve FIELD=orbit` or `--resolve FIELD=remote`, then apply with `{sync_command} --resolve FIELD=orbit|remote --apply`."
            ))
        }
        crate::error::code::PROFILE_CONFLICT => {
            let conflicting = details
                .get("conflicting_profile")
                .and_then(Value::as_str)
                .unwrap_or("<profile>");
            let profile = details
                .get("profile")
                .and_then(Value::as_str)
                .unwrap_or("<profile>");
            Some(format!(
                "Remove profile `{conflicting}` and its applied `[workflow]` section from the Board's config.toml, then retry `orbit setup {profile}` — or keep `{conflicting}` and skip `{profile}`."
            ))
        }
        crate::error::code::TRIAGE_APPROVAL_REQUIRED => {
            let task = details
                .get("issue")
                .and_then(Value::as_str)
                .unwrap_or("<task-id>");
            let proposal = details
                .get("proposal_file")
                .and_then(Value::as_str)
                .unwrap_or("<proposal-file>");
            Some(format!(
                "Review the proposal, then rerun `orbit triage apply {task} --proposal-file {proposal} --approve`."
            ))
        }
        crate::error::code::TRIAGE_INVALID_PROPOSAL => Some(
            "Edit the proposal JSON to fix the reported cause. Use only `area`, `clear_area`, `verification`, `clear_verification`, `labels_add`, and `labels_remove`; labels must be canonical. Review it with `orbit triage review <task-id> --proposal-file <proposal-file>` before applying.".to_string(),
        ),
        _ => None,
    };

    let Some(remediation) = remediation else {
        return (error.message.clone(), cause, None, error.details.clone());
    };

    details.insert("cause".to_string(), json!(cause));
    details.insert("remediation".to_string(), json!(remediation));
    let details = if details.is_empty() {
        None
    } else {
        let mut details = Value::Object(details);
        if error.code.starts_with("GITHUB_") {
            redact_json_strings(&mut details);
        }
        Some(details)
    };
    (
        format!("{cause} Next: {remediation}"),
        cause,
        Some(remediation),
        details,
    )
}

fn redact_github_secrets(message: &str) -> String {
    const MARKERS: &[&str] = &["ghp_", "gho_", "ghu_", "ghs_", "ghr_", "github_pat_"];
    let mut result = String::with_capacity(message.len());
    let mut rest = message;
    while !rest.is_empty() {
        let next = MARKERS
            .iter()
            .filter_map(|marker| rest.find(marker).map(|index| (index, *marker)))
            .min_by_key(|(index, _)| *index);
        let Some((index, marker)) = next else {
            result.push_str(rest);
            break;
        };
        result.push_str(&rest[..index]);
        result.push_str("[REDACTED_GITHUB_TOKEN]");
        let after_marker = &rest[index + marker.len()..];
        let token_end = after_marker
            .find(|character: char| {
                character.is_whitespace() || matches!(character, '\'' | '"' | ',' | ';' | ')' | ']')
            })
            .unwrap_or(after_marker.len());
        rest = &after_marker[token_end..];
    }
    result
}

fn redact_json_strings(value: &mut Value) {
    match value {
        Value::String(value) => *value = redact_github_secrets(value),
        Value::Array(values) => values.iter_mut().for_each(redact_json_strings),
        Value::Object(values) => values.values_mut().for_each(redact_json_strings),
        _ => {}
    }
}

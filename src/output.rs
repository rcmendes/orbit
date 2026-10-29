use crate::error::OrbitError;
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

pub fn print_error(error: &OrbitError, json_mode: bool) {
    let (message, details) = actionable_error(error);
    if json_mode {
        let mut body = Map::new();
        body.insert("code".to_string(), json!(error.code));
        body.insert("message".to_string(), json!(message));
        body.insert("retryable".to_string(), json!(error.retryable));
        if let Some(details) = details {
            body.insert("details".to_string(), details);
        }
        let payload = json!({ "ok": false, "error": Value::Object(body) });
        println!("{payload}");
    } else {
        eprintln!("error [{}]: {}", error.code, message);
        if let Some(details) = details
            && let Ok(pretty) = serde_json::to_string_pretty(&details)
        {
            eprintln!("{pretty}");
        }
    }
}

fn actionable_error(error: &OrbitError) -> (String, Option<Value>) {
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
            "If GitHub is not configured, run `orbit github connect` from the repository checkout with the intended GitHub origin. If `gh` is not authenticated, run `gh auth login` first.".to_string(),
        ),
        crate::error::code::GITHUB_SETUP_INCOMPLETE => Some(
            "Run `orbit github setup` to resolve and store the configured Issue Field ids. If the named field is missing, create it in the organization at Settings > Projects > Issue fields, or run `orbit github setup --create-fields` as an organization administrator; if the organization is unset, run `orbit github connect`.".to_string(),
        ),
        crate::error::code::GITHUB_REPO_NOT_LINKED => {
            let repository = details
                .get("repository")
                .and_then(Value::as_str)
                .unwrap_or("OWNER/REPO");
            Some(format!(
                "Link this repository to the Board with `orbit github repo link {repository}`, then retry."
            ))
        }
        crate::error::code::GITHUB_ISSUE_CONFLICT => {
            let task = details
                .get("task")
                .and_then(Value::as_str)
                .unwrap_or("<task-id>")
                .to_string();
            let repo = details
                .get("repository")
                .and_then(Value::as_str)
                .unwrap_or("OWNER/REPO")
                .to_string();
            let number = details
                .get("number")
                .and_then(Value::as_u64)
                .map(|number| number.to_string())
                .unwrap_or_else(|| "<issue-number>".to_string());
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
            Some(format!(
                "Task {task} conflicts with GitHub Issue {repo}#{number} in {fields}. Review with `orbit github sync {task}`, choose each side with `--resolve FIELD=orbit` or `--resolve FIELD=github`, then apply with `orbit github sync {task} --resolve FIELD=orbit|github --apply`."
            ))
        }
        crate::error::code::TRIAGE_NOT_LINKED => {
            let task = details
                .get("task")
                .and_then(Value::as_str)
                .unwrap_or("<task-id>");
            Some(format!(
                "Adopt the intended GitHub Issue for Task {task} with `orbit github adopt {task} <issue-url>`, then retry triage."
            ))
        }
        crate::error::code::TRIAGE_APPROVAL_REQUIRED => {
            let task = details
                .get("task")
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
        return (error.message.clone(), error.details.clone());
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
    (format!("{cause} Next: {remediation}"), details)
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

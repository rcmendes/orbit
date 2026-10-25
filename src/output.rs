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
    if json_mode {
        let mut body = Map::new();
        body.insert("code".to_string(), json!(error.code));
        body.insert("message".to_string(), json!(error.message));
        body.insert("retryable".to_string(), json!(error.retryable));
        if let Some(details) = &error.details {
            body.insert("details".to_string(), details.clone());
        }
        let payload = json!({ "ok": false, "error": Value::Object(body) });
        println!("{payload}");
    } else {
        eprintln!("error [{}]: {}", error.code, error.message);
        if let Some(details) = &error.details
            && let Ok(pretty) = serde_json::to_string_pretty(details)
        {
            eprintln!("{pretty}");
        }
    }
}

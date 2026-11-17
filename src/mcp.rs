use std::io::{BufRead, Write};
use std::path::Path;
use std::process::{Command, Stdio};

use serde_json::{Value, json};

use crate::error::{OrbitError, Result};

const PROTOCOL_VERSION: &str = "2025-06-18";
const INSTRUCTIONS: &str = "Orbit is the task tracker for this repository. Use `ready` with `claim:true` and a stable `by` to take work, `heartbeat` with the same `by` while working, and `finish` with `by` when done. Use `checkpoint` with the same `by` to record progress, the next action, and blockers for a handoff. `create`, `update`, `link`, and `delete` shape the board. Never edit task files directly.";

pub fn serve(dir: Option<&Path>) -> Result<()> {
    let executable = std::env::current_exe()
        .map_err(|error| OrbitError::io("resolve the orbit executable", error))?;
    let dir = dir.map(Path::to_path_buf);

    let stdin = std::io::stdin();
    let mut stdout = std::io::stdout();

    for line in stdin.lock().lines() {
        let line = line.map_err(|error| OrbitError::io("read MCP stdin", error))?;
        if line.trim().is_empty() {
            continue;
        }

        let message: Value = match serde_json::from_str(&line) {
            Ok(message) => message,
            Err(error) => {
                let response = rpc_error(Value::Null, -32700, format!("parse error: {error}"));
                write_message(&mut stdout, &response)?;
                continue;
            }
        };

        let response = if let Some(batch) = message.as_array() {
            let mut responses = Vec::new();
            for item in batch {
                if let Some(response) = handle_message(item, &executable, dir.as_deref()) {
                    responses.push(response);
                }
            }
            if responses.is_empty() {
                None
            } else {
                Some(Value::Array(responses))
            }
        } else {
            handle_message(&message, &executable, dir.as_deref())
        };

        if let Some(response) = response {
            write_message(&mut stdout, &response)?;
        }
    }

    Ok(())
}

fn write_message(stdout: &mut std::io::Stdout, message: &Value) -> Result<()> {
    let encoded = serde_json::to_string(message)
        .map_err(|error| OrbitError::config(format!("serialize MCP response: {error}")))?;
    writeln!(stdout, "{encoded}").map_err(|error| OrbitError::io("write MCP stdout", error))?;
    stdout
        .flush()
        .map_err(|error| OrbitError::io("flush MCP stdout", error))
}

fn handle_message(message: &Value, executable: &Path, dir: Option<&Path>) -> Option<Value> {
    let id = message.get("id").cloned();
    let Some(method) = message.get("method").and_then(Value::as_str) else {
        return id.map(|id| rpc_error(id, -32600, "invalid request"));
    };

    let id = id?;

    let params = message.get("params").cloned().unwrap_or_else(|| json!({}));
    let response = match method {
        "initialize" => Ok(initialize_result(&params)),
        "ping" => Ok(json!({})),
        "tools/list" => Ok(json!({ "tools": tool_definitions() })),
        "tools/call" => tool_call(&params, executable, dir),
        "resources/list" => Ok(resources_list(executable, dir)),
        "resources/templates/list" => Ok(json!({
            "resourceTemplates": [resource_template()]
        })),
        "resources/read" => resources_read(&params, executable, dir),
        "shutdown" => Ok(Value::Null),
        _ => Err((-32601, format!("method not found: {method}"))),
    };

    Some(match response {
        Ok(result) => rpc_result(id, result),
        Err((code, message)) => rpc_error(id, code, message),
    })
}

fn rpc_result(id: Value, result: Value) -> Value {
    json!({ "jsonrpc": "2.0", "id": id, "result": result })
}

fn rpc_error(id: Value, code: i64, message: impl Into<String>) -> Value {
    json!({
        "jsonrpc": "2.0",
        "id": id,
        "error": { "code": code, "message": message.into() }
    })
}

fn initialize_result(params: &Value) -> Value {
    let requested = params
        .get("protocolVersion")
        .and_then(Value::as_str)
        .unwrap_or(PROTOCOL_VERSION);
    let protocol_version = if requested.is_empty() {
        PROTOCOL_VERSION
    } else {
        requested
    };

    json!({
        "protocolVersion": protocol_version,
        "capabilities": {
            "tools": { "listChanged": false },
            "resources": { "subscribe": false, "listChanged": false }
        },
        "serverInfo": {
            "name": "orbit",
            "title": "Orbit",
            "version": env!("CARGO_PKG_VERSION")
        },
        "instructions": INSTRUCTIONS
    })
}

fn tool_definitions() -> Vec<Value> {
    crate::registry::tool_definitions()
}

fn resource_template() -> Value {
    json!({
        "uriTemplate": "orbit://tasks/{id}",
        "name": "orbit-task",
        "title": "Orbit task",
        "description": "A task's Markdown body, acceptance criteria, claim state, and checkpoint.",
        "mimeType": "text/markdown"
    })
}

fn tool_call(
    params: &Value,
    executable: &Path,
    dir: Option<&Path>,
) -> std::result::Result<Value, (i64, String)> {
    let name = params
        .get("name")
        .and_then(Value::as_str)
        .ok_or((-32602, "missing tool name".to_string()))?;
    let arguments = params
        .get("arguments")
        .cloned()
        .unwrap_or_else(|| json!({}));
    let tool = dispatch_tool(name, &arguments, executable, dir);
    let text = serde_json::to_string(&tool.envelope)
        .map_err(|error| (-32603, format!("serialize tool result: {error}")))?;
    let mut result = json!({
        "content": [{ "type": "text", "text": text }],
        "structuredContent": tool.envelope
    });
    if tool.is_error {
        result["isError"] = json!(true);
    }
    Ok(result)
}

struct ToolResponse {
    envelope: Value,
    is_error: bool,
}

fn dispatch_tool(
    name: &str,
    arguments: &Value,
    executable: &Path,
    dir: Option<&Path>,
) -> ToolResponse {
    if name == "ready" {
        let claim = bool_arg(arguments, "claim");
        if arguments.get("by").is_some() && !claim {
            return tool_invalid("`by` requires `claim: true`");
        }
        if optional_u64(arguments, "limit") == Some(0) {
            return tool_invalid("`limit` must be at least 1");
        }
    }
    if let Some(status) = optional_str(arguments, "status")
        && name == "finish"
        && !matches!(status.as_str(), "done" | "cancelled")
    {
        return tool_invalid("`status` must be `done` or `cancelled`");
    }
    if name == "update"
        && arguments.get("waiting_on").is_some()
        && bool_arg(arguments, "clear_waiting")
    {
        return tool_invalid("`waiting_on` and `clear_waiting` are mutually exclusive");
    }
    if name == "checkpoint" {
        if arguments.get("next").is_some() && bool_arg(arguments, "clear_next") {
            return tool_invalid("`next` and `clear_next` are mutually exclusive");
        }
        if arguments.get("blocker").is_some() && bool_arg(arguments, "clear_blocker") {
            return tool_invalid("`blocker` and `clear_blocker` are mutually exclusive");
        }
    }
    if name == "update"
        && arguments.get("component").is_some()
        && bool_arg(arguments, "clear_component")
    {
        return tool_invalid("`component` and `clear_component` are mutually exclusive");
    }
    if name == "update"
        && arguments.get("repository").is_some()
        && bool_arg(arguments, "clear_repository")
    {
        return tool_invalid("`repository` and `clear_repository` are mutually exclusive");
    }
    if name == "update"
        && arguments.get("external_id").is_some()
        && bool_arg(arguments, "clear_external_id")
    {
        return tool_invalid("`external_id` and `clear_external_id` are mutually exclusive");
    }
    if name == "update"
        && arguments.get("verification").is_some()
        && bool_arg(arguments, "clear_verification")
    {
        return tool_invalid("`verification` and `clear_verification` are mutually exclusive");
    }
    if name == "create"
        && let Some(depends_on) = arguments.get("depends_on")
        && !depends_on.is_array()
    {
        return tool_invalid("`depends_on` must be an array of task ids");
    }
    if name == "create"
        && let Some(labels) = arguments.get("labels")
        && !labels.is_array()
    {
        return tool_invalid("`labels` must be an array of strings");
    }
    if name == "list" && arguments.get("label").is_some() && bool_arg(arguments, "unlabeled") {
        return tool_invalid("`label` and `unlabeled` are mutually exclusive");
    }
    if name == "list" && bool_arg(arguments, "linked") && bool_arg(arguments, "local") {
        return tool_invalid("`linked` and `local` are mutually exclusive");
    }
    if arguments
        .get("parent")
        .is_some_and(|value| !value.is_string())
    {
        return tool_invalid("`parent` must be a task id string");
    }
    if arguments
        .get("repository")
        .is_some_and(|value| !value.is_string())
    {
        return tool_invalid("`repository` must be a string");
    }
    if arguments
        .get("labels")
        .is_some_and(|value| !value.is_array())
    {
        return tool_invalid("`labels` must be an array of strings");
    }
    if arguments
        .get("exclude_labels")
        .is_some_and(|value| !value.is_array())
    {
        return tool_invalid("`exclude_labels` must be an array of strings");
    }
    for key in ["types", "exclude_types", "components", "exclude_components"] {
        if arguments.get(key).is_some_and(|value| !value.is_array()) {
            return tool_invalid(format!("`{key}` must be an array of strings"));
        }
    }

    let mut args: Vec<String> = Vec::new();
    // A Batch travels as a temporary file because tool calls spawn the CLI without stdin.
    let mut batch_file: Option<std::path::PathBuf> = None;

    match name {
        "ready" => {
            args.push("ready".to_string());
            if bool_arg(arguments, "claim") {
                args.push("--claim".to_string());
            }
            if let Some(by) = optional_str(arguments, "by") {
                args.push("--by".to_string());
                args.push(by);
            }
            if let Some(strategy) = optional_str(arguments, "strategy") {
                args.push("--strategy".to_string());
                args.push(strategy);
            }
            if let Some(limit) = optional_u64(arguments, "limit") {
                args.push("--limit".to_string());
                args.push(limit.to_string());
            }
            if let Some(parent) = optional_str(arguments, "parent") {
                args.push("--parent".to_string());
                args.push(parent);
            }
            if let Some(repository) = optional_str(arguments, "repository") {
                args.push("--repository".to_string());
                args.push(repository);
            }
            if let Some(labels) = arguments.get("labels").and_then(Value::as_array) {
                for label in labels {
                    let Some(label) = label.as_str() else {
                        return tool_invalid("`labels` must be an array of strings");
                    };
                    args.push("--label".to_string());
                    args.push(label.to_string());
                }
            }
            if let Some(labels) = arguments.get("exclude_labels").and_then(Value::as_array) {
                for label in labels {
                    let Some(label) = label.as_str() else {
                        return tool_invalid("`exclude_labels` must be an array of strings");
                    };
                    args.push("--exclude-label".to_string());
                    args.push(label.to_string());
                }
            }
            if let Some(types) = arguments.get("types").and_then(Value::as_array) {
                for task_type in types {
                    let Some(task_type) = task_type.as_str() else {
                        return tool_invalid("`types` must be an array of strings");
                    };
                    args.push("--type".to_string());
                    args.push(task_type.to_string());
                }
            }
            if let Some(types) = arguments.get("exclude_types").and_then(Value::as_array) {
                for task_type in types {
                    let Some(task_type) = task_type.as_str() else {
                        return tool_invalid("`exclude_types` must be an array of strings");
                    };
                    args.push("--exclude-type".to_string());
                    args.push(task_type.to_string());
                }
            }
            if let Some(components) = arguments.get("components").and_then(Value::as_array) {
                for component in components {
                    let Some(component) = component.as_str() else {
                        return tool_invalid("`components` must be an array of strings");
                    };
                    args.push("--component".to_string());
                    args.push(component.to_string());
                }
            }
            if let Some(components) = arguments
                .get("exclude_components")
                .and_then(Value::as_array)
            {
                for component in components {
                    let Some(component) = component.as_str() else {
                        return tool_invalid("`exclude_components` must be an array of strings");
                    };
                    args.push("--exclude-component".to_string());
                    args.push(component.to_string());
                }
            }
            if bool_arg(arguments, "all") {
                args.push("--all".to_string());
            }
        }
        "list" => {
            args.push("list".to_string());
            if let Some(status) = optional_str(arguments, "status") {
                args.push("--status".to_string());
                args.push(status);
            }
            if let Some(priority) = optional_str(arguments, "priority") {
                args.push("--priority".to_string());
                args.push(priority);
            }
            let assignee = optional_str(arguments, "assignee")
                .or_else(|| optional_str(arguments, "claimed_by"));
            if let Some(assignee) = assignee {
                args.push("--assignee".to_string());
                args.push(assignee);
            }
            if let Some(label) = optional_str(arguments, "label") {
                args.push("--label".to_string());
                args.push(label);
            }
            if bool_arg(arguments, "unlabeled") {
                args.push("--unlabeled".to_string());
            }
            if let Some(parent) = optional_str(arguments, "parent") {
                args.push("--parent".to_string());
                args.push(parent);
            }
            if let Some(repository) = optional_str(arguments, "repository") {
                args.push("--repository".to_string());
                args.push(repository);
            }
            if let Some(labels) = arguments.get("labels").and_then(Value::as_array) {
                for label in labels {
                    let Some(label) = label.as_str() else {
                        return tool_invalid("`labels` must be an array of strings");
                    };
                    args.push("--label".to_string());
                    args.push(label.to_string());
                }
            }
            if let Some(labels) = arguments.get("exclude_labels").and_then(Value::as_array) {
                for label in labels {
                    let Some(label) = label.as_str() else {
                        return tool_invalid("`exclude_labels` must be an array of strings");
                    };
                    args.push("--exclude-label".to_string());
                    args.push(label.to_string());
                }
            }
            if let Some(types) = arguments.get("types").and_then(Value::as_array) {
                for task_type in types {
                    let Some(task_type) = task_type.as_str() else {
                        return tool_invalid("`types` must be an array of strings");
                    };
                    args.push("--type".to_string());
                    args.push(task_type.to_string());
                }
            }
            if let Some(types) = arguments.get("exclude_types").and_then(Value::as_array) {
                for task_type in types {
                    let Some(task_type) = task_type.as_str() else {
                        return tool_invalid("`exclude_types` must be an array of strings");
                    };
                    args.push("--exclude-type".to_string());
                    args.push(task_type.to_string());
                }
            }
            if let Some(components) = arguments.get("components").and_then(Value::as_array) {
                for component in components {
                    let Some(component) = component.as_str() else {
                        return tool_invalid("`components` must be an array of strings");
                    };
                    args.push("--component".to_string());
                    args.push(component.to_string());
                }
            }
            if let Some(components) = arguments
                .get("exclude_components")
                .and_then(Value::as_array)
            {
                for component in components {
                    let Some(component) = component.as_str() else {
                        return tool_invalid("`exclude_components` must be an array of strings");
                    };
                    args.push("--exclude-component".to_string());
                    args.push(component.to_string());
                }
            }
            if bool_arg(arguments, "waiting") {
                args.push("--waiting".to_string());
            }
            if bool_arg(arguments, "linked") {
                args.push("--linked".to_string());
            }
            if bool_arg(arguments, "local") {
                args.push("--local".to_string());
            }
            if bool_arg(arguments, "all") {
                args.push("--all".to_string());
            }
            if bool_arg(arguments, "include_closed") {
                args.push("--include-closed".to_string());
            }
            if let Some(filter) = arguments.get("filter").filter(|v| !v.is_null()) {
                let text = match filter {
                    Value::String(text) => text.clone(),
                    other => serde_json::to_string(other).unwrap_or_default(),
                };
                args.push("--filter".to_string());
                args.push(text);
            }
            if let Some(fields) = optional_str(arguments, "fields") {
                args.push("--fields".to_string());
                args.push(fields);
            }
            if let Some(limit) = optional_u64(arguments, "limit") {
                args.push("--limit".to_string());
                args.push(limit.to_string());
            }
            if let Some(cursor) = optional_str(arguments, "cursor") {
                args.push("--cursor".to_string());
                args.push(cursor);
            }
        }
        "get" => {
            let Some(id) = required_str(arguments, "id") else {
                return tool_invalid("missing or invalid `id`");
            };
            args.push("show".to_string());
            args.push(id);
            if let Some(fields) = optional_str(arguments, "fields") {
                args.push("--fields".to_string());
                args.push(fields);
            }
        }
        "create" => {
            let Some(title) = required_str(arguments, "title") else {
                return tool_invalid("missing or invalid `title`");
            };
            args.push("add".to_string());
            args.push(title);
            if let Some(parent) = optional_str(arguments, "parent") {
                args.push("--parent".to_string());
                args.push(parent);
            }
            if let Some(task_type) = optional_str(arguments, "type") {
                args.push("--type".to_string());
                args.push(task_type);
            }
            if let Some(priority) = optional_str(arguments, "priority") {
                args.push("-p".to_string());
                args.push(priority);
            }
            if let Some(labels) = arguments.get("labels").and_then(Value::as_array) {
                for label in labels {
                    let Some(label) = label.as_str() else {
                        return tool_invalid("`labels` must be an array of strings");
                    };
                    args.push("--label".to_string());
                    args.push(label.to_string());
                }
            }
            if let Some(depends_on) = arguments.get("depends_on").and_then(Value::as_array) {
                for dependency in depends_on {
                    let Some(dependency) = dependency.as_str() else {
                        return tool_invalid("`depends_on` must be an array of task ids");
                    };
                    args.push("--depends-on".to_string());
                    args.push(dependency.to_string());
                }
            }
            if let Some(waiting_on) = optional_str(arguments, "waiting_on") {
                args.push("--waiting-on".to_string());
                args.push(waiting_on);
            }
            if let Some(component) = optional_str(arguments, "component") {
                args.push("--component".to_string());
                args.push(component);
            }
            if let Some(external_id) = optional_str(arguments, "external_id") {
                args.push("--external-id".to_string());
                args.push(external_id);
            }
            if let Some(verification) = optional_str(arguments, "verification") {
                args.push("--verification".to_string());
                args.push(verification);
            }
            if let Some(reporter) = optional_str(arguments, "reporter") {
                args.push("--reporter".to_string());
                args.push(reporter);
            }
            if let Some(body) = optional_str(arguments, "body") {
                args.push("--body".to_string());
                args.push(body);
            }
        }
        "update" => {
            let Some(id) = required_str(arguments, "id") else {
                return tool_invalid("missing or invalid `id`");
            };
            args.push("update".to_string());
            args.push(id);
            if let Some(title) = optional_str(arguments, "title") {
                args.push("--title".to_string());
                args.push(title);
            }
            if let Some(body) = optional_str(arguments, "body") {
                args.push("--body".to_string());
                args.push(body);
            }
            if let Some(task_type) = optional_str(arguments, "type") {
                args.push("--type".to_string());
                args.push(task_type);
            }
            if let Some(priority) = optional_str(arguments, "priority") {
                args.push("--priority".to_string());
                args.push(priority);
            }
            if let Some(waiting_on) = optional_str(arguments, "waiting_on") {
                args.push("--waiting-on".to_string());
                args.push(waiting_on);
            }
            if bool_arg(arguments, "clear_waiting") {
                args.push("--clear-waiting".to_string());
            }
            if let Some(component) = optional_str(arguments, "component") {
                args.push("--component".to_string());
                args.push(component);
            }
            if bool_arg(arguments, "clear_component") {
                args.push("--clear-component".to_string());
            }
            if let Some(repository) = optional_str(arguments, "repository") {
                args.push("--repository".to_string());
                args.push(repository);
            }
            if bool_arg(arguments, "clear_repository") {
                args.push("--clear-repository".to_string());
            }
            if let Some(external_id) = optional_str(arguments, "external_id") {
                args.push("--external-id".to_string());
                args.push(external_id);
            }
            if bool_arg(arguments, "clear_external_id") {
                args.push("--clear-external-id".to_string());
            }
            if let Some(verification) = optional_str(arguments, "verification") {
                args.push("--verification".to_string());
                args.push(verification);
            }
            if bool_arg(arguments, "clear_verification") {
                args.push("--clear-verification".to_string());
            }
        }
        "claim" => {
            let Some(id) = required_str(arguments, "id") else {
                return tool_invalid("missing or invalid `id`");
            };
            args.push("claim".to_string());
            args.push(id);
            if let Some(by) = optional_str(arguments, "by") {
                args.push("--by".to_string());
                args.push(by);
            }
            if let Some(kind) = optional_str(arguments, "kind") {
                args.push("--kind".to_string());
                args.push(kind);
            }
            if bool_arg(arguments, "steal") {
                args.push("--steal".to_string());
            }
        }
        "assign" => {
            let Some(id) = required_str(arguments, "id") else {
                return tool_invalid("missing or invalid `id`");
            };
            let Some(to) = required_str(arguments, "to") else {
                return tool_invalid("missing or invalid `to`");
            };
            args.push("assign".to_string());
            args.push(id);
            args.push("--to".to_string());
            args.push(to);
            if let Some(kind) = optional_str(arguments, "kind") {
                args.push("--kind".to_string());
                args.push(kind);
            }
            if bool_arg(arguments, "force") {
                args.push("--force".to_string());
            }
        }
        "release" => {
            let Some(id) = required_str(arguments, "id") else {
                return tool_invalid("missing or invalid `id`");
            };
            args.push("release".to_string());
            args.push(id);
            if let Some(by) = optional_str(arguments, "by") {
                args.push("--by".to_string());
                args.push(by);
            }
            if bool_arg(arguments, "force") {
                args.push("--force".to_string());
            }
        }
        "heartbeat" => {
            let Some(id) = required_str(arguments, "id") else {
                return tool_invalid("missing or invalid `id`");
            };
            args.push("heartbeat".to_string());
            args.push(id);
            if let Some(by) = optional_str(arguments, "by") {
                args.push("--by".to_string());
                args.push(by);
            }
        }
        "checkpoint" => {
            let Some(id) = required_str(arguments, "id") else {
                return tool_invalid("missing or invalid `id`");
            };
            args.push("checkpoint".to_string());
            args.push(id);
            if let Some(progress) = optional_str(arguments, "progress") {
                args.push("--progress".to_string());
                args.push(progress);
            }
            if let Some(next) = optional_str(arguments, "next") {
                args.push("--next".to_string());
                args.push(next);
            }
            if bool_arg(arguments, "clear_next") {
                args.push("--clear-next".to_string());
            }
            if let Some(blocker) = optional_str(arguments, "blocker") {
                args.push("--blocker".to_string());
                args.push(blocker);
            }
            if bool_arg(arguments, "clear_blocker") {
                args.push("--clear-blocker".to_string());
            }
            if let Some(by) = optional_str(arguments, "by") {
                args.push("--by".to_string());
                args.push(by);
            }
        }
        "finish" => {
            let Some(id) = required_str(arguments, "id") else {
                return tool_invalid("missing or invalid `id`");
            };
            let Some(status) = required_str(arguments, "status") else {
                return tool_invalid("missing or invalid `status`");
            };
            let command = match status.as_str() {
                "done" => "done",
                "cancelled" => "cancel",
                _ => return tool_invalid("`status` must be `done` or `cancelled`"),
            };
            args.push(command.to_string());
            args.push(id);
            if let Some(by) = optional_str(arguments, "by") {
                args.push("--by".to_string());
                args.push(by);
            }
            if bool_arg(arguments, "force") {
                args.push("--force".to_string());
            }
            if let Some(comment) = optional_str(arguments, "comment") {
                args.push("--comment".to_string());
                args.push(comment);
            }
        }
        "transition" => {
            let Some(id) = required_str(arguments, "id") else {
                return tool_invalid("missing or invalid `id`");
            };
            let Some(name) = required_str(arguments, "name") else {
                return tool_invalid("missing or invalid `name`");
            };
            args.push("transition".to_string());
            args.push(id);
            args.push(name);
            if let Some(by) = optional_str(arguments, "by") {
                args.push("--by".to_string());
                args.push(by);
            }
            if bool_arg(arguments, "force") {
                args.push("--force".to_string());
            }
        }
        "link" => {
            let Some(id) = required_str(arguments, "id") else {
                return tool_invalid("missing or invalid `id`");
            };
            let Some(action) = required_str(arguments, "action") else {
                return tool_invalid("missing or invalid `action`");
            };
            if action == "list" {
                args.push("link".to_string());
                args.push("list".to_string());
                args.push(id);
            } else {
                let dep_id = optional_str(arguments, "dep_id");
                let external = optional_str(arguments, "external");
                if dep_id.is_none() && external.is_none() {
                    return tool_invalid("`link` needs `dep_id` or `external`");
                }
                if dep_id.is_some() && external.is_some() {
                    return tool_invalid("`link` takes `dep_id` or `external`, not both");
                }
                let kind = optional_str(arguments, "type");
                let attributes = arguments.get("attributes").and_then(Value::as_object);
                let url = optional_str(arguments, "url");
                let blocking = arguments.get("blocking").and_then(Value::as_bool);
                // Plain depend/undepend shorthand when no typed-link options
                // are given and the target is a task.
                let shorthand = dep_id.is_some()
                    && kind.is_none()
                    && external.is_none()
                    && attributes.is_none()
                    && url.is_none()
                    && blocking.is_none();
                if shorthand {
                    let command = match action.as_str() {
                        "add" => "depend",
                        "remove" => "undepend",
                        _ => return tool_invalid("`action` must be `add`, `remove`, or `list`"),
                    };
                    args.push(command.to_string());
                    args.push(id);
                    args.push(dep_id.unwrap());
                } else {
                    let subcommand = match action.as_str() {
                        "add" => "add",
                        "remove" => "remove",
                        _ => return tool_invalid("`action` must be `add`, `remove`, or `list`"),
                    };
                    args.push("link".to_string());
                    args.push(subcommand.to_string());
                    args.push(id);
                    args.push("--type".to_string());
                    args.push(kind.unwrap_or_else(|| {
                        if dep_id.is_some() {
                            "blocks".to_string()
                        } else {
                            "relates-to".to_string()
                        }
                    }));
                    if let Some(dep_id) = dep_id {
                        args.push("--to".to_string());
                        args.push(dep_id);
                    }
                    if let Some(external) = external {
                        args.push("--external".to_string());
                        args.push(external);
                    }
                    if let Some(url) = url {
                        args.push("--url".to_string());
                        args.push(url);
                    }
                    match blocking {
                        Some(true) => args.push("--blocking".to_string()),
                        Some(false) => args.push("--no-blocking".to_string()),
                        None => {}
                    }
                    if let Some(attributes) = attributes {
                        for (key, value) in attributes {
                            let rendered = match value {
                                Value::String(text) => text.clone(),
                                other => serde_json::to_string(other).unwrap_or_default(),
                            };
                            args.push("--attribute".to_string());
                            args.push(format!("{key}={rendered}"));
                        }
                    }
                }
            }
        }
        "delete" => {
            let Some(id) = required_str(arguments, "id") else {
                return tool_invalid("missing or invalid `id`");
            };
            args.push("delete".to_string());
            args.push(id);
            if let Some(by) = optional_str(arguments, "by") {
                args.push("--by".to_string());
                args.push(by);
            }
            if bool_arg(arguments, "force") {
                args.push("--force".to_string());
            }
        }
        "label" => {
            let Some(action) = required_str(arguments, "action") else {
                return tool_invalid("missing or invalid `action`");
            };
            match action.as_str() {
                "add" | "remove" => {
                    let Some(id) = required_str(arguments, "id") else {
                        return tool_invalid("missing or invalid `id`");
                    };
                    let Some(labels) = arguments.get("labels").and_then(Value::as_array) else {
                        return tool_invalid("missing or invalid `labels`");
                    };
                    if labels.is_empty() {
                        return tool_invalid("`labels` must not be empty");
                    }
                    args.push("label".to_string());
                    args.push(action);
                    args.push(id);
                    for label in labels {
                        let Some(label) = label.as_str() else {
                            return tool_invalid("`labels` must be an array of strings");
                        };
                        args.push(label.to_string());
                    }
                }
                "list" => {
                    args.push("label".to_string());
                    args.push("list".to_string());
                }
                _ => return tool_invalid("`action` must be `add`, `remove`, or `list`"),
            }
        }
        "batch" => {
            let Some(operations) = arguments.get("operations").filter(|v| v.is_array()) else {
                return tool_invalid("missing or invalid `operations`");
            };
            let nanos = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|elapsed| elapsed.as_nanos())
                .unwrap_or_default();
            let path = std::env::temp_dir()
                .join(format!("orbit-batch-{}-{nanos}.json", std::process::id()));
            let body = json!({ "operations": operations }).to_string();
            if let Err(error) = std::fs::write(&path, body) {
                return tool_invalid(format!("cannot stage the batch: {error}"));
            }
            args.extend(["batch".to_string(), "--file".to_string()]);
            args.push(path.to_string_lossy().to_string());
            batch_file = Some(path);
            if bool_arg(arguments, "dry_run") {
                args.push("--dry-run".to_string());
            }
            if let Some(by) = optional_str(arguments, "by") {
                args.push("--by".to_string());
                args.push(by);
            }
        }
        "property" => {
            let Some(id) = required_str(arguments, "id") else {
                return tool_invalid("missing or invalid `id`");
            };
            let Some(owner) = required_str(arguments, "owner") else {
                return tool_invalid("missing or invalid `owner`");
            };
            args.push("property".to_string());
            match arguments.get("value") {
                None => return tool_invalid("missing `value`; pass null to remove the Property"),
                Some(Value::Null) => {
                    args.extend(["remove".to_string(), id, owner]);
                }
                Some(value @ Value::Object(_)) => {
                    args.extend(["set".to_string(), id, owner]);
                    args.push("--value".to_string());
                    args.push(value.to_string());
                }
                Some(_) => return tool_invalid("`value` must be an object or null"),
            }
        }
        "comment" => {
            let Some(action) = required_str(arguments, "action") else {
                return tool_invalid("missing or invalid `action`");
            };
            match action.as_str() {
                "add" => {
                    let Some(id) = required_str(arguments, "id") else {
                        return tool_invalid("missing or invalid `id`");
                    };
                    let Some(body) = required_str(arguments, "body") else {
                        return tool_invalid("missing or invalid `body`");
                    };
                    args.push("comment".to_string());
                    args.push("add".to_string());
                    args.push(id);
                    args.push("--body".to_string());
                    args.push(body);
                    if let Some(by) = optional_str(arguments, "by") {
                        args.push("--by".to_string());
                        args.push(by);
                    }
                }
                "list" => {
                    let Some(id) = required_str(arguments, "id") else {
                        return tool_invalid("missing or invalid `id`");
                    };
                    args.push("comment".to_string());
                    args.push("list".to_string());
                    args.push(id);
                }
                _ => return tool_invalid("`action` must be `add` or `list`"),
            }
        }
        other => match crate::registry::tool_argv(other, arguments) {
            Ok(argv) => args = argv,
            Err(message) => return tool_invalid(message),
        },
    }

    // `--if-version` and `--idempotency-key` are global CLI flags declared on
    // the mutation tools' schemas; forward them for custom and registry-driven
    // tools alike. The CLI itself rejects them where they do not apply.
    if let Some(version) = optional_str(arguments, "if_version") {
        args.push("--if-version".to_string());
        args.push(version);
    }
    if let Some(key) = optional_str(arguments, "idempotency_key") {
        args.push("--idempotency-key".to_string());
        args.push(key);
    }

    let outcome = run_orbit(executable, dir, &args, true);
    if let Some(path) = batch_file {
        let _ = std::fs::remove_file(path);
    }
    match outcome {
        Ok(run) => {
            let stdout = run.stdout.trim();
            if stdout.is_empty() && !run.success && run.code == Some(2) {
                return tool_invalid(format!(
                    "orbit rejected the arguments: {}",
                    run.stderr.trim()
                ));
            }
            match serde_json::from_str::<Value>(stdout) {
                Ok(envelope) => {
                    let is_error = !run.success || envelope.get("ok") == Some(&Value::Bool(false));
                    ToolResponse { envelope, is_error }
                }
                Err(error) => tool_internal(format!(
                    "orbit produced invalid JSON: {error}; stderr: {}",
                    run.stderr.trim()
                )),
            }
        }
        Err(error) => tool_internal(error),
    }
}

fn resources_list(executable: &Path, dir: Option<&Path>) -> Value {
    // `--all` keeps every Repository in scope and `--include-closed` keeps
    // terminal Tasks, matching the resources list's previous coverage.
    let args = vec![
        "list".to_string(),
        "--all".to_string(),
        "--include-closed".to_string(),
    ];
    let mut resources = Vec::new();

    if let Ok(run) = run_orbit(executable, dir, &args, true)
        && run.success
        && let Ok(envelope) = serde_json::from_str::<Value>(run.stdout.trim())
        && let Some(tasks) = envelope.pointer("/data/tasks").and_then(Value::as_array)
    {
        for task in tasks.iter().take(200) {
            let id = task.get("id").and_then(Value::as_str).unwrap_or("");
            if id.is_empty() {
                continue;
            }
            let title = task.get("title").and_then(Value::as_str).unwrap_or(id);
            resources.push(json!({
                "uri": format!("orbit://tasks/{id}"),
                "name": title,
                "description": format!("Orbit task {id}"),
                "mimeType": "text/markdown"
            }));
        }
    }

    json!({ "resources": resources })
}

fn resources_read(
    params: &Value,
    executable: &Path,
    dir: Option<&Path>,
) -> std::result::Result<Value, (i64, String)> {
    let uri = params
        .get("uri")
        .and_then(Value::as_str)
        .ok_or((-32602, "missing resource uri".to_string()))?;
    let Some(id) = uri.strip_prefix("orbit://tasks/") else {
        return Err((-32602, format!("unsupported resource uri: {uri}")));
    };
    if id.is_empty() || id.contains('/') {
        return Err((-32602, "invalid task id in resource uri".to_string()));
    }

    let args = vec!["show".to_string(), id.to_string()];
    let run = run_orbit(executable, dir, &args, false).map_err(|error| (-32603, error))?;
    if !run.success {
        return Err((-32002, format!("resource not found: {uri}")));
    }

    Ok(json!({
        "contents": [{
            "uri": uri,
            "mimeType": "text/markdown",
            "text": run.stdout
        }]
    }))
}

struct OrbitRun {
    stdout: String,
    stderr: String,
    success: bool,
    code: Option<i32>,
}

fn run_orbit(
    executable: &Path,
    dir: Option<&Path>,
    args: &[String],
    json: bool,
) -> std::result::Result<OrbitRun, String> {
    let mut command = Command::new(executable);
    if let Some(dir) = dir {
        command.arg("--dir").arg(dir);
    }
    command.args(args);
    if json {
        command.arg("--json");
    }
    command
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());

    let output = command
        .output()
        .map_err(|error| format!("spawn orbit: {error}"))?;

    Ok(OrbitRun {
        stdout: String::from_utf8_lossy(&output.stdout).to_string(),
        stderr: String::from_utf8_lossy(&output.stderr).to_string(),
        success: output.status.success(),
        code: output.status.code(),
    })
}

fn required_str(arguments: &Value, key: &str) -> Option<String> {
    arguments
        .get(key)
        .and_then(Value::as_str)
        .map(str::to_string)
}

fn optional_str(arguments: &Value, key: &str) -> Option<String> {
    required_str(arguments, key)
}

fn optional_u64(arguments: &Value, key: &str) -> Option<u64> {
    arguments.get(key).and_then(Value::as_u64)
}

fn bool_arg(arguments: &Value, key: &str) -> bool {
    arguments.get(key).and_then(Value::as_bool).unwrap_or(false)
}

/// MCP tools whose arguments are translated to argv by hand in
/// [`dispatch_tool`]. Registry-driven tools are not listed here; the
/// registry parity tests keep both sets in sync with `registry::TOOLS`.
#[cfg(test)]
pub(crate) fn custom_tool_names() -> &'static [&'static str] {
    &[
        "ready",
        "list",
        "get",
        "create",
        "update",
        "claim",
        "assign",
        "release",
        "heartbeat",
        "checkpoint",
        "finish",
        "transition",
        "link",
        "delete",
        "label",
        "batch",
        "property",
        "comment",
    ]
}

fn tool_invalid(message: impl Into<String>) -> ToolResponse {
    tool_error("INVALID_ARGUMENT", message)
}

fn tool_internal(message: impl Into<String>) -> ToolResponse {
    tool_error("INTERNAL_ERROR", message)
}

fn tool_error(code: &str, message: impl Into<String>) -> ToolResponse {
    let message = message.into();
    ToolResponse {
        envelope: json!({
            "ok": false,
            "error": {
                "type": crate::output::problem_type(code),
                "title": crate::output::problem_title(code),
                "code": code,
                "message": message,
                "detail": message,
                "retryable": false,
                "hint": crate::output::default_hint(code)
            }
        }),
        is_error: true,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn initialize_echoes_the_protocol_version() {
        let result = initialize_result(&json!({ "protocolVersion": "2025-11-25" }));
        assert_eq!(result["protocolVersion"], "2025-11-25");
        assert_eq!(result["serverInfo"]["name"], "orbit");
    }

    #[test]
    fn tool_list_contains_task_tools() {
        let tools = tool_definitions();
        assert_eq!(tools.len(), 63);
        let names: Vec<&str> = tools
            .iter()
            .filter_map(|tool| tool.get("name").and_then(Value::as_str))
            .collect();
        for expected in [
            "ready",
            "list",
            "get",
            "create",
            "update",
            "claim",
            "assign",
            "release",
            "heartbeat",
            "checkpoint",
            "finish",
            "transition",
            "submit",
            "review",
            "ask",
            "answer",
            "link",
            "delete",
            "label",
            "comment",
            "property",
            "batch",
            "tree",
            "reopen",
            "prime",
            "search",
            "digest",
            "export",
            "changes",
            "history",
            "doctor",
            "sync",
            "repo_add",
            "slug_migrate",
            "migrate_model",
            "board_sync",
            "remote_status",
            "remote_issues",
            "remote_auto_push",
            "triage_context",
            "triage_apply",
        ] {
            assert!(names.contains(&expected), "{expected}");
        }
        // Every tool carries MCP safety annotations and a structured output
        // schema, generated from the command registry.
        for tool in &tools {
            let annotations = tool["annotations"].as_object().expect("annotations");
            for hint in [
                "readOnlyHint",
                "destructiveHint",
                "idempotentHint",
                "openWorldHint",
            ] {
                assert!(annotations.contains_key(hint), "{}: {hint}", tool["name"]);
            }
            assert!(tool["outputSchema"].is_object());
        }
        let get = tools.iter().find(|tool| tool["name"] == "get").unwrap();
        assert_eq!(get["annotations"]["readOnlyHint"], true);
        let delete = tools.iter().find(|tool| tool["name"] == "delete").unwrap();
        assert_eq!(delete["annotations"]["destructiveHint"], true);
    }

    #[test]
    fn tool_results_are_marked_as_errors() {
        let response = tool_invalid("bad input");
        assert!(response.is_error);
        assert_eq!(response.envelope["error"]["code"], "INVALID_ARGUMENT");
    }
}

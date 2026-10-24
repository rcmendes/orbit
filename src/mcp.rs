use std::io::{BufRead, Write};
use std::path::Path;
use std::process::{Command, Stdio};

use serde_json::{Value, json};

use crate::error::{OrbitError, Result};

const PROTOCOL_VERSION: &str = "2025-06-18";
const INSTRUCTIONS: &str = "Orbit is the task tracker for this repository. Use `ready` with `claim:true` and a stable `by` to take work, `heartbeat` with the same `by` while working, and `finish` with `by` when done. `create`, `update`, `link`, and `delete` shape the board. Never edit task files directly.";

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
    vec![
        tool(
            "ready",
            "List ready tasks. With claim=true, atomically claim the highest-priority one.",
            json!({
                "claim": { "type": "boolean", "description": "Claim the first ready task." },
                "by": { "type": "string", "description": "Agent identity for the claim." },
                "limit": { "type": "integer", "minimum": 1, "description": "Maximum tasks to list." },
                "parent": { "type": "string", "description": "Only Ready SubTasks of this Parent Task." },
                "repo": { "type": "string", "description": "Only tasks with this repository label. Shorthand for label repo:NAME." },
                "labels": { "type": "array", "items": { "type": "string" }, "description": "Only tasks carrying every label." }
            }),
            &[],
        ),
        tool(
            "list",
            "List tasks with filters. Terminal tasks are hidden unless all=true.",
            json!({
                "status": { "type": "string", "enum": ["todo", "in_progress", "done", "cancelled"] },
                "priority": { "type": "integer", "minimum": 0, "maximum": 3 },
                "claimed_by": { "type": "string" },
                "waiting": { "type": "boolean", "description": "Only tasks with an external blocker." },
                "label": { "type": "string", "description": "Filter by label." },
                "unlabeled": { "type": "boolean", "description": "Only tasks with no labels." },
                "parent": { "type": "string", "description": "Only SubTasks of this Parent Task." },
                "repo": { "type": "string", "description": "Only tasks with this repository label. Shorthand for label repo:NAME." },
                "labels": { "type": "array", "items": { "type": "string" }, "description": "Only tasks carrying every label." },
                "all": { "type": "boolean", "description": "Include done and cancelled tasks." }
            }),
            &[],
        ),
        tool(
            "get",
            "Read one task, including its Markdown body and dependents.",
            json!({ "id": { "type": "string", "description": "Task id, for example orb-a7b2c9." } }),
            &["id"],
        ),
        tool(
            "create",
            "Create a task. Returns the new task id.",
            json!({
                "title": { "type": "string" },
                "priority": { "type": "integer", "minimum": 0, "maximum": 3 },
                "labels": { "type": "array", "items": { "type": "string" } },
                "depends_on": { "type": "array", "items": { "type": "string" } },
                "parent": { "type": "string", "description": "Create a SubTask under this Parent Task." },
                "waiting_on": { "type": "string", "description": "External blocker." },
                "body": { "type": "string", "description": "Markdown body and acceptance criteria." }
            }),
            &["title"],
        ),
        tool(
            "update",
            "Change a task's title, body, priority, or external wait. Does not change status.",
            json!({
                "id": { "type": "string" },
                "title": { "type": "string" },
                "body": { "type": "string" },
                "priority": { "type": "integer", "minimum": 0, "maximum": 3 },
                "waiting_on": { "type": "string" },
                "clear_waiting": { "type": "boolean" }
            }),
            &["id"],
        ),
        tool(
            "claim",
            "Claim a specific task.",
            json!({
                "id": { "type": "string" },
                "by": { "type": "string", "description": "Agent identity." },
                "steal": { "type": "boolean", "description": "Take over a live or stale claim." }
            }),
            &["id"],
        ),
        tool(
            "release",
            "Release a claim and return the task to todo.",
            json!({
                "id": { "type": "string" },
                "by": { "type": "string" },
                "force": { "type": "boolean", "description": "Release another agent's claim." }
            }),
            &["id"],
        ),
        tool(
            "heartbeat",
            "Extend a claim's lease while working.",
            json!({
                "id": { "type": "string" },
                "by": { "type": "string" }
            }),
            &["id"],
        ),
        tool(
            "finish",
            "Complete or cancel a task. Done still requires finished dependencies.",
            json!({
                "id": { "type": "string" },
                "status": { "type": "string", "enum": ["done", "cancelled"] },
                "by": { "type": "string" },
                "force": { "type": "boolean", "description": "Override a competing live claim; never bypasses dependencies." },
                "comment": { "type": "string", "description": "Completion note recorded atomically with the status change." }
            }),
            &["id", "status"],
        ),
        tool(
            "link",
            "Add or remove a dependency.",
            json!({
                "id": { "type": "string" },
                "dep_id": { "type": "string" },
                "action": { "type": "string", "enum": ["add", "remove"] }
            }),
            &["id", "dep_id", "action"],
        ),
        tool(
            "delete",
            "Delete a task. With force=true, clean up dependent tasks first.",
            json!({
                "id": { "type": "string" },
                "by": { "type": "string" },
                "force": { "type": "boolean" }
            }),
            &["id"],
        ),
        tool(
            "label",
            "Add, remove, or list task labels.",
            json!({
                "action": { "type": "string", "enum": ["add", "remove", "list"] },
                "id": { "type": "string" },
                "labels": { "type": "array", "items": { "type": "string" } }
            }),
            &["action"],
        ),
        tool(
            "comment",
            "Add or list task comments.",
            json!({
                "action": { "type": "string", "enum": ["add", "list"] },
                "id": { "type": "string" },
                "body": { "type": "string" },
                "by": { "type": "string" }
            }),
            &["action"],
        ),
    ]
}

fn tool(name: &str, description: &str, properties: Value, required: &[&str]) -> Value {
    json!({
        "name": name,
        "description": description,
        "inputSchema": {
            "type": "object",
            "properties": properties,
            "required": required,
            "additionalProperties": false
        }
    })
}

fn resource_template() -> Value {
    json!({
        "uriTemplate": "orbit://tasks/{id}",
        "name": "orbit-task",
        "title": "Orbit task",
        "description": "A task's Markdown body and acceptance criteria.",
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
        "content": [{ "type": "text", "text": text }]
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
    if let Some(status) = optional_str(arguments, "status") {
        if name == "list"
            && !matches!(
                status.as_str(),
                "todo" | "in_progress" | "done" | "cancelled"
            )
        {
            return tool_invalid("`status` must be todo, in_progress, done, or cancelled");
        }
        if name == "finish" && !matches!(status.as_str(), "done" | "cancelled") {
            return tool_invalid("`status` must be `done` or `cancelled`");
        }
    }
    if optional_u64(arguments, "priority").is_some_and(|priority| priority > 3) {
        return tool_invalid("`priority` must be between 0 and 3");
    }
    if name == "update"
        && arguments.get("waiting_on").is_some()
        && bool_arg(arguments, "clear_waiting")
    {
        return tool_invalid("`waiting_on` and `clear_waiting` are mutually exclusive");
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
    if arguments
        .get("parent")
        .is_some_and(|value| !value.is_string())
    {
        return tool_invalid("`parent` must be a task id string");
    }
    if arguments
        .get("repo")
        .is_some_and(|value| !value.is_string())
    {
        return tool_invalid("`repo` must be a string");
    }
    if arguments
        .get("labels")
        .is_some_and(|value| !value.is_array())
    {
        return tool_invalid("`labels` must be an array of strings");
    }

    let mut args: Vec<String> = Vec::new();

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
            if let Some(limit) = optional_u64(arguments, "limit") {
                args.push("--limit".to_string());
                args.push(limit.to_string());
            }
            if let Some(parent) = optional_str(arguments, "parent") {
                args.push("--parent".to_string());
                args.push(parent);
            }
            if let Some(repo) = optional_str(arguments, "repo") {
                args.push("--repo".to_string());
                args.push(repo);
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
        }
        "list" => {
            args.push("list".to_string());
            if let Some(status) = optional_str(arguments, "status") {
                args.push("--status".to_string());
                args.push(status);
            }
            if let Some(priority) = optional_u64(arguments, "priority") {
                args.push("--priority".to_string());
                args.push(priority.to_string());
            }
            if let Some(claimed_by) = optional_str(arguments, "claimed_by") {
                args.push("--claimed-by".to_string());
                args.push(claimed_by);
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
            if let Some(repo) = optional_str(arguments, "repo") {
                args.push("--repo".to_string());
                args.push(repo);
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
            if bool_arg(arguments, "waiting") {
                args.push("--waiting".to_string());
            }
            if bool_arg(arguments, "all") {
                args.push("--all".to_string());
            }
        }
        "get" => {
            let Some(id) = required_str(arguments, "id") else {
                return tool_invalid("missing or invalid `id`");
            };
            args.push("show".to_string());
            args.push(id);
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
            if let Some(priority) = optional_u64(arguments, "priority") {
                args.push("-p".to_string());
                args.push(priority.to_string());
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
            if let Some(priority) = optional_u64(arguments, "priority") {
                args.push("--priority".to_string());
                args.push(priority.to_string());
            }
            if let Some(waiting_on) = optional_str(arguments, "waiting_on") {
                args.push("--waiting-on".to_string());
                args.push(waiting_on);
            }
            if bool_arg(arguments, "clear_waiting") {
                args.push("--clear-waiting".to_string());
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
            if bool_arg(arguments, "steal") {
                args.push("--steal".to_string());
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
        "link" => {
            let Some(id) = required_str(arguments, "id") else {
                return tool_invalid("missing or invalid `id`");
            };
            let Some(dep_id) = required_str(arguments, "dep_id") else {
                return tool_invalid("missing or invalid `dep_id`");
            };
            let Some(action) = required_str(arguments, "action") else {
                return tool_invalid("missing or invalid `action`");
            };
            let command = match action.as_str() {
                "add" => "depend",
                "remove" => "undepend",
                _ => return tool_invalid("`action` must be `add` or `remove`"),
            };
            args.push(command.to_string());
            args.push(id);
            args.push(dep_id);
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
        other => return tool_invalid(format!("unknown tool `{other}`")),
    }

    match run_orbit(executable, dir, &args, true) {
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
    let args = vec!["list".to_string(), "--all".to_string()];
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

fn tool_invalid(message: impl Into<String>) -> ToolResponse {
    tool_error("INVALID_ARGUMENT", message)
}

fn tool_internal(message: impl Into<String>) -> ToolResponse {
    tool_error("INTERNAL_ERROR", message)
}

fn tool_error(code: &str, message: impl Into<String>) -> ToolResponse {
    ToolResponse {
        envelope: json!({
            "ok": false,
            "error": {
                "code": code,
                "message": message.into(),
                "retryable": false
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
    fn tool_list_contains_all_thirteen_tools() {
        let tools = tool_definitions();
        assert_eq!(tools.len(), 13);
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
            "release",
            "heartbeat",
            "finish",
            "link",
            "delete",
            "label",
            "comment",
        ] {
            assert!(names.contains(&expected), "{expected}");
        }
    }

    #[test]
    fn tool_results_are_marked_as_errors() {
        let response = tool_invalid("bad input");
        assert!(response.is_error);
        assert_eq!(response.envelope["error"]["code"], "INVALID_ARGUMENT");
    }
}

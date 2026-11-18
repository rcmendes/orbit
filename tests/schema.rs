mod common;

use common::TestRepo;
use serde_json::Value;

fn schema_json(repo: &TestRepo, args: &[&str]) -> Value {
    let output = repo.orbit(args);
    assert!(
        output.status.success(),
        "orbit {args:?} failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let envelope: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(envelope["ok"], true, "{envelope}");
    envelope["data"].clone()
}

#[test]
fn orbit_schema_publishes_the_command_registry() {
    let repo = TestRepo::new();
    // `orbit schema` needs no Board.
    let document = schema_json(&repo, &["schema", "--json"]);

    assert_eq!(document["contract"], "orbit-command-registry/1");
    let commands = document["commands"].as_array().unwrap();
    let tools = document["tools"].as_array().unwrap();
    assert!(commands.len() > 50, "{}", commands.len());
    assert!(tools.len() > 40, "{}", tools.len());

    let ready = commands
        .iter()
        .find(|entry| entry["command"] == "orbit ready")
        .expect("orbit ready entry");
    assert_eq!(ready["mcp_tool"], "ready");
    assert_eq!(ready["annotations"]["readOnlyHint"], false);
    assert!(ready["inputSchema"]["properties"]["claim"].is_object());

    let init = commands
        .iter()
        .find(|entry| entry["command"] == "orbit init")
        .expect("orbit init entry");
    assert!(init["mcp_tool"].is_null());
    assert!(init["mcp_exempt"].is_string());

    let delete_tool = tools
        .iter()
        .find(|tool| tool["name"] == "delete")
        .expect("delete tool");
    assert_eq!(delete_tool["annotations"]["destructiveHint"], true);
    assert!(delete_tool["outputSchema"]["properties"]["ok"].is_object());
}

#[test]
fn orbit_schema_filters_one_command_or_tool() {
    let repo = TestRepo::new();

    let ready = schema_json(&repo, &["schema", "ready", "--json"]);
    assert_eq!(ready["command"]["command"], "orbit ready");

    let remote = schema_json(&repo, &["schema", "remote issues refresh", "--json"]);
    assert_eq!(remote["command"]["command"], "orbit remote issues refresh");

    let create = schema_json(&repo, &["schema", "create", "--json"]);
    assert_eq!(create["tool"]["name"], "create");

    let missing = repo.orbit(&["schema", "no-such-command", "--json"]);
    assert!(!missing.status.success());
    let envelope: Value = serde_json::from_slice(&missing.stdout).unwrap();
    assert_eq!(envelope["ok"], false);
    assert_eq!(envelope["error"]["code"], "INVALID_ARGUMENT");
}

#[test]
fn orbit_schema_docs_prints_the_generated_reference() {
    let repo = TestRepo::new();
    let output = repo.orbit(&["schema", "--docs"]);
    assert!(output.status.success());
    let text = String::from_utf8_lossy(&output.stdout);
    assert!(text.contains("BEGIN GENERATED COMMANDS"));
    assert!(text.contains("`orbit ready`"));
    assert!(text.contains("`remote_auto_push`"));
}

mod common;

use common::TestRepo;
use serde_json::{Value, json};
use std::io::{BufRead, BufReader, Write};
use std::process::{Child, ChildStdin, ChildStdout, Stdio};

struct McpClient {
    child: Child,
    stdin: ChildStdin,
    stdout: BufReader<ChildStdout>,
}

impl McpClient {
    fn start(repo: &TestRepo) -> Self {
        let mut command = repo.orbit_command(&["mcp"]);
        command
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        let mut child = command.spawn().unwrap();
        let stdin = child.stdin.take().unwrap();
        let stdout = BufReader::new(child.stdout.take().unwrap());
        Self {
            child,
            stdin,
            stdout,
        }
    }

    fn send(&mut self, message: Value) {
        writeln!(self.stdin, "{}", serde_json::to_string(&message).unwrap()).unwrap();
        self.stdin.flush().unwrap();
    }

    fn notify(&mut self, method: &str) {
        self.send(json!({ "jsonrpc": "2.0", "method": method }));
    }

    fn read(&mut self) -> Value {
        let mut line = String::new();
        self.stdout.read_line(&mut line).unwrap();
        serde_json::from_str(&line)
            .unwrap_or_else(|error| panic!("invalid MCP response: {error}; line: {line:?}"))
    }

    fn request(&mut self, id: u64, method: &str, params: Value) -> Value {
        self.send(json!({
            "jsonrpc": "2.0",
            "id": id,
            "method": method,
            "params": params
        }));
        self.read()
    }

    fn call(&mut self, id: u64, name: &str, arguments: Value) -> Value {
        self.request(
            id,
            "tools/call",
            json!({ "name": name, "arguments": arguments }),
        )
    }
}

impl Drop for McpClient {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

fn tool_envelope(response: &Value) -> Value {
    let text = response["result"]["content"][0]["text"]
        .as_str()
        .unwrap_or_else(|| panic!("no tool text: {response}"));
    serde_json::from_str(text).unwrap()
}

fn initialize(client: &mut McpClient) -> Value {
    let response = client.request(
        1,
        "initialize",
        json!({
            "protocolVersion": "2025-06-18",
            "capabilities": {},
            "clientInfo": { "name": "orbit-tests", "version": "0" }
        }),
    );
    client.notify("notifications/initialized");
    response
}

#[test]
fn initialize_and_tools_list_match_the_mcp_contract() {
    let repo = TestRepo::new();
    repo.init();
    let mut client = McpClient::start(&repo);

    let init = initialize(&mut client);
    assert_eq!(init["jsonrpc"], "2.0");
    assert_eq!(init["result"]["serverInfo"]["name"], "orbit");
    assert!(init["result"]["capabilities"]["tools"].is_object());
    assert!(
        init["result"]["instructions"]
            .as_str()
            .unwrap()
            .contains("Orbit")
    );

    let tools = client.request(2, "tools/list", json!({}));
    let names: Vec<&str> = tools["result"]["tools"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|tool| tool["name"].as_str())
        .collect();
    assert_eq!(names.len(), 14);
    for expected in [
        "ready",
        "list",
        "get",
        "create",
        "update",
        "claim",
        "release",
        "heartbeat",
        "checkpoint",
        "finish",
        "link",
        "delete",
        "label",
        "comment",
    ] {
        assert!(names.contains(&expected), "{expected}");
    }

    let ping = client.request(3, "ping", json!({}));
    assert_eq!(ping["result"], json!({}));

    let unknown = client.request(4, "not/a/method", json!({}));
    assert_eq!(unknown["error"]["code"], -32601);
}

#[test]
fn create_claim_heartbeat_and_finish_round_trip() {
    let repo = TestRepo::new();
    repo.init();
    let mut client = McpClient::start(&repo);
    initialize(&mut client);

    let created = client.call(
        10,
        "create",
        json!({
            "title": "MCP task",
            "priority": 1,
            "body": "Made over MCP"
        }),
    );
    let created = tool_envelope(&created);
    assert_eq!(created["ok"], true);
    let id = created["data"]["id"].as_str().unwrap().to_string();

    let ready = client.call(11, "ready", json!({}));
    let ready = tool_envelope(&ready);
    assert_eq!(ready["data"]["count"], 1);
    assert_eq!(ready["data"]["tasks"][0]["id"], id);

    let claimed = client.call(12, "ready", json!({ "claim": true, "by": "claude" }));
    let claimed = tool_envelope(&claimed);
    assert_eq!(claimed["data"]["status"], "in_progress");
    assert_eq!(claimed["data"]["claimed_by"], "claude");

    let heartbeat = client.call(13, "heartbeat", json!({ "id": id, "by": "claude" }));
    let heartbeat = tool_envelope(&heartbeat);
    assert_eq!(heartbeat["ok"], true);

    let get = client.call(14, "get", json!({ "id": id }));
    let get = tool_envelope(&get);
    assert_eq!(get["data"]["body"], "Made over MCP");

    let finished = client.call(
        15,
        "finish",
        json!({ "id": id, "status": "done", "by": "claude", "comment": "Finished over MCP." }),
    );
    let finished = tool_envelope(&finished);
    assert_eq!(finished["data"]["status"], "done");
    assert_eq!(finished["data"]["comment_count"], 1);
    assert_eq!(
        finished["data"]["comments"][0]["body"],
        "Finished over MCP."
    );

    let list = client.call(16, "list", json!({ "all": true }));
    let list = tool_envelope(&list);
    assert_eq!(list["data"]["count"], 1);
    assert_eq!(list["data"]["tasks"][0]["status"], "done");
}

#[test]
fn checkpoint_round_trips_over_mcp() {
    let repo = TestRepo::new();
    repo.init();
    let mut client = McpClient::start(&repo);
    initialize(&mut client);

    let created = client.call(20, "create", json!({ "title": "MCP checkpoint" }));
    let created = tool_envelope(&created);
    let id = created["data"]["id"].as_str().unwrap().to_string();

    let claimed = client.call(21, "claim", json!({ "id": id, "by": "claude" }));
    let claimed = tool_envelope(&claimed);
    assert_eq!(claimed["data"]["claimed_by"], "claude");

    let recorded = client.call(
        22,
        "checkpoint",
        json!({
            "id": id,
            "progress": "Parser done",
            "next": "Wire the CLI",
            "blocker": "Waiting on review",
            "by": "claude"
        }),
    );
    let recorded = tool_envelope(&recorded);
    assert_eq!(recorded["data"]["checkpoint"]["progress"], "Parser done");
    assert_eq!(recorded["data"]["checkpoint"]["next"], "Wire the CLI");
    assert_eq!(
        recorded["data"]["checkpoint"]["blocker"],
        "Waiting on review"
    );

    let updated = client.call(
        23,
        "checkpoint",
        json!({ "id": id, "clear_blocker": true, "by": "claude" }),
    );
    let updated = tool_envelope(&updated);
    assert_eq!(updated["data"]["checkpoint"]["next"], "Wire the CLI");
    assert!(updated["data"]["checkpoint"]["blocker"].is_null());

    let get = client.call(24, "get", json!({ "id": id }));
    let get = tool_envelope(&get);
    assert_eq!(get["data"]["checkpoint"]["progress"], "Parser done");

    let invalid = client.call(
        25,
        "checkpoint",
        json!({ "id": id, "next": "x", "clear_next": true, "by": "claude" }),
    );
    assert_eq!(invalid["result"]["isError"], true);
}

#[test]
fn subtasks_are_available_over_mcp() {
    let repo = TestRepo::new();
    repo.init();
    let mut client = McpClient::start(&repo);
    initialize(&mut client);

    let parent = client.call(70, "create", json!({ "title": "Feature" }));
    let parent = tool_envelope(&parent);
    let parent_id = parent["data"]["id"].as_str().unwrap().to_string();

    let child = client.call(
        71,
        "create",
        json!({
            "title": "Part",
            "parent": parent_id.clone(),
            "labels": ["repo:api", "ready-for-agent"]
        }),
    );
    let child = tool_envelope(&child);
    let child_id = child["data"]["id"].as_str().unwrap().to_string();
    assert!(child_id.starts_with(&format!("{parent_id}.")));
    assert_eq!(child["data"]["parent"], parent_id.as_str());

    let ready = client.call(72, "ready", json!({ "parent": parent_id.clone() }));
    let ready = tool_envelope(&ready);
    assert_eq!(ready["data"]["count"], 1);
    assert_eq!(ready["data"]["tasks"][0]["id"], child_id.as_str());

    let list = client.call(
        73,
        "list",
        json!({ "parent": parent_id.clone(), "all": true }),
    );
    let list = tool_envelope(&list);
    assert_eq!(list["data"]["count"], 1);
    assert_eq!(list["data"]["tasks"][0]["id"], child_id.as_str());

    let scoped = client.call(74, "list", json!({ "repo": "api", "all": true }));
    let scoped = tool_envelope(&scoped);
    assert_eq!(scoped["data"]["count"], 1);
    assert_eq!(scoped["data"]["tasks"][0]["id"], child_id.as_str());

    let ready_scoped = client.call(75, "ready", json!({ "labels": ["repo:api"] }));
    let ready_scoped = tool_envelope(&ready_scoped);
    assert_eq!(ready_scoped["data"]["count"], 1);
    assert_eq!(ready_scoped["data"]["tasks"][0]["id"], child_id.as_str());

    let excluded = client.call(
        76,
        "ready",
        json!({ "exclude_labels": ["ready-for-agent"] }),
    );
    let excluded = tool_envelope(&excluded);
    assert_eq!(excluded["data"]["count"], 0);
}

#[test]
fn type_is_available_over_mcp() {
    let repo = TestRepo::new();
    repo.init();
    let mut client = McpClient::start(&repo);
    initialize(&mut client);

    let epic = client.call(80, "create", json!({ "title": "Epic", "type": "epic" }));
    let epic = tool_envelope(&epic);
    assert_eq!(epic["data"]["type"], "epic");
    let epic_id = epic["data"]["id"].as_str().unwrap().to_string();

    let task = client.call(81, "create", json!({ "title": "Task" }));
    let task = tool_envelope(&task);
    assert_eq!(task["data"]["type"], "task");
    let task_id = task["data"]["id"].as_str().unwrap().to_string();

    let listed = client.call(82, "list", json!({ "types": ["epic"] }));
    let listed = tool_envelope(&listed);
    assert_eq!(listed["data"]["count"], 1);
    assert_eq!(listed["data"]["tasks"][0]["id"], epic_id.as_str());

    let ready = client.call(83, "ready", json!({ "exclude_types": ["epic"] }));
    let ready = tool_envelope(&ready);
    assert_eq!(ready["data"]["count"], 1);
    assert_eq!(ready["data"]["tasks"][0]["id"], task_id.as_str());

    let updated = client.call(84, "update", json!({ "id": task_id, "type": "spike" }));
    let updated = tool_envelope(&updated);
    assert_eq!(updated["data"]["type"], "spike");

    let nested = client.call(
        85,
        "create",
        json!({ "title": "Nested epic", "parent": epic_id, "type": "epic" }),
    );
    let nested = tool_envelope(&nested);
    assert_eq!(nested["ok"], false);
    assert_eq!(nested["error"]["code"], "INVALID_TYPE");
}

#[test]
fn link_and_delete_are_available_over_mcp() {
    let repo = TestRepo::new();
    repo.init();
    let first = repo.add("First", &[]);
    let second = repo.add("Second", &[]);

    let mut client = McpClient::start(&repo);
    initialize(&mut client);

    let linked = client.call(
        20,
        "link",
        json!({ "id": second, "dep_id": first, "action": "add" }),
    );
    let linked = tool_envelope(&linked);
    assert_eq!(linked["ok"], true);

    let get = client.call(21, "get", json!({ "id": second }));
    let get = tool_envelope(&get);
    assert_eq!(get["data"]["depends_on"][0], first);

    let unlinked = client.call(
        22,
        "link",
        json!({ "id": second, "dep_id": first, "action": "remove" }),
    );
    let unlinked = tool_envelope(&unlinked);
    assert_eq!(unlinked["ok"], true);

    let deleted = client.call(23, "delete", json!({ "id": second, "force": true }));
    let deleted = tool_envelope(&deleted);
    assert_eq!(deleted["data"]["deleted"], true);

    let missing = client.call(24, "get", json!({ "id": second }));
    assert_eq!(missing["result"]["isError"], true);
    let missing = tool_envelope(&missing);
    assert_eq!(missing["error"]["code"], "NOT_FOUND");
}

#[test]
fn update_claim_and_release_are_available() {
    let repo = TestRepo::new();
    repo.init();
    let id = repo.add("Original", &[]);
    let mut client = McpClient::start(&repo);
    initialize(&mut client);

    let updated = client.call(
        50,
        "update",
        json!({ "id": id, "title": "Changed", "priority": 0 }),
    );
    let updated = tool_envelope(&updated);
    assert_eq!(updated["data"]["title"], "Changed");
    assert_eq!(updated["data"]["priority"], 0);

    let claimed = client.call(51, "claim", json!({ "id": id, "by": "claude" }));
    let claimed = tool_envelope(&claimed);
    assert_eq!(claimed["data"]["status"], "in_progress");
    assert_eq!(claimed["data"]["claimed_by"], "claude");

    let released = client.call(52, "release", json!({ "id": id, "by": "claude" }));
    let released = tool_envelope(&released);
    assert_eq!(released["data"]["status"], "todo");
    assert!(released["data"]["claimed_by"].is_null());
}

#[test]
fn label_and_comment_tools_work() {
    let repo = TestRepo::new();
    repo.init();
    let mut client = McpClient::start(&repo);
    initialize(&mut client);

    let created = client.call(
        60,
        "create",
        json!({
            "title": "Labeled over MCP",
            "labels": ["needs-triage"]
        }),
    );
    let created = tool_envelope(&created);
    let id = created["data"]["id"].as_str().unwrap().to_string();

    let ready = client.call(61, "ready", json!({}));
    let ready = tool_envelope(&ready);
    assert_eq!(ready["data"]["count"], 0);

    let blocked = client.call(62, "claim", json!({ "id": id, "by": "claude" }));
    assert_eq!(blocked["result"]["isError"], true);
    let blocked = tool_envelope(&blocked);
    assert_eq!(blocked["error"]["code"], "LABEL_BLOCKED");

    let labeled = client.call(
        63,
        "label",
        json!({ "action": "add", "id": id, "labels": ["ready-for-agent"] }),
    );
    let labeled = tool_envelope(&labeled);
    assert_eq!(
        labeled["data"]["labels"],
        json!(["needs-triage", "ready-for-agent"])
    );

    client.call(
        64,
        "label",
        json!({ "action": "remove", "id": id, "labels": ["needs-triage"] }),
    );
    let labels = client.call(65, "label", json!({ "action": "list" }));
    let labels = tool_envelope(&labels);
    assert!(
        labels["data"]["labels"]
            .as_array()
            .unwrap()
            .iter()
            .any(|entry| entry["label"] == "ready-for-agent")
    );

    let ready = client.call(66, "ready", json!({ "claim": true, "by": "claude" }));
    let ready = tool_envelope(&ready);
    assert_eq!(ready["data"]["status"], "in_progress");

    let comment = client.call(
        67,
        "comment",
        json!({ "action": "add", "id": id, "body": "MCP note", "by": "claude" }),
    );
    let comment = tool_envelope(&comment);
    assert_eq!(comment["data"]["count"], 1);

    let comments = client.call(68, "comment", json!({ "action": "list", "id": id }));
    let comments = tool_envelope(&comments);
    assert_eq!(comments["data"]["comments"][0]["body"], "MCP note");
    assert_eq!(comments["data"]["comments"][0]["author"], "claude");

    let get = client.call(69, "get", json!({ "id": id }));
    let get = tool_envelope(&get);
    assert_eq!(get["data"]["comment_count"], 1);
    assert_eq!(get["data"]["labels"][0], "ready-for-agent");

    let filtered = client.call(70, "list", json!({ "label": "ready-for-agent" }));
    let filtered = tool_envelope(&filtered);
    assert_eq!(filtered["data"]["count"], 1);

    let unlabeled = client.call(71, "list", json!({ "unlabeled": true }));
    let unlabeled = tool_envelope(&unlabeled);
    assert_eq!(unlabeled["data"]["count"], 0);
}

#[test]
fn resources_expose_task_markdown() {
    let repo = TestRepo::new();
    repo.init();
    let id = repo.add("Resource task", &["--body", "Read me over MCP"]);

    let mut client = McpClient::start(&repo);
    initialize(&mut client);

    let templates = client.request(30, "resources/templates/list", json!({}));
    assert_eq!(
        templates["result"]["resourceTemplates"][0]["uriTemplate"],
        "orbit://tasks/{id}"
    );

    let resources = client.request(31, "resources/list", json!({}));
    let resources = resources["result"]["resources"].as_array().unwrap();
    assert!(
        resources
            .iter()
            .any(|resource| resource["uri"] == format!("orbit://tasks/{id}"))
    );

    let uri = format!("orbit://tasks/{id}");
    let read = client.request(32, "resources/read", json!({ "uri": uri }));
    let text = read["result"]["contents"][0]["text"].as_str().unwrap();
    assert!(text.contains("Resource task"));
    assert!(text.contains("Read me over MCP"));

    let missing = client.request(
        33,
        "resources/read",
        json!({ "uri": "orbit://tasks/orb-aaaaaa" }),
    );
    assert_eq!(missing["error"]["code"], -32002);
}

#[test]
fn invalid_inputs_are_tool_errors_not_protocol_errors() {
    let repo = TestRepo::new();
    repo.init();
    let mut client = McpClient::start(&repo);
    initialize(&mut client);

    let unknown = client.call(40, "not_a_tool", json!({}));
    assert_eq!(unknown["result"]["isError"], true);
    let unknown = tool_envelope(&unknown);
    assert_eq!(unknown["error"]["code"], "INVALID_ARGUMENT");

    let missing = client.call(41, "get", json!({ "id": "orb-aaaaaa" }));
    assert_eq!(missing["result"]["isError"], true);
    let missing = tool_envelope(&missing);
    assert_eq!(missing["error"]["code"], "NOT_FOUND");

    let bad_status = client.call(
        42,
        "finish",
        json!({ "id": "orb-aaaaaa", "status": "maybe" }),
    );
    assert_eq!(bad_status["result"]["isError"], true);
    let bad_status = tool_envelope(&bad_status);
    assert_eq!(bad_status["error"]["code"], "INVALID_ARGUMENT");
}

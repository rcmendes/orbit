//! Golden tests for the platform contract (ADR 0022): the RFC 9457-style
//! error envelope, the Board contract version handshake, `orbit
//! capabilities`, and the JSON output of the contract surface commands
//! (`add`, `ready`, `claim`, `show`, `transition`, `batch`).

#![recursion_limit = "512"]

mod common;

use common::TestRepo;
use serde_json::{Value, json};

/// Replaces volatile values — generated task IDs and timestamps — with
/// stable placeholders so envelopes compare byte-for-byte as parsed JSON.
fn normalize(value: &Value) -> Value {
    match value {
        Value::Object(map) => Value::Object(
            map.iter()
                .map(|(key, item)| (key.clone(), normalize(item)))
                .collect(),
        ),
        Value::Array(items) => Value::Array(items.iter().map(normalize).collect()),
        Value::String(text) => {
            if is_task_id(text) {
                json!("<id>")
            } else if is_timestamp(text) {
                json!("<timestamp>")
            } else {
                value.clone()
            }
        }
        _ => value.clone(),
    }
}

fn is_task_id(text: &str) -> bool {
    text.strip_prefix("orb-").is_some_and(|rest| {
        !rest.is_empty()
            && rest.len() <= 14
            && rest
                .chars()
                .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '.')
    })
}

fn is_timestamp(text: &str) -> bool {
    text.len() >= 20 && text.as_bytes()[4] == b'-' && text.contains('T')
}

fn golden(repo: &TestRepo, args: &[&str]) -> Value {
    normalize(&repo.ok_json(args))
}

fn fixture() -> (TestRepo, String) {
    let repo = TestRepo::new();
    repo.init();
    let id = repo.add(
        "Write docs",
        &["--reporter", "alice", "-p", "high", "--label", "docs"],
    );
    (repo, id)
}

#[test]
fn golden_add_output() {
    let repo = TestRepo::new();
    repo.init();
    let value = golden(
        &repo,
        &[
            "add",
            "Write docs",
            "--reporter",
            "alice",
            "-p",
            "high",
            "--label",
            "docs",
            "--json",
        ],
    );
    assert_eq!(
        value,
        json!({
            "ok": true,
            "data": {
                "allowed_transitions": ["claim", "defer", "done", "cancel"],
                "assignee": null,
                "assignee_kind": null,
                "blocking": [],
                "body": "",
                "checkpoint": null,
                "checkpoint_archived": null,
                "claimed_at": null,
                "component": null,
                "created_at": "<timestamp>",
                "dependents": [],
                "depth": 0,
                "eligible_for_close": false,
                "external_id": null,
                "id": "<id>",
                "key": "ORB-1",
                "key_aliases": [],
                "labels": ["docs"],
                "lease_until": null,
                "links": [],
                "open_sub_issue_count": 0,
                "open_sub_issues": [],
                "parent": null,
                "priority": "high",
                "properties": {},
                "ready": true,
                "remote": null,
                "reporter": "alice",
                "repository": null,
                "resolution": null,
                "resolved_sub_issue_count": 0,
                "source_key": null,
                "status": "ready",
                "status_category": "todo",
                "sub_issue_count": 0,
                "sub_issues": [],
                "title": "Write docs",
                "type": "task",
                "updated_at": "<timestamp>",
                "verification": null,
            },
            "meta": { "matched": false }
        })
    );
}

#[test]
fn golden_ready() {
    let (repo, _) = fixture();
    let value = golden(&repo, &["ready", "--json"]);
    assert_eq!(
        value,
        json!({
            "ok": true,
            "data": {
                "count": 1,
                "issues": [{
                    "allowed_transitions": ["claim", "defer", "done", "cancel"],
                    "assignee": null,
                    "assignee_kind": null,
                    "blocking": [],
                    "checkpoint": null,
                    "checkpoint_archived": null,
                    "claimed_at": null,
                    "component": null,
                    "created_at": "<timestamp>",
                    "depth": 0,
                    "eligible_for_close": false,
                    "external_id": null,
                    "id": "<id>",
                    "key": "ORB-1",
                    "key_aliases": [],
                    "labels": ["docs"],
                    "lease_until": null,
                    "links": [],
                    "open_sub_issue_count": 0,
                    "open_sub_issues": [],
                    "parent": null,
                    "priority": "high",
                    "properties": {},
                    "ready": true,
                    "remote": null,
                    "reporter": "alice",
                    "repository": null,
                    "resolution": null,
                    "resolved_sub_issue_count": 0,
                    "source_key": null,
                    "status": "ready",
                    "status_category": "todo",
                    "sub_issue_count": 0,
                    "sub_issues": [],
                    "title": "Write docs",
                    "type": "task",
                    "updated_at": "<timestamp>",
                    "verification": null,
                }]
            },
            "meta": { "count": 1 }
        })
    );
}

#[test]
fn golden_claim() {
    let (repo, id) = fixture();
    let value = golden(&repo, &["claim", &id, "--by", "devin", "--json"]);
    assert_eq!(
        value,
        json!({
            "ok": true,
            "data": {
                "allowed_transitions": ["claim", "release", "submit", "ask", "escalate", "done", "cancel"],
                "assignee": "devin",
                "assignee_kind": "agent",
                "blocking": [],
                "body": "",
                "checkpoint": null,
                "checkpoint_archived": null,
                "claimed_at": "<timestamp>",
                "component": null,
                "created_at": "<timestamp>",
                "dependents": [],
                "depth": 0,
                "eligible_for_close": false,
                "external_id": null,
                "id": "<id>",
                "key": "ORB-1",
                "key_aliases": [],
                "labels": ["docs"],
                "lease_until": "<timestamp>",
                "links": [],
                "open_sub_issue_count": 0,
                "open_sub_issues": [],
                "parent": null,
                "priority": "high",
                "properties": {},
                "ready": false,
                "remote": null,
                "reporter": "alice",
                "repository": null,
                "resolution": null,
                "resolved_sub_issue_count": 0,
                "source_key": null,
                "status": "in_progress",
                "status_category": "active",
                "sub_issue_count": 0,
                "sub_issues": [],
                "title": "Write docs",
                "type": "task",
                "updated_at": "<timestamp>",
                "verification": null,
            }
        })
    );
}

#[test]
fn golden_show() {
    let (repo, id) = fixture();
    repo.ok_json(&["claim", &id, "--by", "devin", "--json"]);
    let value = golden(&repo, &["show", &id, "--json"]);
    assert_eq!(
        value,
        json!({
            "ok": true,
            "data": {
                "allowed_transitions": ["claim", "release", "submit", "ask", "escalate", "done", "cancel"],
                "assignee": "devin",
                "assignee_kind": "agent",
                "blocking": [],
                "body": "",
                "checkpoint": null,
                "checkpoint_archived": null,
                "claimed_at": "<timestamp>",
                "comment_count": 0,
                "comments": [],
                "component": null,
                "created_at": "<timestamp>",
                "dependents": [],
                "depth": 0,
                "eligible_for_close": false,
                "external_id": null,
                "id": "<id>",
                "key": "ORB-1",
                "key_aliases": [],
                "labels": ["docs"],
                "lease_until": "<timestamp>",
                "links": [],
                "open_sub_issue_count": 0,
                "open_sub_issues": [],
                "parent": null,
                "priority": "high",
                "properties": {},
                "ready": false,
                "remote": null,
                "reporter": "alice",
                "repository": null,
                "resolution": null,
                "resolved_sub_issue_count": 0,
                "source_key": null,
                "status": "in_progress",
                "status_category": "active",
                "sub_issue_count": 0,
                "sub_issues": [],
                "title": "Write docs",
                "type": "task",
                "updated_at": "<timestamp>",
                "verification": null,
            }
        })
    );
}

#[test]
fn golden_transition() {
    let (repo, id) = fixture();
    repo.ok_json(&["claim", &id, "--by", "devin", "--json"]);
    let key = repo.key(&id);
    let value = golden(
        &repo,
        &["transition", &key, "done", "--by", "devin", "--json"],
    );
    assert_eq!(
        value,
        json!({
            "ok": true,
            "data": {
                "allowed_transitions": ["reopen"],
                "assignee": null,
                "assignee_kind": null,
                "blocking": [],
                "body": "",
                "checkpoint": null,
                "checkpoint_archived": null,
                "claimed_at": null,
                "component": null,
                "created_at": "<timestamp>",
                "dependents": [],
                "depth": 0,
                "eligible_for_close": false,
                "external_id": null,
                "id": "<id>",
                "key": "ORB-1",
                "key_aliases": [],
                "labels": ["docs"],
                "lease_until": null,
                "links": [],
                "open_sub_issue_count": 0,
                "open_sub_issues": [],
                "parent": null,
                "priority": "high",
                "properties": {},
                "ready": false,
                "remote": null,
                "reporter": "alice",
                "repository": null,
                "resolution": "completed",
                "resolved_sub_issue_count": 0,
                "source_key": null,
                "status": "done",
                "status_category": "done",
                "sub_issue_count": 0,
                "sub_issues": [],
                "title": "Write docs",
                "type": "task",
                "updated_at": "<timestamp>",
                "verification": null,
            }
        })
    );
}

#[test]
fn golden_batch() {
    let repo = TestRepo::new();
    repo.init();
    let (status, batch) = repo.json_with_stdin(
        &["batch", "--file", "-", "--by", "devin", "--json"],
        r#"{"operations":[{"op":"add","key":"k1","title":"Batch task"},{"op":"label","id":"key:k1","add":["bug"]}]}"#,
    );
    assert_eq!(status, 0, "{batch}");
    let value = normalize(&batch);
    assert_eq!(
        value,
        json!({
            "ok": true,
            "data": {
                "committed": true,
                "dry_run": false,
                "keys": { "k1": "<id>" },
                "results": [
                    { "changed": true, "id": "<id>", "index": 0, "matched": false, "op": "add" },
                    { "changed": true, "id": "<id>", "index": 1, "matched": false, "op": "label" }
                ]
            }
        })
    );
}

#[test]
fn every_error_carries_the_problem_fields() {
    let repo = TestRepo::new();
    repo.init();
    let (_, value) = repo.err_json(&["show", "orb-missing", "--json"]);
    let error = &value["error"];
    assert_eq!(error["type"], "orbit://problems/not-found");
    assert_eq!(error["title"], "Not Found");
    assert_eq!(error["code"], "NOT_FOUND");
    assert_eq!(error["detail"], "Issue orb-missing does not exist");
    assert_eq!(error["message"], "Issue orb-missing does not exist");
    assert_eq!(error["retryable"], false);
    assert!(!error["hint"].as_str().unwrap().is_empty());

    // A domain failure with remediation still fills every field.
    let id = repo.add("Blocked", &[]);
    repo.ok_json(&["claim", &id, "--by", "devin", "--json"]);
    let (_, value) = repo.err_json(&["release", &id, "--by", "other", "--json"]);
    let error = &value["error"];
    for field in [
        "type",
        "title",
        "code",
        "detail",
        "message",
        "retryable",
        "hint",
    ] {
        assert!(
            error[field].is_string() || error[field].is_boolean(),
            "{field}: {error}"
        );
    }
    assert!(!error["hint"].as_str().unwrap().is_empty());
}

#[test]
fn capabilities_reports_contract_modules_and_workflow() {
    let repo = TestRepo::new();
    repo.init();
    let value = repo.ok_json(&["capabilities", "--json"]);
    let data = &value["data"];
    assert_eq!(data["contract"], orbit_tracker::contract::VERSION);
    assert_eq!(data["board"]["contract"], orbit_tracker::contract::VERSION);
    assert_eq!(data["board"]["compatible"], true);
    assert_eq!(data["modules"], json!([]));
    let workflow = &data["workflow"];
    assert_eq!(workflow["source"], "template");
    assert_eq!(workflow["template"], "standard");
    assert_eq!(workflow["initial"], "ready");
    assert_eq!(workflow["statuses"].as_array().unwrap().len(), 7);
    assert_eq!(workflow["transitions"].as_array().unwrap().len(), 13);
}

#[test]
fn older_binary_refuses_a_newer_board_but_capabilities_answers() {
    let repo = TestRepo::new();
    repo.init();
    let config_path = repo.board_path().join("config.toml");
    let config = std::fs::read_to_string(&config_path).unwrap();
    std::fs::write(
        &config_path,
        config.replace(
            &format!("contract = \"{}\"", orbit_tracker::contract::VERSION),
            "contract = \"99.0.0\"",
        ),
    )
    .unwrap();

    let (status, value) = repo.err_json(&["list", "--json"]);
    assert_eq!(status, 1);
    assert_eq!(value["error"]["code"], "CONTRACT_VERSION_UNSUPPORTED");
    assert_eq!(
        value["error"]["type"],
        "orbit://problems/contract-version-unsupported"
    );

    // The handshake still answers on the refused Board.
    let value = repo.ok_json(&["capabilities", "--json"]);
    let data = &value["data"];
    assert_eq!(data["board"]["contract"], "99.0.0");
    assert_eq!(data["board"]["compatible"], false);
    assert_eq!(data["contract"], orbit_tracker::contract::VERSION);
    assert_eq!(data["workflow"]["template"], "standard");
}

#[test]
fn an_older_major_board_contract_points_at_the_migration() {
    let repo = TestRepo::new();
    repo.init();
    let config_path = repo.board_path().join("config.toml");
    let config = std::fs::read_to_string(&config_path).unwrap();
    std::fs::write(
        &config_path,
        config.replace(
            &format!("contract = \"{}\"", orbit_tracker::contract::VERSION),
            "contract = \"0.5.0\"",
        ),
    )
    .unwrap();
    let (_, error) = repo.err_json(&["list", "--json"]);
    assert_eq!(error["error"]["code"], "MODEL_NOT_MIGRATED", "{error}");
    let value = repo.ok_json(&["capabilities", "--json"]);
    assert_eq!(value["data"]["board"]["contract"], "0.5.0");
    assert_eq!(value["data"]["board"]["compatible"], false);
}

/// The command registry is the frozen contract surface. Any change to it must
/// ship with a `contract::VERSION` bump: update `EXPECTED` (version and hash)
/// in the same diff, deliberately.
#[test]
fn registry_surface_is_pinned_to_the_contract_version() {
    use sha2::{Digest, Sha256};
    // (contract version, SHA-256 of `orbit schema` minus `orbit_version`)
    const EXPECTED: (&str, &str) = (
        "1.1.0",
        "d30c83412460fe11b3170cc882f23a18ffc0253d44e512bf4263ad2956ea760e",
    );

    let repo = TestRepo::new();
    repo.init();
    let version = orbit_tracker::contract::VERSION;
    let mut schema = repo.ok_json(&["schema", "--json"])["data"].clone();
    schema.as_object_mut().unwrap().remove("orbit_version");
    let digest = Sha256::digest(serde_json::to_string(&schema).unwrap().as_bytes());
    let hash: String = digest.iter().map(|byte| format!("{byte:02x}")).collect();
    assert_eq!(
        (version, hash.as_str()),
        EXPECTED,
        "the command registry or contract version changed: bump contract::VERSION for a surface change, then update EXPECTED"
    );
}

mod common;

use common::TestRepo;
use serde_json::json;
use std::fs;

#[test]
fn labels_and_holds_control_the_ready_queue() {
    let repo = TestRepo::new();
    repo.init();

    let held = repo.add("Held", &["--label", "needs-triage", "--label", "Bug"]);
    let free = repo.add("Free", &[]);

    let show = repo.ok_json(&["show", held.as_str(), "--json"]);
    assert_eq!(show["data"]["labels"], json!(["bug", "needs-triage"]));

    let ready = repo.ok_json(&["ready", "--json"]);
    assert_eq!(ready["data"]["count"], 1);
    assert_eq!(ready["data"]["tasks"][0]["id"], free);

    let (_, error) = repo.err_json(&["claim", held.as_str(), "--by", "agent", "--json"]);
    assert_eq!(error["error"]["code"], "LABEL_BLOCKED");
    assert_eq!(error["error"]["details"]["labels"][0], "needs-triage");

    let (_, error) = repo.err_json(&["claim", held.as_str(), "--by", "agent", "--steal", "--json"]);
    assert_eq!(error["error"]["code"], "LABEL_BLOCKED");

    repo.ok_json(&["label", "remove", held.as_str(), "needs-triage", "--json"]);
    repo.ok_json(&["label", "add", held.as_str(), "ready-for-agent", "--json"]);

    let ready = repo.ok_json(&["ready", "--json"]);
    assert_eq!(ready["data"]["count"], 2);

    let claimed = repo.ok_json(&["claim", held.as_str(), "--by", "agent", "--json"]);
    assert_eq!(claimed["data"]["status"], "in_progress");

    let filtered = repo.ok_json(&["list", "--label", "ready-for-agent", "--json"]);
    assert_eq!(filtered["data"]["count"], 1);
    assert_eq!(filtered["data"]["tasks"][0]["id"], held);

    let unlabeled = repo.ok_json(&["list", "--unlabeled", "--json"]);
    assert_eq!(unlabeled["data"]["count"], 1);
    assert_eq!(unlabeled["data"]["tasks"][0]["id"], free);

    let labels = repo.ok_json(&["label", "list", "--json"]);
    let labels = labels["data"]["labels"].as_array().unwrap();
    assert!(labels.iter().any(|entry| entry["label"] == "bug"));
    assert!(
        labels
            .iter()
            .any(|entry| entry["label"] == "ready-for-agent")
    );
}

#[test]
fn labels_are_idempotent_and_validated() {
    let repo = TestRepo::new();
    repo.init();
    let id = repo.add("Label test", &[]);

    let added = repo.ok_json(&["label", "add", id.as_str(), "bug", "bug", "--json"]);
    assert_eq!(added["data"]["changed"], true);
    assert_eq!(added["data"]["labels"], json!(["bug"]));

    let again = repo.ok_json(&["label", "add", id.as_str(), "bug", "--json"]);
    assert_eq!(again["data"]["changed"], false);

    let missing = repo.ok_json(&["label", "remove", id.as_str(), "missing", "--json"]);
    assert_eq!(missing["data"]["changed"], false);

    let (_, error) = repo.err_json(&["label", "add", id.as_str(), "Has Space", "--json"]);
    assert_eq!(error["error"]["code"], "INVALID_LABEL");

    let (_, error) = repo.err_json(&["add", "Bad label", "--label", "Has Space", "--json"]);
    assert_eq!(error["error"]["code"], "INVALID_LABEL");

    let show = repo.ok_json(&["show", id.as_str(), "--json"]);
    assert_eq!(show["data"]["labels"], json!(["bug"]));
}

#[test]
fn comments_are_append_only_and_appear_in_show() {
    let repo = TestRepo::new();
    repo.init();
    let id = repo.add("Comment test", &[]);

    repo.ok_json(&[
        "comment",
        "add",
        id.as_str(),
        "--by",
        "agent",
        "--body",
        "## Triage Notes\n\n- one\n- two",
        "--json",
    ]);
    repo.ok_json(&[
        "comment",
        "add",
        id.as_str(),
        "--by",
        "reviewer",
        "--body",
        "A reply.",
        "--json",
    ]);

    let listed = repo.ok_json(&["comment", "list", id.as_str(), "--json"]);
    let comments = listed["data"]["comments"].as_array().unwrap();
    assert_eq!(comments.len(), 2);
    assert_eq!(comments[0]["author"], "agent");
    assert_eq!(comments[1]["author"], "reviewer");
    assert!(
        comments[0]["body"]
            .as_str()
            .unwrap()
            .contains("Triage Notes")
    );

    let show = repo.ok_json(&["show", id.as_str(), "--json"]);
    assert_eq!(show["data"]["comment_count"], 2);
    assert_eq!(show["data"]["comments"].as_array().unwrap().len(), 2);

    let human = repo.orbit(&["show", id.as_str()]);
    assert!(human.status.success());
    let stdout = String::from_utf8_lossy(&human.stdout);
    assert!(stdout.contains("reviewer"));
    assert!(stdout.contains("A reply."));

    let (_, error) = repo.err_json(&[
        "comment",
        "add",
        id.as_str(),
        "--by",
        "agent",
        "--body",
        "   ",
        "--json",
    ]);
    assert_eq!(error["error"]["code"], "INVALID_ARGUMENT");

    let (_, error) = repo.err_json(&[
        "comment",
        "add",
        id.as_str(),
        "--by",
        "agent",
        "--body",
        "<!-- /orbit:comment -->",
        "--json",
    ]);
    assert_eq!(error["error"]["code"], "INVALID_ARGUMENT");

    repo.ok_json(&["done", id.as_str(), "--by", "agent", "--json"]);
    let show = repo.ok_json(&["show", id.as_str(), "--json"]);
    assert_eq!(show["data"]["comment_count"], 2);

    let comment_path = repo.board_path().join("comments").join(format!("{id}.md"));
    assert!(comment_path.exists());
    repo.ok_json(&["delete", id.as_str(), "--force", "--json"]);
    assert!(!comment_path.exists());
}

#[test]
fn doctor_reports_corrupt_and_orphan_comments() {
    let repo = TestRepo::new();
    repo.init();
    let id = repo.add("Doctor comment test", &[]);
    let comments_dir = repo.board_path().join("comments");
    fs::create_dir_all(&comments_dir).unwrap();

    fs::write(
        comments_dir.join(format!("{id}.md")),
        "<!-- orbit:comment not-json -->\n",
    )
    .unwrap();
    fs::write(
        comments_dir.join("orb-aaaaaa.md"),
        "<!-- orbit:comment {\"author\":\"x\",\"created_at\":\"2026-10-23T12:00:00Z\"} -->\norphan\n<!-- /orbit:comment -->\n",
    )
    .unwrap();

    let (status, value) = repo.json(&["doctor", "--json"]);
    assert_eq!(status, 1);
    assert_eq!(value["data"]["healthy"], false);
    let issues = value["data"]["issues"].as_array().unwrap();
    assert!(
        issues
            .iter()
            .any(|issue| { issue["code"] == "CORRUPT_COMMENT" && issue["task"] == id.as_str() })
    );
    assert!(issues.iter().any(|issue| issue["code"] == "ORPHAN_COMMENT"));

    let (status, _) = repo.json(&["doctor", "--fix", "--json"]);
    assert_eq!(status, 1);
}

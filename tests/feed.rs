mod common;

use common::TestRepo;
use serde_json::Value;

fn events(value: &Value) -> &[Value] {
    value["data"]["events"]
        .as_array()
        .expect("changes/history returns an events array")
}

fn event<'a>(events: &'a [Value], kind: &str, issue: &str) -> &'a Value {
    events
        .iter()
        .find(|event| event["type"] == kind && event["issue"] == issue)
        .unwrap_or_else(|| panic!("no {kind} event for {issue} in {events:?}"))
}

#[test]
fn changes_lists_events_oldest_first_with_actor_and_trace_id() {
    let repo = TestRepo::new();
    repo.init();
    let first = repo.add("First task", &["--reporter", "carol"]);
    repo.ok_json(&["claim", &first, "--by", "alice", "--json"]);
    repo.ok_json(&[
        "comment", "add", &first, "--body", "a note", "--by", "bob", "--json",
    ]);
    let second = repo.add("Second task", &["--reporter", "dave"]);

    let value = repo.ok_json(&["changes", "--json"]);
    let feed = events(&value);
    assert!(feed.len() >= 5, "expected several events: {feed:?}");

    // Events come oldest first and carry the full envelope.
    let created = event(feed, "issue.created", &first);
    assert_eq!(created["actor"], "carol");
    assert_eq!(created["key"], "ORB-1");
    assert_eq!(created["data"]["title"], "First task");
    let transitioned = event(feed, "issue.transitioned", &first);
    assert_eq!(transitioned["actor"], "alice");
    assert_eq!(transitioned["data"]["status"]["from"], "ready");
    assert_eq!(transitioned["data"]["status"]["to"], "in_progress");
    let commented = event(feed, "issue.commented", &first);
    assert_eq!(commented["actor"], "bob");

    // trace_id is the producing commit; every event has unique id and commit.
    for event in feed {
        assert_eq!(event["trace_id"], event["commit"]);
        assert!(event["id"].as_str().unwrap().contains(':'), "{event}");
        assert!(event["at"].as_str().is_some_and(|at| !at.is_empty()));
        assert!(event["actor"].as_str().is_some_and(|a| !a.is_empty()));
    }
    let mut ids: Vec<&str> = feed.iter().map(|e| e["id"].as_str().unwrap()).collect();
    ids.sort_unstable();
    ids.dedup();
    assert_eq!(ids.len(), feed.len(), "event ids must be unique");

    // One commit's events share a trace id: the comment touched the Task
    // file and the comments sidecar in a single commit.
    assert_eq!(
        commented["trace_id"],
        event(feed, "issue.updated", &first)["trace_id"]
    );

    // The cursor is the Board head commit.
    let head = repo
        .git_in(&repo.board_path(), &["rev-parse", "HEAD"])
        .stdout;
    let head = String::from_utf8(head).unwrap().trim().to_string();
    assert_eq!(value["data"]["cursor"], head);

    // The second Task's events are present too, and later in the feed.
    let created_second = event(feed, "issue.created", &second);
    assert_eq!(created_second["actor"], "dave");
    let first_pos = feed.iter().position(|e| e["id"] == created["id"]).unwrap();
    let second_pos = feed
        .iter()
        .position(|e| e["id"] == created_second["id"])
        .unwrap();
    assert!(first_pos < second_pos, "feed must be oldest first");
}

#[test]
fn changes_since_returns_only_newer_events() {
    let repo = TestRepo::new();
    repo.init();
    let id = repo.add("Cursor task", &[]);

    let first = repo.ok_json(&["changes", "--json"]);
    let cursor = first["data"]["cursor"].as_str().unwrap().to_string();
    let before = events(&first).len();
    assert!(before > 0);

    // Nothing newer: the cursor is the head.
    let same = repo.ok_json(&["changes", "--since", &cursor, "--json"]);
    assert_eq!(same["data"]["events"].as_array().unwrap().len(), 0);
    assert_eq!(same["data"]["cursor"], cursor);

    // A new mutation lands strictly after the cursor.
    repo.ok_json(&["claim", &id, "--by", "alice", "--json"]);
    let delta = repo.ok_json(&["changes", "--since", &cursor, "--json"]);
    let newer = events(&delta);
    assert!(!newer.is_empty());
    assert!(
        newer
            .iter()
            .all(|event| event["issue"] == id || event["issue"].is_null()),
        "only the claimed Task changed: {newer:?}"
    );
    assert!(event(newer, "issue.transitioned", &id)["actor"] == "alice");
    let new_cursor = delta["data"]["cursor"].as_str().unwrap();
    assert_ne!(new_cursor, cursor);

    // And the full feed still contains the older events.
    let full = repo.ok_json(&["changes", "--json"]);
    assert!(events(&full).len() > newer.len());
}

#[test]
fn changes_since_rejects_unknown_and_diverged_cursors() {
    let repo = TestRepo::new();
    repo.init();
    repo.add("Task", &[]);

    let (_, error) = repo.err_json(&["changes", "--since", "deadbeef", "--json"]);
    assert_eq!(error["error"]["code"], "INVALID_ARGUMENT");

    // A commit on a side branch of the Board repo is not an ancestor of HEAD.
    let board = repo.board_path();
    for args in [
        &["checkout", "-q", "-b", "side"][..],
        &["commit", "-q", "--allow-empty", "-m", "orbit: side"],
        &["checkout", "-q", "orbit"],
    ] {
        let output = repo.git_in(&board, args);
        assert!(
            output.status.success(),
            "git {args:?}: {}",
            String::from_utf8_lossy(&output.stderr)
        );
    }
    let side = repo.git_in(&board, &["rev-parse", "side"]).stdout;
    let side = String::from_utf8(side).unwrap().trim().to_string();
    let (_, error) = repo.err_json(&["changes", "--since", &side, "--json"]);
    assert_eq!(error["error"]["code"], "INVALID_ARGUMENT");
    assert!(
        error["error"]["message"]
            .as_str()
            .unwrap()
            .contains("not an ancestor"),
        "{error}"
    );
}

#[test]
fn changes_filters_by_type_and_limit() {
    let repo = TestRepo::new();
    repo.init();
    let id = repo.add("Filtered task", &[]);
    repo.ok_json(&["claim", &id, "--by", "alice", "--json"]);
    repo.ok_json(&[
        "comment", "add", &id, "--body", "note", "--by", "bob", "--json",
    ]);

    let only_comments = repo.ok_json(&["changes", "--type", "issue.commented", "--json"]);
    let feed = events(&only_comments);
    assert!(!feed.is_empty());
    assert!(feed.iter().all(|event| event["type"] == "issue.commented"));

    let limited = repo.ok_json(&["changes", "--limit", "1", "--json"]);
    let feed = events(&limited);
    assert_eq!(feed.len(), 1);
    // The newest event wins: inside the comment commit the Task-file update
    // sorts after the comments-sidecar event.
    assert_eq!(feed[0]["type"], "issue.updated");
}

#[test]
fn history_shows_the_per_issue_timeline() {
    let repo = TestRepo::new();
    repo.init();
    let first = repo.add("Alpha", &[]);
    let second = repo.add("Beta", &[]);
    repo.ok_json(&["claim", &first, "--by", "alice", "--json"]);
    repo.ok_json(&["claim", &second, "--by", "carol", "--json"]);
    repo.ok_json(&[
        "comment",
        "add",
        &first,
        "--body",
        "alpha note",
        "--by",
        "alice",
        "--json",
    ]);

    let value = repo.ok_json(&["history", &first, "--json"]);
    let feed = events(&value);
    let kinds: Vec<&str> = feed
        .iter()
        .map(|event| event["type"].as_str().unwrap())
        .collect();
    assert_eq!(
        kinds,
        vec![
            "issue.created",
            "issue.transitioned",
            "issue.commented",
            // The comment also touched the Task file (updated_at), which the
            // feed reports as a plain update.
            "issue.updated",
        ],
        "unexpected timeline: {feed:?}"
    );
    assert!(feed.iter().all(|event| event["issue"] == first));
    assert_eq!(value["data"]["issue"], "ORB-1");

    // The project key resolves to the same timeline.
    let by_key = repo.ok_json(&["history", "ORB-1", "--json"]);
    assert_eq!(events(&by_key).len(), feed.len());

    // The other Issue's timeline contains none of the first Issue's events.
    let other = repo.ok_json(&["history", &second, "--json"]);
    let other_feed = events(&other);
    assert!(other_feed.iter().all(|event| event["issue"] == second));
    assert_eq!(
        event(other_feed, "issue.transitioned", &second)["actor"],
        "carol"
    );

    // --limit keeps the newest events of the timeline.
    let tail = repo.ok_json(&["history", &first, "--limit", "2", "--json"]);
    let tail_kinds: Vec<&str> = events(&tail)
        .iter()
        .map(|event| event["type"].as_str().unwrap())
        .collect();
    assert_eq!(tail_kinds, vec!["issue.commented", "issue.updated"]);

    // A deleted Issue still has a timeline: history reads Git, not the Board.
    repo.ok_json(&["delete", &second, "--force", "--by", "admin", "--json"]);
    let deleted = repo.ok_json(&["history", &second, "--json"]);
    let deleted_feed = events(&deleted);
    assert_eq!(deleted_feed.last().unwrap()["type"], "issue.deleted");
    assert_eq!(deleted_feed.last().unwrap()["actor"], "admin");
}

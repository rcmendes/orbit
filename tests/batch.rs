mod common;

use common::TestRepo;
use serde_json::{Value, json};

fn board_commits(repo: &TestRepo) -> usize {
    let output = repo.git_in(&repo.board_path(), &["rev-list", "--count", "HEAD"]);
    String::from_utf8_lossy(&output.stdout)
        .trim()
        .parse()
        .unwrap()
}

fn batch(repo: &TestRepo, input: Value, extra: &[&str]) -> (i32, Value) {
    let mut args = vec!["batch", "--file", "-", "--json"];
    args.extend_from_slice(extra);
    repo.json_with_stdin(&args, &input.to_string())
}

fn ok_batch(repo: &TestRepo, input: Value) -> Value {
    let (status, value) = batch(repo, input, &[]);
    assert_eq!(status, 0, "{value}");
    assert_eq!(value["ok"], true, "{value}");
    value
}

fn plan() -> Value {
    json!({"operations": [
        {"op": "add", "key": "planner:plan-7:epic", "title": "Plan 7", "type": "epic"},
        {"op": "add", "key": "planner:plan-7:step-1", "title": "Step 1",
         "parent": "key:planner:plan-7:epic", "labels": ["needs-triage"],
         "annotations": {"planner": {"order": 1}}},
        {"op": "add", "key": "planner:plan-7:step-2", "title": "Step 2",
         "parent": "key:planner:plan-7:epic", "depends_on": ["key:planner:plan-7:step-1"]}
    ]})
}

fn task_files(repo: &TestRepo) -> Vec<String> {
    let mut names: Vec<String> = std::fs::read_dir(repo.board_path().join("tasks"))
        .map(|entries| {
            entries
                .map(|entry| std::fs::read_to_string(entry.unwrap().path()).unwrap())
                .collect()
        })
        .unwrap_or_default();
    names.sort();
    names
}

#[test]
fn batch_creates_parent_subtasks_and_dependencies_in_one_commit() {
    let repo = TestRepo::new();
    repo.init();
    let before = board_commits(&repo);
    let value = ok_batch(&repo, plan());
    assert_eq!(board_commits(&repo), before + 1);
    assert_eq!(value["data"]["committed"], true);
    let keys = &value["data"]["keys"];
    assert_eq!(keys.as_object().unwrap().len(), 3);
    let step1 = keys["planner:plan-7:step-1"].as_str().unwrap();
    let step2 = keys["planner:plan-7:step-2"].as_str().unwrap();
    let show = repo.ok_json(&["show", step2, "--json"]);
    assert_eq!(show["data"]["depends_on"], json!([step1]));
    let step1_show = repo.ok_json(&["show", step1, "--json"]);
    assert_eq!(
        step1_show["data"]["annotations"],
        json!({"planner": {"order": 1}})
    );
    assert_eq!(step1_show["data"]["source_key"], "planner:plan-7:step-1");
    assert_eq!(value["data"]["results"].as_array().unwrap().len(), 3);
    assert_eq!(value["data"]["results"][0]["op"], "add");
    assert_eq!(value["data"]["results"][0]["changed"], true);
}

#[test]
fn batch_resolves_keys_created_earlier_in_the_batch() {
    let repo = TestRepo::new();
    repo.init();
    let value = ok_batch(&repo, plan());
    let epic = value["data"]["keys"]["planner:plan-7:epic"]
        .as_str()
        .unwrap();
    let step1 = value["data"]["keys"]["planner:plan-7:step-1"]
        .as_str()
        .unwrap();
    let show = repo.ok_json(&["show", step1, "--json"]);
    assert_eq!(show["data"]["parent"], epic);
}

#[test]
fn batch_failure_writes_nothing() {
    let repo = TestRepo::new();
    repo.init();
    let existing = repo.add("Existing", &[]);
    let before_commits = board_commits(&repo);
    let before_files = task_files(&repo);
    let (status, value) = batch(
        &repo,
        json!({"operations": [
            {"op": "add", "key": "planner:x:1", "title": "New"},
            {"op": "label", "id": existing, "add": ["bug"]},
            {"op": "depend", "id": existing, "on": "orb-missing"}
        ]}),
        &[],
    );
    assert_ne!(status, 0);
    assert_eq!(value["error"]["code"], "NOT_FOUND", "{value}");
    assert_eq!(value["error"]["details"]["index"], 2);
    assert_eq!(value["error"]["details"]["op"], "depend");
    assert_eq!(board_commits(&repo), before_commits);
    assert_eq!(task_files(&repo), before_files);
}

#[test]
fn batch_dry_run_validates_without_writing() {
    let repo = TestRepo::new();
    repo.init();
    let before = board_commits(&repo);
    let files = task_files(&repo);
    let (status, value) = batch(&repo, plan(), &["--dry-run"]);
    assert_eq!(status, 0, "{value}");
    assert_eq!(value["data"]["dry_run"], true);
    assert_eq!(value["data"]["committed"], false);
    assert_eq!(value["data"]["results"].as_array().unwrap().len(), 3);
    assert_eq!(board_commits(&repo), before);
    assert_eq!(task_files(&repo), files);
}

#[test]
fn batch_remove_hold_label_and_claim_together() {
    let repo = TestRepo::new();
    repo.init();
    let held = repo.add("Held", &["--label", "needs-triage"]);
    let ready = repo.ok_json(&["ready", "--json"]);
    assert_eq!(ready["data"]["count"], 0);
    ok_batch(
        &repo,
        json!({"operations": [
            {"op": "label", "id": held, "remove": ["needs-triage"]},
            {"op": "claim", "id": held, "by": "planner-7"}
        ]}),
    );
    let show = repo.ok_json(&["show", &held, "--json"]);
    assert_eq!(show["data"]["claimed_by"], "planner-7");
    assert_eq!(show["data"]["status"], "in_progress");
    let ready = repo.ok_json(&["ready", "--json"]);
    assert_eq!(ready["data"]["count"], 0);
}

#[test]
fn batch_rerun_after_partial_apply_converges() {
    let repo = TestRepo::new();
    repo.init();
    // A crash partway through: only the epic exists, carrying its Source Key.
    let epic = repo.add(
        "Plan 7",
        &["--type", "epic", "--source-key", "planner:plan-7:epic"],
    );
    let input = json!({"operations": [
        {"op": "add", "key": "planner:plan-7:epic", "title": "Plan 7", "type": "epic"},
        {"op": "add", "key": "planner:plan-7:step-1", "title": "Step 1",
         "parent": "key:planner:plan-7:epic", "labels": ["needs-triage"]},
        {"op": "label", "id": "key:planner:plan-7:step-1", "remove": ["needs-triage"]},
        {"op": "claim", "id": "key:planner:plan-7:step-1", "by": "planner-7"},
        {"op": "annotate", "id": "key:planner:plan-7:step-1", "owner": "planner", "value": {"order": 1}}
    ]});
    let first = ok_batch(&repo, input.clone());
    assert_eq!(first["data"]["keys"]["planner:plan-7:epic"], epic);
    assert_eq!(first["data"]["results"][0]["matched"], true);
    let after_first = task_files(&repo);

    let second = ok_batch(&repo, input);
    assert_eq!(second["data"]["keys"], first["data"]["keys"]);
    let list = repo.ok_json(&["list", "--json"]);
    assert_eq!(list["data"]["count"], 2);
    let step = first["data"]["keys"]["planner:plan-7:step-1"]
        .as_str()
        .unwrap();
    let show = repo.ok_json(&["show", step, "--json"]);
    assert_eq!(show["data"]["claimed_by"], "planner-7");
    assert_eq!(show["data"]["labels"], json!([]));
    assert_eq!(
        show["data"]["annotations"],
        json!({"planner": {"order": 1}})
    );
    assert_eq!(task_files(&repo).len(), after_first.len());
}

#[test]
fn batch_rejects_unknown_fields() {
    let repo = TestRepo::new();
    repo.init();
    let (status, value) = batch(
        &repo,
        json!({"operations": [{"op": "add", "titel": "x"}]}),
        &[],
    );
    assert_ne!(status, 0);
    assert_eq!(value["error"]["code"], "INVALID_BATCH", "{value}");
}

#[test]
fn batch_heartbeat_only_does_not_commit() {
    let repo = TestRepo::new();
    repo.init();
    let id = repo.add("Work", &[]);
    repo.ok_json(&["claim", &id, "--by", "a", "--json"]);
    let before = board_commits(&repo);
    let value = ok_batch(
        &repo,
        json!({"operations": [{"op": "heartbeat", "id": id, "by": "a"}]}),
    );
    assert_eq!(value["data"]["committed"], false);
    assert_eq!(board_commits(&repo), before);
}

#[test]
fn batch_comment_and_done_with_comment() {
    let repo = TestRepo::new();
    repo.init();
    let id = repo.add("Work", &[]);
    ok_batch(
        &repo,
        json!({"operations": [
            {"op": "comment", "id": id, "by": "a", "body": "Started."},
            {"op": "claim", "id": id, "by": "a"},
            {"op": "done", "id": id, "by": "a", "comment": "Finished."}
        ]}),
    );
    let show = repo.ok_json(&["show", &id, "--json"]);
    assert_eq!(show["data"]["status"], "done");
    let bodies: Vec<&str> = show["data"]["comments"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|comment| comment["body"].as_str())
        .collect();
    assert_eq!(bodies, ["Started.", "Finished."]);
}

#[test]
fn batch_add_rejects_an_invalid_repo() {
    let repo = TestRepo::new();
    repo.init();
    let (status, value) = batch(
        &repo,
        json!({"operations": [{"op": "add", "title": "R", "repo": "Bad Repo!"}]}),
        &[],
    );
    assert_ne!(status, 0);
    assert_eq!(value["error"]["code"], "INVALID_ARGUMENT", "{value}");
    assert_eq!(value["error"]["details"]["index"], 0);
    repo.ok_json(&["list", "--json"]);
}

#[test]
fn batch_rerun_after_crash_before_commit_commits() {
    let repo = TestRepo::new();
    repo.init();
    ok_batch(&repo, plan());
    // Crash after the files were written but before the commit.
    let board = repo.board_path();
    repo.git_in(&board, &["reset", "--soft", "HEAD~1"]);
    repo.git_in(&board, &["reset", "-q"]);
    let before = board_commits(&repo);
    let value = ok_batch(&repo, plan());
    assert_eq!(value["data"]["committed"], true, "{value}");
    assert_eq!(board_commits(&repo), before + 1);
    let status = repo.git_in(&board, &["status", "--porcelain"]);
    assert_eq!(String::from_utf8_lossy(&status.stdout).trim(), "");
}

#[test]
fn batch_reopen_of_an_open_task_is_unchanged() {
    let repo = TestRepo::new();
    repo.init();
    let id = repo.add("Work", &[]);
    let value = ok_batch(&repo, json!({"operations": [{"op": "reopen", "id": id}]}));
    assert_eq!(value["data"]["results"][0]["changed"], false);
}

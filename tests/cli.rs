mod common;

use common::TestRepo;
use serde_json::Value;
use std::collections::HashSet;
use std::process::Stdio;

#[test]
fn init_creates_the_local_orbit_folder() {
    let repo = TestRepo::new();
    let value = repo.ok_json(&["init", "--json"]);

    assert_eq!(value["data"]["branch"], "orbit");
    assert_eq!(value["data"]["storage"], "worktree");

    let board = repo.board_path();
    assert!(board.join("config.toml").is_file());
    assert!(board.join("tasks").is_dir());

    let worktrees = repo.git(&["worktree", "list", "--porcelain"]);
    let worktrees = String::from_utf8_lossy(&worktrees.stdout);
    assert!(worktrees.contains("orbit"), "{worktrees}");

    let log = repo.git(&["-C", board.to_str().unwrap(), "log", "--oneline"]);
    let log = String::from_utf8_lossy(&log.stdout);
    assert!(log.contains("orbit: initialize board"), "{log}");

    let second = repo.ok_json(&["init", "--json"]);
    assert_eq!(second["data"]["created"], false);
}

#[test]
fn add_list_show_and_ready_round_trip() {
    let repo = TestRepo::new();
    repo.init();

    let first = repo.add("First", &["-p", "0", "--body", "Body A"]);
    let second = repo.add(
        "Second",
        &[
            "-p",
            "2",
            "--depends-on",
            first.as_str(),
            "--body",
            "Body B",
        ],
    );

    let list = repo.ok_json(&["list", "--json"]);
    assert_eq!(list["data"]["count"], 2);
    assert_eq!(list["meta"]["count"], 2);

    let ready = repo.ok_json(&["ready", "--json"]);
    assert_eq!(ready["data"]["count"], 1);
    assert_eq!(ready["data"]["tasks"][0]["id"], first);

    let show = repo.ok_json(&["show", second.as_str(), "--json"]);
    assert_eq!(show["data"]["title"], "Second");
    assert_eq!(show["data"]["body"], "Body B");
    assert_eq!(show["data"]["depends_on"][0], first);
    assert_eq!(show["data"]["blocking"][0], first);
    assert_eq!(show["data"]["ready"], false);

    let done = repo.ok_json(&["done", first.as_str(), "--by", "agent", "--json"]);
    assert_eq!(done["data"]["status"], "done");
    let ready = repo.ok_json(&["ready", "--json"]);
    assert_eq!(ready["data"]["count"], 1);
    assert_eq!(ready["data"]["tasks"][0]["id"], second);
}

#[test]
fn done_with_comment_closes_and_records_the_note_in_one_step() {
    let repo = TestRepo::new();
    repo.init();
    let id = repo.add("Close with note", &[]);
    repo.ok_json(&["claim", id.as_str(), "--by", "agent", "--json"]);

    let done = repo.ok_json(&[
        "done",
        id.as_str(),
        "--by",
        "agent",
        "--comment",
        "Implemented and verified.",
        "--json",
    ]);
    assert_eq!(done["data"]["status"], "done");
    assert_eq!(done["data"]["claimed_by"], Value::Null);
    assert_eq!(done["data"]["comment_count"], 1);
    assert_eq!(done["data"]["comments"][0]["author"], "agent");
    assert_eq!(
        done["data"]["comments"][0]["body"],
        "Implemented and verified."
    );

    let content = repo.read_task(&id);
    assert!(content.contains("status: done"), "{content}");
    assert!(!content.contains("status: in_progress"), "{content}");

    let show = repo.ok_json(&["show", id.as_str(), "--json"]);
    assert_eq!(show["data"]["status"], "done");
    assert_eq!(show["data"]["comment_count"], 1);

    let board = repo.board_path();
    let log = repo.git(&["-C", board.to_str().unwrap(), "log", "--oneline", "-5"]);
    let log = String::from_utf8_lossy(&log.stdout);
    assert!(log.contains("orbit: done"), "{log}");
    assert!(
        !log.contains("orbit: comment"),
        "the status and note must share one commit: {log}"
    );
}

#[test]
fn done_validates_the_comment_before_changing_status() {
    let repo = TestRepo::new();
    repo.init();
    let id = repo.add("Reject empty note", &[]);
    repo.ok_json(&["claim", id.as_str(), "--by", "agent", "--json"]);

    let (_, error) = repo.err_json(&[
        "done",
        id.as_str(),
        "--by",
        "agent",
        "--comment",
        "   ",
        "--json",
    ]);
    assert_eq!(error["error"]["code"], "INVALID_ARGUMENT");

    let show = repo.ok_json(&["show", id.as_str(), "--json"]);
    assert_eq!(show["data"]["status"], "in_progress");
    assert_eq!(show["data"]["comment_count"], 0);
}

#[test]
fn done_reads_the_completion_note_from_stdin() {
    let repo = TestRepo::new();
    repo.init();
    let id = repo.add("Close from stdin", &[]);

    let (status, done) = repo.json_with_stdin(
        &[
            "done",
            id.as_str(),
            "--by",
            "agent",
            "--comment-file",
            "-",
            "--json",
        ],
        "Closed from a piped note.\n",
    );
    assert_eq!(status, 0, "{done}");
    assert_eq!(done["data"]["status"], "done");
    assert_eq!(done["data"]["comment_count"], 1);
    assert_eq!(
        done["data"]["comments"][0]["body"],
        "Closed from a piped note."
    );
}

#[test]
fn cancel_with_comment_records_the_reason_with_the_status_change() {
    let repo = TestRepo::new();
    repo.init();
    let id = repo.add("Cancel with note", &[]);

    let cancelled = repo.ok_json(&[
        "cancel",
        id.as_str(),
        "--by",
        "agent",
        "--comment",
        "Duplicate of orb-aaaaaa.",
        "--json",
    ]);
    assert_eq!(cancelled["data"]["status"], "cancelled");
    assert_eq!(cancelled["data"]["comment_count"], 1);
    assert_eq!(
        cancelled["data"]["comments"][0]["body"],
        "Duplicate of orb-aaaaaa."
    );

    let show = repo.ok_json(&["show", id.as_str(), "--json"]);
    assert_eq!(show["data"]["status"], "cancelled");
    assert_eq!(show["data"]["comment_count"], 1);
}

#[test]
fn dependencies_block_completion_and_cycles_are_rejected() {
    let repo = TestRepo::new();
    repo.init();

    let first = repo.add("First", &[]);
    let second = repo.add("Second", &["--depends-on", first.as_str()]);

    let (_, error) = repo.err_json(&["done", second.as_str(), "--by", "agent", "--json"]);
    assert_eq!(error["error"]["code"], "DEPENDENCY_BLOCKED");
    assert_eq!(error["error"]["details"]["blocking"][0], first);

    let (_, error) = repo.err_json(&["depend", first.as_str(), second.as_str(), "--json"]);
    assert_eq!(error["error"]["code"], "CYCLE_DETECTED");
    assert_eq!(error["error"]["details"]["cycle"][0], first);

    repo.ok_json(&["done", first.as_str(), "--by", "agent", "--json"]);
    repo.ok_json(&["done", second.as_str(), "--by", "agent", "--json"]);
}

#[test]
fn claims_have_leases_and_holder_checks() {
    let repo = TestRepo::new();
    repo.init();
    let id = repo.add("Claim me", &[]);

    let claimed = repo.ok_json(&["claim", id.as_str(), "--by", "agent-1", "--json"]);
    assert_eq!(claimed["data"]["status"], "in_progress");
    assert_eq!(claimed["data"]["claimed_by"], "agent-1");
    assert!(claimed["data"]["lease_until"].is_string());

    let (_, error) = repo.err_json(&["claim", id.as_str(), "--by", "agent-2", "--json"]);
    assert_eq!(error["error"]["code"], "ALREADY_CLAIMED");
    assert_eq!(error["error"]["details"]["holder"], "agent-1");

    let (_, error) = repo.err_json(&["heartbeat", id.as_str(), "--by", "agent-2", "--json"]);
    assert_eq!(error["error"]["code"], "NOT_CLAIM_HOLDER");

    let heartbeat = repo.ok_json(&["heartbeat", id.as_str(), "--by", "agent-1", "--json"]);
    assert_eq!(heartbeat["meta"]["committed"], false);

    let released = repo.ok_json(&["release", id.as_str(), "--by", "agent-1", "--json"]);
    assert_eq!(released["data"]["status"], "todo");
    assert!(released["data"]["claimed_by"].is_null());

    let ready = repo.ok_json(&["ready", "--json"]);
    assert_eq!(ready["data"]["count"], 1);
}

#[test]
fn stale_claims_are_visible_and_stealable() {
    let repo = TestRepo::new();
    repo.init();
    let id = repo.add("Stale", &[]);

    repo.ok_json(&["claim", id.as_str(), "--by", "agent-1", "--json"]);
    repo.make_stale(&id);

    let (_, error) = repo.err_json(&["claim", id.as_str(), "--by", "agent-2", "--json"]);
    assert_eq!(error["error"]["code"], "CLAIM_STALE");
    assert_eq!(error["error"]["details"]["holder"], "agent-1");

    let stolen = repo.ok_json(&["claim", id.as_str(), "--by", "agent-2", "--steal", "--json"]);
    assert_eq!(stolen["data"]["claimed_by"], "agent-2");
    assert_eq!(stolen["data"]["status"], "in_progress");
}

#[test]
fn waiting_tasks_are_not_ready() {
    let repo = TestRepo::new();
    repo.init();
    let id = repo.add("Blocked externally", &["--waiting-on", "API credentials"]);

    let ready = repo.ok_json(&["ready", "--json"]);
    assert_eq!(ready["data"]["count"], 0);

    let (_, error) = repo.err_json(&["claim", id.as_str(), "--by", "agent", "--json"]);
    assert_eq!(error["error"]["code"], "WAITING_EXTERNAL");

    let waiting = repo.ok_json(&["list", "--waiting", "--json"]);
    assert_eq!(waiting["data"]["count"], 1);

    repo.ok_json(&["update", id.as_str(), "--clear-waiting", "--json"]);
    let ready = repo.ok_json(&["ready", "--json"]);
    assert_eq!(ready["data"]["count"], 1);
}

#[test]
fn delete_refuses_dependents_and_force_cleans_them() {
    let repo = TestRepo::new();
    repo.init();
    let first = repo.add("First", &[]);
    let second = repo.add("Second", &["--depends-on", first.as_str()]);

    let (_, error) = repo.err_json(&["delete", first.as_str(), "--json"]);
    assert_eq!(error["error"]["code"], "DEPENDENCY_BLOCKED");
    assert_eq!(error["error"]["details"]["dependents"][0], second);

    let deleted = repo.ok_json(&["delete", first.as_str(), "--force", "--json"]);
    assert_eq!(deleted["data"]["cleaned"][0], second);

    let (_, error) = repo.err_json(&["show", first.as_str(), "--json"]);
    assert_eq!(error["error"]["code"], "NOT_FOUND");

    let show = repo.ok_json(&["show", second.as_str(), "--json"]);
    assert!(show["data"]["depends_on"].as_array().unwrap().is_empty());
}

#[test]
fn cancel_resolves_dependents_and_reopen_restores_the_block() {
    let repo = TestRepo::new();
    repo.init();
    let first = repo.add("First", &[]);
    let second = repo.add("Second", &["--depends-on", first.as_str()]);

    repo.ok_json(&["cancel", first.as_str(), "--by", "agent", "--json"]);
    let ready = repo.ok_json(&["ready", "--json"]);
    assert_eq!(ready["data"]["count"], 1);
    assert_eq!(ready["data"]["tasks"][0]["id"], second);

    repo.ok_json(&["reopen", first.as_str(), "--json"]);
    let (_, error) = repo.err_json(&["done", second.as_str(), "--by", "agent", "--json"]);
    assert_eq!(error["error"]["code"], "DEPENDENCY_BLOCKED");

    repo.ok_json(&["done", first.as_str(), "--by", "agent", "--json"]);
    repo.ok_json(&["done", second.as_str(), "--by", "agent", "--json"]);

    let (_, error) = repo.err_json(&["reopen", first.as_str(), "--json"]);
    assert_eq!(error["error"]["code"], "INVALID_TRANSITION");
    assert_eq!(error["error"]["details"]["done_dependents"][0], second);

    repo.ok_json(&["reopen", first.as_str(), "--force", "--json"]);

    let list = repo.ok_json(&["list", "--json"]);
    assert_eq!(list["data"]["count"], 1);

    let doctor = repo.ok_json(&["doctor", "--json"]);
    assert_eq!(doctor["data"]["healthy"], true);
    assert!(
        doctor["data"]["warnings"]
            .as_array()
            .unwrap()
            .iter()
            .any(|warning| warning
                .as_str()
                .unwrap()
                .contains("done but has unfinished dependencies"))
    );
}

#[test]
fn list_filters_are_applied() {
    let repo = TestRepo::new();
    repo.init();
    let first = repo.add("First", &["-p", "0"]);
    let second = repo.add("Second", &["-p", "0", "--waiting-on", "human"]);
    repo.add("Third", &["-p", "1"]);

    let by_priority = repo.ok_json(&["list", "--priority", "0", "--json"]);
    assert_eq!(by_priority["data"]["count"], 2);

    let waiting = repo.ok_json(&["list", "--waiting", "--json"]);
    assert_eq!(waiting["data"]["count"], 1);
    assert_eq!(waiting["data"]["tasks"][0]["id"], second);

    repo.ok_json(&["claim", first.as_str(), "--by", "agent-1", "--json"]);
    let claimed = repo.ok_json(&["list", "--claimed-by", "agent-1", "--json"]);
    assert_eq!(claimed["data"]["count"], 1);

    repo.ok_json(&["done", first.as_str(), "--by", "agent-1", "--json"]);
    let active = repo.ok_json(&["list", "--json"]);
    assert_eq!(active["data"]["count"], 2);
    let done = repo.ok_json(&["list", "--status", "done", "--json"]);
    assert_eq!(done["data"]["count"], 1);
    let all = repo.ok_json(&["list", "--all", "--json"]);
    assert_eq!(all["data"]["count"], 3);
}

#[test]
fn update_and_add_validate_their_inputs() {
    let repo = TestRepo::new();
    repo.init();

    let (_, error) = repo.err_json(&["add", "Bad", "-p", "9", "--json"]);
    assert_eq!(error["error"]["code"], "INVALID_ARGUMENT");
    let (_, error) = repo.err_json(&["add", "", "--json"]);
    assert_eq!(error["error"]["code"], "INVALID_ARGUMENT");
    let (_, error) = repo.err_json(&["add", "Bad", "--depends-on", "orb-aaaaaa", "--json"]);
    assert_eq!(error["error"]["code"], "NOT_FOUND");

    let id = repo.add("Original", &["-p", "2", "--body", "Old body"]);
    repo.ok_json(&[
        "update",
        id.as_str(),
        "--title",
        "Changed",
        "--priority",
        "0",
        "--body",
        "New body",
        "--waiting-on",
        "human",
        "--json",
    ]);
    let show = repo.ok_json(&["show", id.as_str(), "--json"]);
    assert_eq!(show["data"]["title"], "Changed");
    assert_eq!(show["data"]["priority"], 0);
    assert_eq!(show["data"]["body"], "New body");
    assert_eq!(show["data"]["waiting_on"], "human");

    let (_, error) = repo.err_json(&["update", id.as_str(), "--json"]);
    assert_eq!(error["error"]["code"], "INVALID_ARGUMENT");
}

#[test]
fn sync_commits_heartbeat_changes() {
    let repo = TestRepo::new();
    repo.init();
    let id = repo.add("Heartbeat only", &[]);

    repo.ok_json(&["claim", id.as_str(), "--by", "agent-1", "--json"]);
    repo.ok_json(&["heartbeat", id.as_str(), "--by", "agent-1", "--json"]);

    let doctor = repo.ok_json(&["doctor", "--json"]);
    assert!(
        doctor["data"]["warnings"]
            .as_array()
            .unwrap()
            .iter()
            .any(|warning| warning.as_str().unwrap().contains("uncommitted"))
    );

    let sync = repo.ok_json(&["sync", "--json"]);
    assert_eq!(sync["data"]["committed"], true);

    let doctor = repo.ok_json(&["doctor", "--json"]);
    assert!(doctor["data"]["warnings"].as_array().unwrap().is_empty());
}

#[test]
#[cfg(debug_assertions)]
fn crash_between_temp_and_rename_preserves_the_old_file() {
    let repo = TestRepo::new();
    repo.init();
    let id = repo.add("Crash test", &[]);

    let output = repo.orbit_with_env(
        &["update", id.as_str(), "--title", "New title", "--json"],
        &[("ORBIT_TEST_CRASH_AFTER_TEMP_WRITE", "1")],
    );
    assert!(!output.status.success());

    let show = repo.ok_json(&["show", id.as_str(), "--json"]);
    assert_eq!(show["data"]["title"], "Crash test");

    let doctor = repo.ok_json(&["doctor", "--json"]);
    assert_eq!(doctor["data"]["healthy"], true);
    let warnings = doctor["data"]["warnings"].as_array().unwrap();
    assert!(
        warnings
            .iter()
            .any(|warning| warning.as_str().unwrap().contains("stale temp"))
    );

    let fixed = repo.ok_json(&["doctor", "--fix", "--json"]);
    assert_eq!(fixed["data"]["fixed"].as_array().unwrap().len(), 1);

    let doctor = repo.ok_json(&["doctor", "--json"]);
    assert!(doctor["data"]["warnings"].as_array().unwrap().is_empty());
}

#[test]
fn linked_worktrees_share_one_board() {
    let repo = TestRepo::new();
    repo.init();
    let id = repo.add("Shared task", &[]);

    let worktree_dir = tempfile::tempdir().unwrap();
    let worktree = worktree_dir.path().join("wt");
    repo.add_worktree(&worktree, "feature");

    let output = repo.orbit_in(&worktree, &["list", "--json"]);
    assert!(output.status.success());
    let list: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(list["data"]["count"], 1);

    let output = repo.orbit_in(
        &worktree,
        &["claim", id.as_str(), "--by", "worktree-agent", "--json"],
    );
    assert!(output.status.success());

    let show = repo.ok_json(&["show", id.as_str(), "--json"]);
    assert_eq!(show["data"]["status"], "in_progress");
    assert_eq!(show["data"]["claimed_by"], "worktree-agent");

    repo.git(&["worktree", "remove", "--force", worktree.to_str().unwrap()]);
}

#[test]
fn external_boards_work_outside_a_repository() {
    let repo = TestRepo::new();
    let external = tempfile::tempdir().unwrap();
    let workspace = external.path().join("workspace");
    let board = workspace.join(".orbit/board");
    let workspace_arg = workspace.to_str().unwrap();

    let output = repo.orbit(&["--dir", workspace_arg, "init", "--json"]);
    assert!(output.status.success());
    let init: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(init["data"]["storage"], "repository");
    assert_eq!(init["data"]["global"], false);
    assert!(board.join("config.toml").is_file());

    let output = repo.orbit(&["--dir", workspace_arg, "add", "External task", "--json"]);
    assert!(output.status.success());

    let output = repo.orbit(&["--dir", workspace_arg, "list", "--json"]);
    assert!(output.status.success());
    let list: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(list["data"]["count"], 1);
}

#[test]
fn doctor_reports_dangling_dependencies() {
    let repo = TestRepo::new();
    repo.init();
    let first = repo.add("First", &[]);
    let second = repo.add("Second", &["--depends-on", first.as_str()]);

    std::fs::remove_file(repo.task_path(&first)).unwrap();

    let (status, value) = repo.json(&["doctor", "--json"]);
    assert_eq!(status, 1);
    assert_eq!(value["ok"], true);
    assert_eq!(value["data"]["healthy"], false);

    let issues = value["data"]["issues"].as_array().unwrap();
    assert!(
        issues
            .iter()
            .any(|issue| { issue["code"] == "DANGLING_DEP" && issue["task"] == second.as_str() })
    );
}

#[test]
fn json_errors_have_a_stable_envelope() {
    let repo = TestRepo::new();
    repo.init();

    let (status, error) = repo.err_json(&["show", "orb-aaaaaa", "--json"]);
    assert_eq!(status, 1);
    assert_eq!(error["ok"], false);
    assert_eq!(error["error"]["code"], "NOT_FOUND");
    assert_eq!(error["error"]["retryable"], false);
    assert_eq!(error["error"]["details"]["task"], "orb-aaaaaa");

    let no_ready = repo.ok_json(&["ready", "--claim", "--by", "agent", "--json"]);
    assert!(no_ready["data"].is_null());
    assert_eq!(no_ready["meta"]["reason"], "NO_READY_TASKS");
}

#[test]
fn human_output_is_available_without_json() {
    let repo = TestRepo::new();
    repo.init();

    let output = repo.orbit(&["add", "Human task"]);
    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("created orb-"), "{stdout}");

    let output = repo.orbit(&["ready"]);
    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("Human task"), "{stdout}");
}

#[test]
fn concurrent_ids_never_collide() {
    let repo = TestRepo::new();
    repo.init();

    // Windows CI can take longer than the default 2s lock timeout while a
    // competing process is committing its task.
    let config_path = repo.board_path().join("config.toml");
    let config = std::fs::read_to_string(&config_path).unwrap();
    std::fs::write(
        &config_path,
        config.replace("lock_timeout_ms = 2000", "lock_timeout_ms = 60000"),
    )
    .unwrap();

    let mut children = Vec::new();
    for index in 0..12 {
        let mut command = repo.orbit_command(&["add", &format!("Task {index}"), "--json"]);
        command.stdout(Stdio::piped()).stderr(Stdio::piped());
        children.push(command.spawn().unwrap());
    }

    let mut ids = HashSet::new();
    for child in children {
        let output = child.wait_with_output().unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        let value: Value = serde_json::from_slice(&output.stdout).unwrap();
        let id = value["data"]["id"].as_str().unwrap().to_string();
        assert!(ids.insert(id), "duplicate id");
    }
    assert_eq!(ids.len(), 12);
}

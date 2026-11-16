mod common;

use common::TestRepo;
use serde_json::{Value, json};
use std::collections::HashSet;
use std::process::Stdio;

#[test]
fn concurrent_ready_claims_are_distinct() {
    let repo = TestRepo::new();
    repo.init();
    // Eight contending processes can exceed the default 2s lock wait on a
    // slow CI runner. This test is about distinct claims, not timeout tuning.
    repo.set_lock_timeout(15_000);

    for index in 0..8 {
        repo.add(&format!("Task {index}"), &["-p", "high"]);
    }

    let mut children = Vec::new();
    for index in 0..8 {
        let mut command = repo.orbit_command(&[
            "ready",
            "--claim",
            "--by",
            &format!("agent-{index}"),
            "--json",
        ]);
        command.stdout(Stdio::piped()).stderr(Stdio::piped());
        children.push(command.spawn().unwrap());
    }

    let mut claimed = HashSet::new();
    let mut holders = HashSet::new();
    for child in children {
        let output = child.wait_with_output().unwrap();
        assert!(
            output.status.success(),
            "ready --claim failed\nstdout: {}\nstderr: {}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        let value: Value = serde_json::from_slice(&output.stdout).unwrap();
        assert_eq!(value["ok"], true, "{value}");
        let id = value["data"]["id"].as_str().unwrap().to_string();
        let holder = value["data"]["assignee"].as_str().unwrap().to_string();
        assert!(claimed.insert(id), "the same task was claimed twice");
        assert!(holders.insert(holder), "two claims had the same holder");
    }

    assert_eq!(claimed.len(), 8);
    assert_eq!(holders.len(), 8);

    let all = repo.ok_json(&["list", "--all", "--json"]);
    assert_eq!(all["data"]["count"], 8);
    for task in all["data"]["tasks"].as_array().unwrap() {
        assert_eq!(task["status"], "in_progress");
        assert!(task["assignee"].is_string());
    }

    let ready = repo.ok_json(&["ready", "--json"]);
    assert_eq!(ready["data"]["count"], 0);
}

#[test]
fn concurrent_reverse_dependencies_cannot_both_win() {
    let repo = TestRepo::new();
    repo.init();

    let first = repo.add("First", &[]);
    let second = repo.add("Second", &[]);

    let mut children = Vec::new();
    for (task, dependency) in [(&first, &second), (&second, &first)] {
        let mut command =
            repo.orbit_command(&["depend", task.as_str(), dependency.as_str(), "--json"]);
        command.stdout(Stdio::piped()).stderr(Stdio::piped());
        children.push(command.spawn().unwrap());
    }

    let mut succeeded = 0;
    let mut cycles = 0;
    for child in children {
        let output = child.wait_with_output().unwrap();
        let value: Value = serde_json::from_slice(&output.stdout).unwrap();
        if output.status.success() {
            assert_eq!(value["ok"], true, "{value}");
            succeeded += 1;
        } else {
            assert_eq!(value["error"]["code"], "CYCLE_DETECTED", "{value}");
            cycles += 1;
        }
    }

    assert_eq!(succeeded, 1);
    assert_eq!(cycles, 1);

    let doctor = repo.ok_json(&["doctor", "--json"]);
    assert_eq!(doctor["data"]["healthy"], true);
}

#[test]
fn concurrent_deletes_leave_a_healthy_board() {
    let repo = TestRepo::new();
    repo.init();

    let first = repo.add("First", &[]);
    let second = repo.add("Second", &["--depends-on", first.as_str()]);
    let third = repo.add("Third", &["--depends-on", first.as_str()]);

    let mut children = Vec::new();
    for id in [&first, &second] {
        let mut command = repo.orbit_command(&["delete", id.as_str(), "--force", "--json"]);
        command.stdout(Stdio::piped()).stderr(Stdio::piped());
        children.push(command.spawn().unwrap());
    }

    for child in children {
        let output = child.wait_with_output().unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
    }

    let doctor = repo.ok_json(&["doctor", "--json"]);
    assert_eq!(doctor["data"]["healthy"], true);

    let third = repo.ok_json(&["show", third.as_str(), "--json"]);
    assert!(third["data"]["depends_on"].as_array().unwrap().is_empty());
}

// --- --if-version and --idempotency-key --------------------------------------

fn updated_at(repo: &TestRepo, id: &str) -> String {
    repo.ok_json(&["show", id, "--json"])["data"]["updated_at"]
        .as_str()
        .unwrap()
        .to_string()
}

fn batch(repo: &TestRepo, operations: Value) -> (i32, Value) {
    repo.json_with_stdin(
        &["batch", "--file", "-", "--json"],
        &json!({ "operations": operations }).to_string(),
    )
}

#[test]
fn if_version_accepts_the_current_version_and_rejects_a_stale_one() {
    let repo = TestRepo::new();
    repo.init();
    let id = repo.add("Task", &[]);
    let version = updated_at(&repo, &id);

    // A real mutation moves the version forward.
    repo.ok_json(&["update", &id, "--title", "Renamed", "--json"]);

    let (_, conflict) = repo.err_json(&[
        "update",
        &id,
        "--title",
        "Stale write",
        "--if-version",
        &version,
        "--json",
    ]);
    assert_eq!(conflict["error"]["code"], "VERSION_CONFLICT", "{conflict}");
    assert_eq!(conflict["error"]["retryable"], true, "{conflict}");
    assert_eq!(conflict["error"]["details"]["task"], id, "{conflict}");

    // The version `orbit show` now reports is accepted.
    let current = updated_at(&repo, &id);
    let updated = repo.ok_json(&[
        "update",
        &id,
        "--title",
        "Fresh write",
        "--if-version",
        &current,
        "--json",
    ]);
    assert_eq!(updated["data"]["title"], "Fresh write");
}

#[test]
fn if_version_resolves_project_keys_and_rejects_bad_input() {
    let repo = TestRepo::new();
    repo.init();
    let id = repo.add("Task", &[]);
    let version = updated_at(&repo, &id);

    let (_, bad) = repo.err_json(&[
        "update",
        &id,
        "--title",
        "Nope",
        "--if-version",
        "not-a-date",
        "--json",
    ]);
    assert_eq!(bad["error"]["code"], "INVALID_ARGUMENT", "{bad}");

    // Read commands and creations have no version to guard.
    for args in [
        vec!["list", "--if-version", &version],
        vec!["add", "Other", "--if-version", &version],
        vec!["show", &id, "--if-version", &version],
    ] {
        let mut full = args.clone();
        full.push("--json");
        let (_, error) = repo.err_json(&full);
        assert!(
            error["error"]["message"]
                .as_str()
                .unwrap()
                .contains("--if-version"),
            "{args:?}: {error}"
        );
    }
}

#[test]
fn if_version_guards_ready_claim() {
    let repo = TestRepo::new();
    repo.init();
    let id = repo.add("Task", &[]);
    let version = updated_at(&repo, &id);
    // Bump the Task's version without claiming it.
    repo.ok_json(&["update", &id, "--title", "Still ready", "--json"]);

    let (_, conflict) = repo.err_json(&[
        "ready",
        "--claim",
        "--by",
        "agent",
        "--if-version",
        &version,
        "--json",
    ]);
    assert_eq!(conflict["error"]["code"], "VERSION_CONFLICT", "{conflict}");
    assert_eq!(conflict["error"]["retryable"], true, "{conflict}");

    let current = updated_at(&repo, &id);
    let claimed = repo.ok_json(&[
        "ready",
        "--claim",
        "--by",
        "agent",
        "--if-version",
        &current,
        "--json",
    ]);
    assert_eq!(claimed["data"]["id"], id);
    assert_eq!(claimed["data"]["status"], "in_progress");
}

#[test]
fn idempotency_key_replays_the_first_result() {
    let repo = TestRepo::new();
    repo.init();

    let first = repo.ok_json(&["add", "Once", "--idempotency-key", "k-1", "--json"]);
    let id = first["data"]["id"].as_str().unwrap().to_string();

    // A retry with a different payload still returns the recorded result.
    let again = repo.ok_json(&[
        "add",
        "Different title",
        "--idempotency-key",
        "k-1",
        "--json",
    ]);
    assert_eq!(again["data"]["id"], id);
    assert_eq!(again["data"]["title"], "Once");
    assert_eq!(again["meta"]["idempotent_replay"], true, "{again}");

    let list = repo.ok_json(&["list", "--json"]);
    assert_eq!(list["data"]["count"], 1);
}

#[test]
fn idempotency_key_survives_restarts_and_is_not_consumed_by_failures() {
    let repo = TestRepo::new();
    repo.init();
    let id = repo.add("Task", &[]);

    // A failed call leaves the key free for a real retry.
    let (_, missing) = repo.err_json(&[
        "update",
        "orb-missing",
        "--title",
        "Nope",
        "--idempotency-key",
        "k-2",
        "--json",
    ]);
    assert_eq!(missing["error"]["code"], "NOT_FOUND", "{missing}");

    let updated = repo.ok_json(&[
        "update",
        &id,
        "--title",
        "Renamed",
        "--idempotency-key",
        "k-2",
        "--json",
    ]);
    assert_eq!(updated["data"]["title"], "Renamed");

    // The record lives under the Board cache, so a fresh process replays it.
    let replayed = repo.ok_json(&[
        "update",
        &id,
        "--title",
        "Would differ",
        "--idempotency-key",
        "k-2",
        "--json",
    ]);
    assert_eq!(replayed["data"]["title"], "Renamed");
    assert_eq!(replayed["meta"]["idempotent_replay"], true, "{replayed}");

    // Read commands cannot take the key.
    let (_, read) = repo.err_json(&["list", "--idempotency-key", "k-3", "--json"]);
    assert!(
        read["error"]["message"]
            .as_str()
            .unwrap()
            .contains("--idempotency-key"),
        "{read}"
    );
}

#[test]
fn batch_if_version_conflict_rolls_back_the_whole_batch() {
    let repo = TestRepo::new();
    repo.init();
    let existing = repo.add("Existing", &[]);
    let version = updated_at(&repo, &existing);
    repo.ok_json(&["update", &existing, "--title", "Moved", "--json"]);

    let before = repo.ok_json(&["list", "--json"])["data"]["count"]
        .as_u64()
        .unwrap();
    let (status, value) = batch(
        &repo,
        json!([
            {"op": "add", "key": "planner:x:1", "title": "Never lands"},
            {"op": "update", "id": existing, "title": "Blocked", "if_version": version}
        ]),
    );
    assert_ne!(status, 0, "{value}");
    assert_eq!(value["error"]["code"], "VERSION_CONFLICT", "{value}");
    assert_eq!(value["error"]["retryable"], true, "{value}");

    // All-or-none: the `add` did not land either.
    let after = repo.ok_json(&["list", "--json"])["data"]["count"]
        .as_u64()
        .unwrap();
    assert_eq!(after, before);
    let show = repo.ok_json(&["show", &existing, "--json"]);
    assert_eq!(show["data"]["title"], "Moved");
}

#[test]
fn batch_if_version_accepts_the_batch_start_version() {
    let repo = TestRepo::new();
    repo.init();
    let existing = repo.add("Existing", &[]);
    let version = updated_at(&repo, &existing);

    let (status, value) = batch(
        &repo,
        json!([
            {"op": "add", "key": "planner:x:1", "title": "Lands"},
            {"op": "update", "id": existing, "title": "Updated", "if_version": version}
        ]),
    );
    assert_eq!(status, 0, "{value}");
    assert_eq!(value["ok"], true, "{value}");

    let show = repo.ok_json(&["show", &existing, "--json"]);
    assert_eq!(show["data"]["title"], "Updated");
}

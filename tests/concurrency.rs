mod common;

use common::TestRepo;
use serde_json::Value;
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
        repo.add(&format!("Task {index}"), &["-p", "1"]);
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
        let holder = value["data"]["claimed_by"].as_str().unwrap().to_string();
        assert!(claimed.insert(id), "the same task was claimed twice");
        assert!(holders.insert(holder), "two claims had the same holder");
    }

    assert_eq!(claimed.len(), 8);
    assert_eq!(holders.len(), 8);

    let all = repo.ok_json(&["list", "--all", "--json"]);
    assert_eq!(all["data"]["count"], 8);
    for task in all["data"]["tasks"].as_array().unwrap() {
        assert_eq!(task["status"], "in_progress");
        assert!(task["claimed_by"].is_string());
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

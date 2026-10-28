mod common;

use common::TestRepo;

#[test]
fn claimant_records_and_updates_a_checkpoint() {
    let repo = TestRepo::new();
    repo.init();
    let id = repo.add("Checkpointed work", &[]);
    repo.ok_json(&["claim", &id, "--by", "alice", "--json"]);

    let recorded = repo.ok_json(&[
        "checkpoint",
        &id,
        "--progress",
        "Implemented the parser",
        "--next",
        "Add regression tests",
        "--blocker",
        "Waiting on review",
        "--by",
        "alice",
        "--json",
    ]);
    let checkpoint = &recorded["data"]["checkpoint"];
    assert_eq!(checkpoint["by"], "alice");
    assert_eq!(checkpoint["progress"], "Implemented the parser");
    assert_eq!(checkpoint["next"], "Add regression tests");
    assert_eq!(checkpoint["blocker"], "Waiting on review");
    assert_eq!(recorded["data"]["checkpoint_archived"], false);
    assert!(
        checkpoint["at"]
            .as_str()
            .is_some_and(|at| at.ends_with('Z'))
    );

    let updated = repo.ok_json(&[
        "checkpoint",
        &id,
        "--progress",
        "Implemented the parser and its tests",
        "--clear-blocker",
        "--by",
        "alice",
        "--json",
    ]);
    let checkpoint = &updated["data"]["checkpoint"];
    assert_eq!(
        checkpoint["progress"],
        "Implemented the parser and its tests"
    );
    assert_eq!(checkpoint["next"], "Add regression tests");
    assert!(checkpoint["blocker"].is_null());

    let cleared = repo.ok_json(&["checkpoint", &id, "--clear-next", "--by", "alice", "--json"]);
    assert!(cleared["data"]["checkpoint"]["next"].is_null());
    assert_eq!(
        cleared["data"]["checkpoint"]["progress"],
        "Implemented the parser and its tests"
    );
}

#[test]
fn checkpoint_survives_release_and_takeover() {
    let repo = TestRepo::new();
    repo.init();
    let id = repo.add("Handoff work", &[]);
    repo.ok_json(&["claim", &id, "--by", "alice", "--json"]);
    repo.ok_json(&[
        "checkpoint",
        &id,
        "--progress",
        "Finished the schema",
        "--next",
        "Wire the CLI",
        "--by",
        "alice",
        "--json",
    ]);

    repo.ok_json(&["release", &id, "--by", "alice", "--json"]);

    let ready = repo.ok_json(&["ready", "--json"]);
    assert_eq!(ready["data"]["count"], 1);
    assert_eq!(
        ready["data"]["tasks"][0]["checkpoint"]["progress"],
        "Finished the schema"
    );

    let claimed = repo.ok_json(&["claim", &id, "--by", "bob", "--json"]);
    assert_eq!(claimed["data"]["checkpoint"]["by"], "alice");

    let stolen = repo.ok_json(&["claim", &id, "--by", "carol", "--steal", "--json"]);
    assert_eq!(stolen["data"]["checkpoint"]["next"], "Wire the CLI");

    let shown = repo.ok_json(&["show", &id, "--json"]);
    assert_eq!(
        shown["data"]["checkpoint"]["progress"],
        "Finished the schema"
    );
}

#[test]
fn stale_claim_takeover_exposes_the_checkpoint() {
    let repo = TestRepo::new();
    repo.init();
    let id = repo.add("Stale handoff", &[]);
    repo.ok_json(&["claim", &id, "--by", "alice", "--json"]);
    repo.ok_json(&[
        "checkpoint",
        &id,
        "--progress",
        "Half done",
        "--next",
        "Finish the tests",
        "--by",
        "alice",
        "--json",
    ]);
    repo.make_stale(&id);

    let stale_update = repo.ok_json(&[
        "checkpoint",
        &id,
        "--progress",
        "Still making progress",
        "--by",
        "alice",
        "--json",
    ]);
    assert_eq!(
        stale_update["data"]["checkpoint"]["progress"],
        "Still making progress"
    );

    let prime = repo.ok_json(&["prime", "--json"]);
    let stale = prime["data"]["stale"].as_array().unwrap();
    assert_eq!(stale.len(), 1);
    assert_eq!(stale[0]["checkpoint"]["progress"], "Still making progress");

    let prime_human = String::from_utf8(repo.orbit(&["prime"]).stdout).unwrap();
    assert!(
        prime_human.contains("checkpoint: Still making progress"),
        "{prime_human}"
    );

    let stolen = repo.ok_json(&["claim", &id, "--by", "bob", "--steal", "--json"]);
    assert_eq!(stolen["data"]["checkpoint"]["by"], "alice");
    assert_eq!(
        stolen["data"]["checkpoint"]["progress"],
        "Still making progress"
    );
    assert_eq!(stolen["data"]["checkpoint"]["next"], "Finish the tests");
}

#[test]
fn checkpoints_do_not_change_readiness() {
    let repo = TestRepo::new();
    repo.init();
    let first = repo.add("First", &[]);
    let second = repo.add("Second", &["--depends-on", first.as_str()]);

    let before = repo.ok_json(&["list", "--json"]);
    assert_eq!(before["data"]["count"], 2);

    repo.ok_json(&["claim", &first, "--by", "alice", "--json"]);
    repo.ok_json(&[
        "checkpoint",
        &first,
        "--progress",
        "Halfway",
        "--by",
        "alice",
        "--json",
    ]);

    let second_task = repo.ok_json(&["show", &second, "--json"]);
    assert_eq!(second_task["data"]["ready"], false);
    assert_eq!(second_task["data"]["blocking"][0], first.as_str());

    let released = repo.ok_json(&["release", &first, "--by", "alice", "--json"]);
    assert_eq!(released["data"]["status"], "todo");

    let ready = repo.ok_json(&["ready", "--json"]);
    assert_eq!(ready["data"]["count"], 1);
    assert_eq!(ready["data"]["tasks"][0]["id"], first.as_str());
}

#[test]
fn terminal_tasks_keep_the_checkpoint_as_archived_history() {
    let repo = TestRepo::new();
    repo.init();
    let id = repo.add("Finished work", &[]);
    repo.ok_json(&["claim", &id, "--by", "alice", "--json"]);
    repo.ok_json(&[
        "checkpoint",
        &id,
        "--progress",
        "All checks pass",
        "--by",
        "alice",
        "--json",
    ]);
    repo.ok_json(&["done", &id, "--by", "alice", "--json"]);

    let shown = repo.ok_json(&["show", &id, "--json"]);
    assert_eq!(shown["data"]["status"], "done");
    assert_eq!(shown["data"]["checkpoint"]["progress"], "All checks pass");
    assert_eq!(shown["data"]["checkpoint_archived"], true);

    let prime = repo.ok_json(&["prime", "--json"]);
    assert!(prime["data"]["ready"].as_array().unwrap().is_empty());
    assert!(prime["data"]["stale"].as_array().unwrap().is_empty());

    let reopened = repo.ok_json(&["reopen", &id, "--json"]);
    assert_eq!(reopened["data"]["checkpoint_archived"], false);
    assert_eq!(
        reopened["data"]["checkpoint"]["progress"],
        "All checks pass"
    );

    let human = String::from_utf8(repo.orbit(&["show", &id]).stdout).unwrap();
    assert!(human.contains("checkpoint: alice"), "{human}");
    assert!(human.contains("progress: All checks pass"), "{human}");

    let cancelled = repo.add("Cancelled work", &[]);
    repo.ok_json(&["claim", &cancelled, "--by", "alice", "--json"]);
    repo.ok_json(&[
        "checkpoint",
        &cancelled,
        "--progress",
        "Stopped after the spike",
        "--by",
        "alice",
        "--json",
    ]);
    repo.ok_json(&["cancel", &cancelled, "--by", "alice", "--json"]);
    let cancelled = repo.ok_json(&["show", &cancelled, "--json"]);
    assert_eq!(cancelled["data"]["status"], "cancelled");
    assert_eq!(
        cancelled["data"]["checkpoint"]["progress"],
        "Stopped after the spike"
    );
    assert_eq!(cancelled["data"]["checkpoint_archived"], true);
}

#[test]
fn only_the_claimant_can_record_a_checkpoint() {
    let repo = TestRepo::new();
    repo.init();
    let id = repo.add("Guarded work", &[]);

    let (_, unclaimed) = repo.err_json(&[
        "checkpoint",
        &id,
        "--progress",
        "No claim yet",
        "--by",
        "alice",
        "--json",
    ]);
    assert_eq!(unclaimed["error"]["code"], "INVALID_TRANSITION");

    repo.ok_json(&["claim", &id, "--by", "alice", "--json"]);
    let (_, not_holder) = repo.err_json(&[
        "checkpoint",
        &id,
        "--progress",
        "Bob was here",
        "--by",
        "bob",
        "--json",
    ]);
    assert_eq!(not_holder["error"]["code"], "NOT_CLAIM_HOLDER");

    let (_, missing_progress) = repo.err_json(&[
        "checkpoint",
        &id,
        "--next",
        "Skip progress",
        "--by",
        "alice",
        "--json",
    ]);
    assert_eq!(missing_progress["error"]["code"], "INVALID_ARGUMENT");

    let (_, empty_progress) = repo.err_json(&[
        "checkpoint",
        &id,
        "--progress",
        "   ",
        "--by",
        "alice",
        "--json",
    ]);
    assert_eq!(empty_progress["error"]["code"], "INVALID_ARGUMENT");

    repo.ok_json(&[
        "checkpoint",
        &id,
        "--progress",
        "Work in progress",
        "--by",
        "alice",
        "--json",
    ]);
    repo.ok_json(&["done", &id, "--by", "alice", "--json"]);

    let (_, terminal) = repo.err_json(&[
        "checkpoint",
        &id,
        "--progress",
        "Too late",
        "--by",
        "alice",
        "--json",
    ]);
    assert_eq!(terminal["error"]["code"], "INVALID_TRANSITION");
}

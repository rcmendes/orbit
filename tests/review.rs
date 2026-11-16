//! Review, evidence, and escalation (ADR 0022): `orbit submit` records
//! structured evidence and moves work into In Review, `orbit review`
//! approves or requests changes under `[policy]` guards, attempt limits
//! escalate to `needs_human`, and `orbit ask`/`orbit answer` block an Issue
//! until answered.

mod common;

use common::TestRepo;

/// Appends a `[policy]` section to the Board's `config.toml`.
fn write_policy(repo: &TestRepo, toml: &str) {
    let path = repo.board_path().join("config.toml");
    let mut config = std::fs::read_to_string(&path).unwrap();
    config.push_str("\n[policy]\n");
    config.push_str(toml);
    std::fs::write(&path, config).unwrap();
}

fn status_of(repo: &TestRepo, id: &str) -> String {
    let shown = repo.ok_json(&["show", id, "--json"]);
    shown["data"]["status"].as_str().unwrap().to_string()
}

fn properties(repo: &TestRepo, id: &str) -> serde_json::Value {
    repo.ok_json(&["show", id, "--json"])["data"]["properties"].clone()
}

/// Claims `id` for `agent`, then submits it with the given evidence flags.
fn submit(repo: &TestRepo, id: &str, agent: &str, extra: &[&str]) {
    repo.ok_json(&["claim", id, "--by", agent, "--json"]);
    let mut args = vec!["submit", id, "--by", agent, "--note", "done", "--json"];
    args.extend_from_slice(extra);
    repo.ok_json(&args);
}

#[test]
fn submit_records_evidence_and_moves_to_in_review() {
    let repo = TestRepo::new();
    repo.init();
    let id = repo.add("ship it", &[]);
    repo.ok_json(&["claim", &id, "--by", "devin", "--json"]);

    let value = repo.ok_json(&[
        "submit",
        &id,
        "--by",
        "devin",
        "--command",
        "cargo test",
        "--exit-code",
        "0",
        "--commit",
        "abc1234",
        "--file",
        "src/lib.rs",
        "--note",
        "all green",
        "--json",
    ]);
    assert_eq!(value["data"]["status"], "in_review");

    let props = properties(&repo, &id);
    let item = &props["evidence"]["items"][0];
    assert_eq!(item["by"], "devin");
    assert_eq!(item["commands"][0], "cargo test");
    assert_eq!(item["exit_code"], 0);
    assert_eq!(item["commit"], "abc1234");
    assert_eq!(item["files"][0], "src/lib.rs");
    assert_eq!(item["note"], "all green");
    assert_eq!(props["orbit"]["review"]["submitted_by"], "devin");
    assert_eq!(props["orbit"]["review"]["rounds"], 1);
    assert_eq!(status_of(&repo, &id), "in_review");
}

#[test]
fn submit_requires_evidence_and_an_in_progress_task() {
    let repo = TestRepo::new();
    repo.init();
    let id = repo.add("no proof", &[]);
    repo.ok_json(&["claim", &id, "--by", "devin", "--json"]);

    let (_, error) = repo.err_json(&["submit", &id, "--by", "devin", "--json"]);
    assert_eq!(error["error"]["code"], "INVALID_ARGUMENT");

    // From the Ready status there is no `submit` edge at all.
    let other = repo.add("not started", &[]);
    let (_, error) = repo.err_json(&["submit", &other, "--by", "devin", "--note", "x", "--json"]);
    assert_eq!(error["error"]["code"], "INVALID_TRANSITION");
}

#[test]
fn review_approve_closes_the_task() {
    let repo = TestRepo::new();
    repo.init();
    let id = repo.add("review me", &[]);
    submit(&repo, &id, "devin", &[]);

    let value = repo.ok_json(&[
        "review",
        &id,
        "--verdict",
        "approve",
        "--by",
        "alice",
        "--kind",
        "human",
        "--json",
    ]);
    assert_eq!(value["data"]["status"], "done");
    assert_eq!(value["data"]["resolution"], "completed");

    let props = properties(&repo, &id);
    assert_eq!(props["orbit"]["review"]["verdict"], "approve");
    assert_eq!(props["orbit"]["review"]["reviewed_by"], "alice");
    assert_eq!(props["orbit"]["review"]["reviewed_kind"], "human");
    // A closed Issue carries no assignee.
    assert!(value["data"]["assignee"].is_null());
}

#[test]
fn review_request_changes_needs_a_reason_and_returns_to_the_author() {
    let repo = TestRepo::new();
    repo.init();
    let id = repo.add("fix it", &[]);
    submit(&repo, &id, "devin", &[]);

    let (_, error) = repo.err_json(&[
        "review",
        &id,
        "--verdict",
        "request-changes",
        "--by",
        "alice",
        "--kind",
        "human",
        "--json",
    ]);
    assert_eq!(error["error"]["code"], "INVALID_ARGUMENT");

    let value = repo.ok_json(&[
        "review",
        &id,
        "--verdict",
        "request-changes",
        "--reason",
        "missing regression test",
        "--by",
        "alice",
        "--kind",
        "human",
        "--json",
    ]);
    assert_eq!(value["data"]["status"], "in_progress");
    // The Issue goes back to its recorded author.
    assert_eq!(value["data"]["assignee"], "devin");

    let props = properties(&repo, &id);
    assert_eq!(props["orbit"]["review"]["verdict"], "request-changes");
    assert_eq!(
        props["orbit"]["review"]["reason"],
        "missing regression test"
    );
}

#[test]
fn policy_refuses_a_reviewer_who_is_the_author() {
    let repo = TestRepo::new();
    repo.init();
    write_policy(
        &repo,
        r#"
[[policy.rules]]
name = "reviewer-is-not-author"
transition = "request_changes"
distinct_from = ["author"]
"#,
    );
    let id = repo.add("self review", &["--reporter", "carol"]);
    submit(&repo, &id, "devin", &[]);

    // The submitter cannot review their own work.
    let (_, error) = repo.err_json(&[
        "review",
        &id,
        "--verdict",
        "request-changes",
        "--reason",
        "no",
        "--by",
        "devin",
        "--json",
    ]);
    assert_eq!(error["error"]["code"], "POLICY_VIOLATION");
    assert_eq!(
        error["error"]["details"]["violations"][0]["check"],
        "distinct_from"
    );
    assert_eq!(status_of(&repo, &id), "in_review");

    // A different reviewer passes.
    repo.ok_json(&[
        "review",
        &id,
        "--verdict",
        "request-changes",
        "--reason",
        "needs work",
        "--by",
        "alice",
        "--kind",
        "human",
        "--json",
    ]);
    assert_eq!(status_of(&repo, &id), "in_progress");
}

#[test]
fn attempt_limit_escalates_request_changes_to_needs_human() {
    let repo = TestRepo::new();
    repo.init();
    write_policy(
        &repo,
        r#"
[[policy.rules]]
name = "one-review-round"
transition = "request_changes"
max_attempts = 1
"#,
    );
    let id = repo.add("looping", &[]);
    submit(&repo, &id, "devin", &[]);

    // First request-changes consumes the single allowed attempt.
    repo.ok_json(&[
        "review",
        &id,
        "--verdict",
        "request-changes",
        "--reason",
        "round one",
        "--by",
        "alice",
        "--kind",
        "human",
        "--json",
    ]);
    assert_eq!(status_of(&repo, &id), "in_progress");

    // The second hits the attempt limit and escalates instead of failing.
    submit(&repo, &id, "devin", &[]);
    let value = repo.ok_json(&[
        "review",
        &id,
        "--verdict",
        "request-changes",
        "--reason",
        "round two",
        "--by",
        "alice",
        "--kind",
        "human",
        "--json",
    ]);
    assert_eq!(value["data"]["status"], "needs_human");

    let props = properties(&repo, &id);
    assert_eq!(props["orbit"]["escalation"]["reason"], "attempt-limit");
    assert_eq!(props["orbit"]["escalation"]["rules"][0], "one-review-round");
    assert_eq!(props["orbit"]["review"]["verdict"], "request-changes");
    // The hold sits with the author, not the reviewer.
    assert_eq!(value["data"]["assignee"], "devin");
}

#[test]
fn ask_blocks_the_task_until_answered() {
    let repo = TestRepo::new();
    repo.init();
    let id = repo.add("blocked question", &[]);
    repo.ok_json(&["claim", &id, "--by", "devin", "--json"]);

    let value = repo.ok_json(&[
        "ask",
        &id,
        "which environment should this target?",
        "--by",
        "devin",
        "--json",
    ]);
    assert_eq!(value["data"]["status"], "needs_human");

    // The open Ask blocks completion and other transitions.
    let (_, error) = repo.err_json(&["done", &id, "--by", "devin", "--json"]);
    assert_eq!(error["error"]["code"], "ASK_OPEN");
    let (_, error) = repo.err_json(&["submit", &id, "--by", "devin", "--note", "x", "--json"]);
    assert_eq!(error["error"]["code"], "ASK_OPEN");

    // Answering restores the pre-ask status.
    let value = repo.ok_json(&["answer", &id, "staging first", "--by", "alice", "--json"]);
    assert_eq!(value["data"]["status"], "in_progress");
    let props = properties(&repo, &id);
    assert_eq!(props["orbit"]["ask"]["answer"], "staging first");
    assert_eq!(props["orbit"]["ask"]["answered_by"], "alice");

    // The block is gone: the task can finish.
    repo.ok_json(&["done", &id, "--by", "devin", "--json"]);
    assert_eq!(status_of(&repo, &id), "done");
}

#[test]
fn ask_on_a_ready_task_blocks_without_moving_status() {
    let repo = TestRepo::new();
    repo.init();
    let id = repo.add("question up front", &[]);

    // `ask` is not an edge from `ready`, so the status stays — but the open
    // Ask still blocks readiness and claiming.
    let value = repo.ok_json(&["ask", &id, "scope?", "--by", "alice", "--json"]);
    assert_eq!(value["data"]["status"], "ready");
    assert_eq!(value["data"]["ready"], false);

    let (_, error) = repo.err_json(&["claim", &id, "--by", "devin", "--json"]);
    assert_eq!(error["error"]["code"], "ASK_OPEN");

    repo.ok_json(&["answer", &id, "the whole board", "--by", "alice", "--json"]);
    repo.ok_json(&["claim", &id, "--by", "devin", "--json"]);
    assert_eq!(status_of(&repo, &id), "in_progress");
}

#[test]
fn ask_and_answer_validate_their_inputs() {
    let repo = TestRepo::new();
    repo.init();
    let id = repo.add("strict", &[]);
    repo.ok_json(&["claim", &id, "--by", "devin", "--json"]);

    let (_, error) = repo.err_json(&["answer", &id, "early", "--by", "alice", "--json"]);
    assert_eq!(error["error"]["code"], "INVALID_ARGUMENT");

    repo.ok_json(&["ask", &id, "first?", "--by", "devin", "--json"]);
    // A second open Ask is refused while the first is unanswered.
    let (_, error) = repo.err_json(&["ask", &id, "second?", "--by", "devin", "--json"]);
    assert_eq!(error["error"]["code"], "ASK_OPEN");
}

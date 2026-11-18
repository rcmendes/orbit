//! `[policy]` rules (ADR 0022): declarative guards on workflow transitions —
//! required fields, evidence properties, WIP limits, attempt limits,
//! post-functions, and the readiness lint — enforced in `ops` so the CLI,
//! `orbit batch`, and MCP share them.

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

#[test]
fn transition_into_ready_requires_fields_with_actionable_error() {
    let repo = TestRepo::new();
    repo.init();
    write_policy(
        &repo,
        r#"
[[policy.rules]]
name = "ready-brief"
transition = "*"
to = "ready"
require_fields = ["verification"]
"#,
    );

    let id = repo.add("vague ticket", &[]);
    // New Tasks land in the Ready status directly; park it in the backlog.
    repo.ok_json(&["transition", &id, "defer", "--json"]);
    assert_eq!(status_of(&repo, &id), "todo");

    let (_, error) = repo.err_json(&["transition", &id, "promote", "--json"]);
    assert_eq!(error["error"]["code"], "POLICY_VIOLATION");
    assert!(
        error["error"]["message"]
            .as_str()
            .unwrap()
            .contains("ready-brief"),
        "{error}"
    );
    let violations = &error["error"]["details"]["violations"];
    assert_eq!(violations[0]["check"], "require_fields");
    assert!(
        violations[0]["hint"]
            .as_str()
            .unwrap()
            .contains("orbit update"),
        "{error}"
    );
    // The failed transition rolled back: still in the backlog.
    assert_eq!(status_of(&repo, &id), "todo");

    repo.ok_json(&["update", &id, "--verification", "cargo test", "--json"]);
    repo.ok_json(&["transition", &id, "promote", "--json"]);
    assert_eq!(status_of(&repo, &id), "ready");
}

#[test]
fn evidence_property_satisfies_require_properties() {
    let repo = TestRepo::new();
    repo.init();
    write_policy(
        &repo,
        r#"
[[policy.rules]]
name = "done-evidence"
transition = "done"
require_properties = ["proof"]
"#,
    );

    let id = repo.add("ship it", &[]);
    repo.ok_json(&["claim", &id, "--by", "devin", "--json"]);
    let (_, error) = repo.err_json(&["done", &id, "--by", "devin", "--json"]);
    assert_eq!(error["error"]["code"], "POLICY_VIOLATION");
    assert_eq!(
        error["error"]["details"]["violations"][0]["check"],
        "require_properties"
    );

    repo.ok_json(&[
        "property",
        "set",
        &id,
        "proof",
        "--value",
        r#"{"command":"cargo test","exit_code":0}"#,
        "--json",
    ]);
    repo.ok_json(&["done", &id, "--by", "devin", "--json"]);
    assert_eq!(status_of(&repo, &id), "done");
}

#[test]
fn wip_limit_blocks_claim_past_the_cap() {
    let repo = TestRepo::new();
    repo.init();
    write_policy(
        &repo,
        r#"
[[policy.rules]]
name = "one-at-a-time"
transition = "claim"
wip_limit = { status = "in_progress", max = 1 }
"#,
    );

    let first = repo.add("first", &[]);
    let second = repo.add("second", &[]);
    repo.ok_json(&["claim", &first, "--by", "devin", "--json"]);
    assert_eq!(status_of(&repo, &first), "in_progress");

    let (_, error) = repo.err_json(&["claim", &second, "--by", "devin", "--json"]);
    assert_eq!(error["error"]["code"], "POLICY_VIOLATION");
    assert_eq!(
        error["error"]["details"]["violations"][0]["check"],
        "wip_limit"
    );
    assert_eq!(status_of(&repo, &second), "ready");

    repo.ok_json(&["done", &first, "--by", "devin", "--json"]);
    repo.ok_json(&["claim", &second, "--by", "devin", "--json"]);
    assert_eq!(status_of(&repo, &second), "in_progress");
}

#[test]
fn max_attempts_counts_successful_governed_transitions() {
    let repo = TestRepo::new();
    repo.init();
    write_policy(
        &repo,
        r#"
[[policy.rules]]
name = "one-defer"
transition = "defer"
max_attempts = 1
"#,
    );

    let id = repo.add("ping-pong", &[]);
    repo.ok_json(&["transition", &id, "defer", "--json"]);
    repo.ok_json(&["transition", &id, "promote", "--json"]);

    let (_, error) = repo.err_json(&["transition", &id, "defer", "--json"]);
    assert_eq!(error["error"]["code"], "POLICY_VIOLATION");
    assert_eq!(
        error["error"]["details"]["violations"][0]["check"],
        "max_attempts"
    );
    assert_eq!(status_of(&repo, &id), "ready");

    // The attempt counter lives in the `orbit` property.
    let shown = repo.ok_json(&["show", &id, "--json"]);
    assert_eq!(
        shown["data"]["properties"]["orbit"]["attempts"]["one-defer"],
        1
    );
}

#[test]
fn post_functions_run_after_a_governed_transition() {
    let repo = TestRepo::new();
    repo.init();
    write_policy(
        &repo,
        r#"
[[policy.rules]]
name = "closing-bookkeeping"
transition = "done"
add_labels = ["shipped"]
set_properties = { verdict = { ok = true } }
"#,
    );

    let id = repo.add("booked", &[]);
    repo.ok_json(&["done", &id, "--json"]);
    let shown = repo.ok_json(&["show", &id, "--json"]);
    let labels = shown["data"]["labels"].as_array().unwrap();
    assert!(labels.iter().any(|label| label == "shipped"));
    assert_eq!(shown["data"]["properties"]["verdict"]["ok"], true);
}

#[test]
fn rules_scope_by_type_label_and_profile() {
    let repo = TestRepo::new();
    repo.init();
    // A hand-written applied-profile record scopes the `acme-only` rule.
    let path = repo.board_path().join("config.toml");
    let mut config = std::fs::read_to_string(&path).unwrap();
    config.push_str(
        r#"
[[profiles]]
name = "acme"
requires = []

[policy]

[[policy.rules]]
name = "bug-evidence"
transition = "*"
to = "ready"
types = ["bug"]
require_properties = ["proof"]

[[policy.rules]]
name = "labeled-evidence"
transition = "*"
to = "ready"
labels = ["needs-evidence"]
require_properties = ["proof"]

[[policy.rules]]
name = "acme-only"
transition = "*"
to = "ready"
profiles = ["acme"]
require_fields = ["verification"]
"#,
    );
    std::fs::write(&path, config).unwrap();

    // A plain task only trips the profile-scoped rule.
    let plain = repo.add("plain", &[]);
    repo.ok_json(&["transition", &plain, "defer", "--json"]);
    let (_, error) = repo.err_json(&["transition", &plain, "promote", "--json"]);
    assert_eq!(error["error"]["code"], "POLICY_VIOLATION");
    assert!(
        error["error"]["message"]
            .as_str()
            .unwrap()
            .contains("acme-only"),
        "{error}"
    );
    repo.ok_json(&["update", &plain, "--verification", "cargo test", "--json"]);
    repo.ok_json(&["transition", &plain, "promote", "--json"]);

    // A bug trips the type-scoped rule; the labeled rule needs the label.
    let bug = repo.add("bug", &["--type", "bug"]);
    repo.ok_json(&["transition", &bug, "defer", "--json"]);
    let (_, error) = repo.err_json(&["transition", &bug, "promote", "--json"]);
    let rules: Vec<&str> = error["error"]["details"]["violations"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|violation| violation["rule"].as_str())
        .collect();
    assert!(rules.contains(&"bug-evidence"), "{rules:?}");
    assert!(rules.contains(&"acme-only"), "{rules:?}");
    assert!(!rules.contains(&"labeled-evidence"), "{rules:?}");

    repo.ok_json(&[
        "property",
        "set",
        &bug,
        "proof",
        "--value",
        r#"{"command":"cargo test"}"#,
        "--json",
    ]);
    repo.ok_json(&["update", &bug, "--verification", "cargo test", "--json"]);
    repo.ok_json(&["transition", &bug, "promote", "--json"]);
    assert_eq!(status_of(&repo, &bug), "ready");
}

#[test]
fn readiness_lint_replaces_the_ready_brief_check() {
    let repo = TestRepo::new();
    repo.init();
    write_policy(
        &repo,
        r#"
[[policy.rules]]
name = "ready-lint"
transition = "*"
to = "ready"
lint = true
require_fields = ["verification"]
"#,
    );

    // `orbit add` files straight into the Ready status, so the lint — not
    // the transition — is what keeps a vague Task out of the queue.
    let vague = repo.add("vague", &[]);
    let briefed = repo.add("briefed", &[]);
    repo.ok_json(&["update", &briefed, "--verification", "cargo test", "--json"]);

    let ready = repo.ok_json(&["ready", "--json"]);
    let ids: Vec<&str> = ready["data"]["issues"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|task| task["id"].as_str())
        .collect();
    assert!(ids.contains(&briefed.as_str()), "{ids:?}");
    assert!(!ids.contains(&vague.as_str()), "{ids:?}");

    // `ready --claim` cannot pick the lint-failing Task either.
    let claim = repo.ok_json(&["ready", "--claim", "--by", "devin", "--json"]);
    assert_eq!(claim["data"]["id"], briefed.as_str());

    // A direct claim on the vague Task fails the lint with an actionable error.
    let (_, error) = repo.err_json(&["claim", &vague, "--by", "devin", "--json"]);
    assert_eq!(error["error"]["code"], "POLICY_VIOLATION");
    assert_eq!(
        error["error"]["details"]["violations"][0]["rule"],
        "ready-lint"
    );

    // Entering Ready through a transition is guarded by the same rule.
    let parked = repo.add("parked", &[]);
    repo.ok_json(&["transition", &parked, "defer", "--json"]);
    let (_, error) = repo.err_json(&["transition", &parked, "promote", "--json"]);
    assert_eq!(error["error"]["code"], "POLICY_VIOLATION");
}

#[test]
fn batch_enforces_policy_all_or_none() {
    let repo = TestRepo::new();
    repo.init();
    write_policy(
        &repo,
        r#"
[[policy.rules]]
name = "ready-brief"
transition = "*"
to = "ready"
require_fields = ["verification"]
"#,
    );

    let ok = repo.add("fine", &[]);
    repo.ok_json(&["update", &ok, "--verification", "cargo test", "--json"]);
    let bad = repo.add("bad", &[]);
    repo.ok_json(&["transition", &bad, "defer", "--json"]);

    let plan = format!(
        r#"{{"operations": [
            {{"op": "transition", "id": "{ok}", "name": "defer"}},
            {{"op": "transition", "id": "{bad}", "name": "promote"}}
        ]}}"#
    );
    let plan_path = repo.path().join("plan.json");
    std::fs::write(&plan_path, plan).unwrap();
    let (_, error) = repo.err_json(&["batch", "--file", plan_path.to_str().unwrap(), "--json"]);
    assert_eq!(error["error"]["code"], "POLICY_VIOLATION", "{error}");
    // All-or-none: the first (valid) transition rolled back with the batch.
    assert_eq!(status_of(&repo, &ok), "ready");
    assert_eq!(status_of(&repo, &bad), "todo");
}

#[test]
fn profile_contributes_policy_rules() {
    let repo = TestRepo::new();
    repo.init();
    let profiles = repo.home().join(".config").join("orbit").join("profiles");
    std::fs::create_dir_all(&profiles).unwrap();
    std::fs::write(
        profiles.join("acme.toml"),
        r#"
name = "acme"

[policy]

[[policy.rules]]
name = "acme-ready-lint"
transition = "*"
to = "ready"
lint = true
profiles = ["acme"]
require_fields = ["verification"]
"#,
    )
    .unwrap();

    repo.ok_json(&["setup", "acme", "--json"]);
    let id = repo.add("acme task", &[]);
    // The profile's lint rule drops the Task from the ready queue.
    let ready = repo.ok_json(&["ready", "--json"]);
    let ids: Vec<&str> = ready["data"]["issues"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|task| task["id"].as_str())
        .collect();
    assert!(!ids.contains(&id.as_str()), "{ids:?}");
    let (_, error) = repo.err_json(&["claim", &id, "--by", "devin", "--json"]);
    assert_eq!(error["error"]["code"], "POLICY_VIOLATION");
}

#[test]
fn reserved_property_owners_cannot_be_set_by_hand() {
    let repo = TestRepo::new();
    repo.init();
    let id = repo.add("guarded", &[]);
    for owner in ["orbit", "evidence"] {
        let (_, error) = repo.err_json(&["property", "set", &id, owner, "--value", "{}", "--json"]);
        assert_eq!(error["error"]["code"], "INVALID_ARGUMENT", "{error}");
    }
}

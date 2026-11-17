mod common;

use common::TestRepo;
use serde_json::Value;

fn ids(value: &Value) -> Vec<String> {
    value["data"]["tasks"]
        .as_array()
        .expect("tasks array")
        .iter()
        .map(|task| task["id"].as_str().unwrap().to_string())
        .collect()
}

fn set_scheduler(repo: &TestRepo, toml: &str) {
    let path = repo.board_path().join("config.toml");
    let mut config = std::fs::read_to_string(&path).unwrap();
    config.push_str(toml);
    std::fs::write(&path, config).unwrap();
}

fn set_property(repo: &TestRepo, id: &str, owner: &str, value: &str) {
    repo.ok_json(&["property", "set", id, owner, "--value", value, "--json"]);
}

#[test]
fn default_strategy_is_the_kernel_order() {
    let repo = TestRepo::new();
    repo.init();
    let low = repo.add("Low priority", &["-p", "low"]);
    let high = repo.add("High priority", &["-p", "high"]);

    let plain = repo.ok_json(&["ready", "--json"]);
    assert_eq!(ids(&plain), vec![high.clone(), low.clone()]);

    let explicit = repo.ok_json(&["ready", "--strategy", "default", "--json"]);
    assert_eq!(ids(&explicit), vec![high, low]);
}

#[test]
fn flag_strategy_overrides_board_config() {
    let repo = TestRepo::new();
    repo.init();
    // Independent task sorts first by id/age; the chain head unblocks two
    // dependents, so critical-path promotes it.
    let independent = repo.add("Independent", &[]);
    let head = repo.add("Chain head", &[]);
    let mid = repo.add("Chain mid", &[]);
    let leaf = repo.add("Chain leaf", &[]);
    repo.ok_json(&["depend", &mid, &head, "--json"]);
    repo.ok_json(&["depend", &leaf, &mid, "--json"]);

    set_scheduler(&repo, "\n[scheduler]\nstrategy = \"critical-path\"\n");

    // Board config applies without a flag.
    let configured = repo.ok_json(&["ready", "--json"]);
    assert_eq!(ids(&configured), vec![head.clone(), independent.clone()]);

    // The flag wins over the Board config.
    let flagged = repo.ok_json(&["ready", "--strategy", "default", "--json"]);
    assert_eq!(ids(&flagged), vec![independent, head]);
}

#[test]
fn touches_claim_never_overlaps_another_agents_paths() {
    let repo = TestRepo::new();
    repo.init();
    let held = repo.add("Held by alice", &[]);
    let overlap = repo.add("Touches the same tree", &[]);
    let disjoint = repo.add("Touches other files", &[]);

    set_property(&repo, &held, "touches", r#"{"paths": ["src/shared"]}"#);
    set_property(
        &repo,
        &overlap,
        "touches",
        r#"{"paths": ["src/shared/x.rs"]}"#,
    );
    set_property(&repo, &disjoint, "touches", r#"{"paths": ["src/other"]}"#);

    repo.ok_json(&["claim", &held, "--by", "alice", "--json"]);

    // The plain listing keeps every candidate but sorts the safe one first.
    let listed = repo.ok_json(&["ready", "--strategy", "touches", "--json"]);
    assert_eq!(ids(&listed), vec![disjoint.clone(), overlap.clone()]);

    // The claim refuses the overlapping candidate outright.
    let claimed = repo.ok_json(&[
        "ready",
        "--claim",
        "--by",
        "bob",
        "--strategy",
        "touches",
        "--json",
    ]);
    assert_eq!(claimed["data"]["id"], disjoint.as_str());

    // Without the strategy the same queue hands carol the overlapping task
    // that was created first.
    repo.add("Another overlap", &[]);
    let default_claim = repo.ok_json(&["ready", "--claim", "--by", "carol", "--json"]);
    assert_eq!(default_claim["data"]["id"], overlap.as_str());
}

#[test]
fn touches_overlap_is_scoped_per_assignee() {
    let repo = TestRepo::new();
    repo.init();
    let held = repo.add("Alice holds shared", &[]);
    let overlap = repo.add("Also touches shared", &[]);
    set_property(&repo, &held, "touches", r#"{"paths": ["src/shared"]}"#);
    set_property(&repo, &overlap, "touches", r#"{"paths": ["src/shared"]}"#);
    repo.ok_json(&["claim", &held, "--by", "alice", "--json"]);

    // Alice may stack overlapping work on her own claim.
    let claimed = repo.ok_json(&[
        "ready",
        "--claim",
        "--by",
        "alice",
        "--strategy",
        "touches",
        "--json",
    ]);
    assert_eq!(claimed["data"]["id"], overlap.as_str());
}

#[test]
fn capability_claim_prefers_tasks_the_agent_can_do() {
    let repo = TestRepo::new();
    repo.init();
    let kotlin = repo.add("Kotlin work", &[]);
    let rust = repo.add("Rust work", &[]);
    let open = repo.add("Anyone work", &[]);
    set_property(
        &repo,
        &kotlin,
        "capabilities",
        r#"{"requires": ["kotlin"]}"#,
    );
    set_property(&repo, &rust, "capabilities", r#"{"requires": ["rust"]}"#);

    set_scheduler(
        &repo,
        "\n[scheduler]\nstrategy = \"capability\"\n\n[scheduler.capabilities]\nbob = [\"rust\"]\n",
    );

    let claimed = repo.ok_json(&["ready", "--claim", "--by", "bob", "--json"]);
    assert_eq!(claimed["data"]["id"], rust.as_str());
    let _ = open;
}

#[test]
fn wip_limit_refuses_a_claim_at_capacity() {
    let repo = TestRepo::new();
    repo.init();
    let first = repo.add("First", &[]);
    let second = repo.add("Second", &[]);

    set_scheduler(&repo, "\n[scheduler]\nstrategy = \"wip\"\nwip_limit = 1\n");

    repo.ok_json(&["claim", &first, "--by", "alice", "--json"]);

    let blocked = repo.ok_json(&["ready", "--claim", "--by", "alice", "--json"]);
    assert_eq!(blocked["data"], Value::Null);
    assert_eq!(blocked["meta"]["reason"], "WIP_LIMIT");

    // A different agent under the limit still claims.
    let claimed = repo.ok_json(&["ready", "--claim", "--by", "bob", "--json"]);
    assert_eq!(claimed["data"]["id"], second.as_str());
}

#[test]
fn wip_strategy_requires_a_configured_limit() {
    let repo = TestRepo::new();
    repo.init();
    repo.add("Work", &[]);
    let (_, error) = repo.err_json(&[
        "ready",
        "--claim",
        "--by",
        "bob",
        "--strategy",
        "wip",
        "--json",
    ]);
    assert_eq!(error["error"]["code"], "INVALID_ARGUMENT");
}

#[test]
fn unknown_strategy_flag_is_rejected() {
    let repo = TestRepo::new();
    repo.init();
    repo.add("Work", &[]);
    let (_, error) = repo.err_json(&["ready", "--strategy", "bogus", "--json"]);
    assert_eq!(error["error"]["code"], "INVALID_ARGUMENT");
}

#[test]
fn bad_scheduler_config_fails_board_load() {
    let repo = TestRepo::new();
    repo.init();
    repo.add("Work", &[]);
    set_scheduler(&repo, "\n[scheduler]\nstrategy = \"bogus\"\n");
    let (_, error) = repo.err_json(&["ready", "--json"]);
    assert_eq!(error["error"]["code"], "CONFIG_ERROR");
}

//! Regression tests for the second-round review findings on the platform PR.

mod common;

use common::TestRepo;

fn write_policy(repo: &TestRepo, toml: &str) {
    let path = repo.board_path().join("config.toml");
    let mut config = std::fs::read_to_string(&path).unwrap();
    config.push_str("\n[policy]\n");
    config.push_str(toml);
    std::fs::write(&path, config).unwrap();
}

#[test]
fn distinct_from_applies_to_closing_transitions() {
    let repo = TestRepo::new();
    repo.init();
    write_policy(
        &repo,
        "\n[[policy.rules]]\nname = \"not-self\"\ntransition = \"done\"\ndistinct_from = [\"reporter\"]\n",
    );
    let id = repo.add("self close", &["--reporter", "carol"]);
    repo.ok_json(&["claim", &id, "--by", "carol", "--json"]);
    let (_, error) = repo.err_json(&["done", &id, "--by", "carol", "--json"]);
    assert_eq!(error["error"]["code"], "POLICY_VIOLATION", "{error}");
    assert_eq!(
        error["error"]["details"]["violations"][0]["check"],
        "distinct_from"
    );
}

#[test]
fn wip_limit_ignores_transitions_that_do_not_enter_the_scope() {
    let repo = TestRepo::new();
    repo.init();
    write_policy(
        &repo,
        "\n[[policy.rules]]\nname = \"cap\"\ntransition = \"*\"\nwip_limit = { status = \"in_progress\", max = 1 }\n",
    );
    let busy = repo.add("busy", &[]);
    let idle = repo.add("idle", &[]);
    repo.ok_json(&["claim", &busy, "--by", "devin", "--json"]);
    // Cancelling an unrelated ready Task never enters `in_progress`.
    repo.ok_json(&["cancel", &idle, "--by", "devin", "--json"]);
}

#[test]
fn parent_filters_accept_project_keys() {
    let repo = TestRepo::new();
    repo.init();
    let parent = repo.add("parent", &["--type", "epic"]);
    let child = repo.add("child", &["--parent", &parent]);
    let shown = repo.ok_json(&["show", &parent, "--json"]);
    let key = shown["data"]["key"].as_str().unwrap().to_string();
    let listed = repo.ok_json(&["list", "--parent", &key, "--json"]);
    assert_eq!(listed["data"]["count"], 1, "{listed}");
    assert_eq!(listed["data"]["issues"][0]["id"], child);
}

#[test]
fn ready_claim_skips_a_candidate_a_claim_policy_refuses() {
    let repo = TestRepo::new();
    repo.init();
    write_policy(
        &repo,
        "\n[[policy.rules]]\nname = \"needs-brief\"\ntransition = \"claim\"\nlabels = [\"gated\"]\nrequire_fields = [\"verification\"]\n",
    );
    let gated = repo.add("gated", &["-p", "highest", "--label", "gated"]);
    let plain = repo.add("plain", &["-p", "low"]);
    let value = repo.ok_json(&["ready", "--claim", "--by", "devin", "--json"]);
    assert_eq!(value["data"]["id"], plain, "{value}");
    let shown = repo.ok_json(&["show", &gated, "--json"]);
    assert_eq!(shown["data"]["status"], "ready");
}

#[test]
fn answering_an_ask_returns_the_issue_to_its_holder() {
    let repo = TestRepo::new();
    repo.init();
    let id = repo.add("held", &[]);
    repo.ok_json(&["claim", &id, "--by", "devin", "--json"]);
    repo.ok_json(&["ask", &id, "which one?", "--by", "devin", "--json"]);
    repo.ok_json(&["answer", &id, "that one", "--by", "alice", "--json"]);
    let shown = repo.ok_json(&["show", &id, "--json"]);
    assert_eq!(shown["data"]["status"], "in_progress", "{shown}");
    assert_eq!(shown["data"]["assignee"], "devin", "{shown}");
}

#[test]
fn changes_limit_pages_without_losing_events() {
    let repo = TestRepo::new();
    repo.init();
    let first = repo.ok_json(&["changes", "--json"]);
    let cursor = first["data"]["cursor"].as_str().unwrap().to_string();
    for title in ["a", "b", "c"] {
        repo.add(title, &[]);
    }
    let all = repo.ok_json(&["changes", "--since", &cursor, "--json"]);
    let total = all["data"]["count"].as_u64().unwrap();
    let mut seen = 0;
    let mut cursor = cursor;
    for _ in 0..10 {
        let page = repo.ok_json(&["changes", "--since", &cursor, "--limit", "1", "--json"]);
        seen += page["data"]["count"].as_u64().unwrap();
        cursor = page["data"]["cursor"].as_str().unwrap().to_string();
        if page["data"]["truncated"] == false {
            break;
        }
    }
    assert_eq!(seen, total);
}

#[test]
fn cursor_is_bound_to_its_query() {
    let repo = TestRepo::new();
    repo.init();
    for title in ["one", "two", "three"] {
        repo.add(title, &[]);
    }
    let page = repo.ok_json(&["list", "--limit", "1", "--json"]);
    let cursor = page["data"]["next_cursor"].as_str().unwrap().to_string();
    let (_, error) = repo.err_json(&["list", "--status", "ready", "--cursor", &cursor, "--json"]);
    assert_eq!(error["error"]["code"], "INVALID_ARGUMENT", "{error}");
    repo.ok_json(&["list", "--cursor", &cursor, "--json"]);
}

#[test]
fn search_matches_quoted_phrases_and_rejects_repeated_repository() {
    let repo = TestRepo::new();
    repo.init();
    repo.add("fix login page crash", &[]);
    repo.add("login redesign", &[]);
    let hit = repo.ok_json(&["search", "\"login page\"", "--json"]);
    assert_eq!(hit["data"]["count"], 1, "{hit}");
    let (_, error) = repo.err_json(&["search", "repository:a repository:b", "--json"]);
    assert_eq!(error["error"]["code"], "INVALID_ARGUMENT", "{error}");
}

#[test]
fn prime_budget_holds_for_the_json_payload() {
    let repo = TestRepo::new();
    repo.init();
    for index in 0..12 {
        repo.add(&format!("a reasonably long task title number {index}"), &[]);
    }
    let value = repo.ok_json(&["prime", "--max-tokens", "1500", "--json"]);
    let payload = serde_json::to_string(&serde_json::json!({
        "instructions": value["data"]["instructions"],
        "counts": value["data"]["counts"],
        "ready": value["data"]["ready"],
        "close_candidates": value["data"]["close_candidates"],
        "stale": value["data"]["stale"],
    }))
    .unwrap();
    assert!(
        payload.chars().count().div_ceil(4) <= 1500,
        "{}",
        payload.len()
    );
}

#[test]
fn ready_rejects_a_zero_limit() {
    let repo = TestRepo::new();
    repo.init();
    let (_, error) = repo.err_json(&["ready", "--limit", "0", "--json"]);
    assert_eq!(error["error"]["code"], "INVALID_ARGUMENT", "{error}");
}

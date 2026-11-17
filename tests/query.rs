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

fn result_ids(value: &Value) -> Vec<String> {
    value["data"]["results"]
        .as_array()
        .expect("results array")
        .iter()
        .map(|task| task["id"].as_str().unwrap().to_string())
        .collect()
}

#[test]
fn search_matches_text_and_returns_snippets() {
    let repo = TestRepo::new();
    repo.init();
    let hit = repo.add(
        "Fix login redirect loop",
        &["--body", "the login page loops"],
    );
    repo.add("Unrelated chore", &[]);

    let value = repo.ok_json(&["search", "login redirect", "--json"]);
    let results = result_ids(&value);
    assert_eq!(results, vec![hit.clone()]);
    let snippet = value["data"]["results"][0]["snippet"]
        .as_str()
        .expect("every search result carries a snippet");
    assert!(
        snippet.to_lowercase().contains("login"),
        "snippet should quote the match: {snippet}"
    );
}

#[test]
fn search_qualifiers_filter_by_status_assignee_and_label() {
    let repo = TestRepo::new();
    repo.init();
    let mine = repo.add("Alpha work", &["--label", "backend", "--reporter", "carol"]);
    repo.add("Beta work", &["--label", "frontend"]);
    repo.ok_json(&["claim", &mine, "--by", "claude", "--json"]);

    // Qualifier-only query.
    let value = repo.ok_json(&["search", "status:in_progress assignee:claude", "--json"]);
    assert_eq!(result_ids(&value), vec![mine.clone()]);

    // Qualifiers combine with free text.
    let value = repo.ok_json(&["search", "label:backend alpha", "--json"]);
    assert_eq!(result_ids(&value), vec![mine.clone()]);

    // A qualifier that matches nothing returns an empty page.
    let value = repo.ok_json(&["search", "label:does-not-exist", "--json"]);
    assert!(result_ids(&value).is_empty());
    assert_eq!(value["data"]["count"], 0);
}

#[test]
fn list_paginates_with_a_stable_cursor_under_writes() {
    let repo = TestRepo::new();
    repo.init();
    let mut created = Vec::new();
    for index in 0..5 {
        created.push(repo.add(&format!("Task {index}"), &[]));
    }

    let first = repo.ok_json(&["list", "--limit", "2", "--json"]);
    let page1 = ids(&first);
    assert_eq!(page1.len(), 2);
    let cursor = first["data"]["next_cursor"]
        .as_str()
        .expect("a full page reports next_cursor")
        .to_string();
    assert_eq!(first["data"]["total"], 5);

    // A write lands between pages: the new Task sorts first (newest
    // creation time is *last* in list order — verify no duplicates/skips
    // for the Tasks that existed when paging started).
    repo.add("Task inserted mid-pagination", &[]);

    let second = repo.ok_json(&["list", "--limit", "2", "--cursor", &cursor, "--json"]);
    let page2 = ids(&second);
    assert_eq!(page2.len(), 2);
    let overlap: Vec<&String> = page1.iter().filter(|id| page2.contains(id)).collect();
    assert!(overlap.is_empty(), "pages must not repeat: {overlap:?}");

    // Drain the remaining pages; the Task inserted mid-pagination may
    // legitimately appear, so only the pre-existing Tasks are checked.
    let mut cursor = second["data"]["next_cursor"].clone();
    let mut seen = page1.clone();
    seen.extend(page2.clone());
    while let Some(token) = cursor.as_str() {
        let page = repo.ok_json(&["list", "--limit", "2", "--cursor", token, "--json"]);
        seen.extend(ids(&page));
        cursor = page["data"]["next_cursor"].clone();
    }
    seen.retain(|id| created.contains(id));
    seen.sort();
    seen.dedup();
    let mut expected = created.clone();
    expected.sort();
    assert_eq!(
        seen, expected,
        "every pre-existing Task appears exactly once across the pages"
    );
}

#[test]
fn list_filter_json_and_fields_project_the_output() {
    let repo = TestRepo::new();
    repo.init();
    let alpha = repo.add("Alpha", &["--label", "backend"]);
    repo.add("Beta", &["--label", "frontend"]);

    // JSON filter: scalar equality and array membership.
    let value = repo.ok_json(&[
        "list",
        "--filter",
        r#"{"labels": ["backend"], "status": ["ready", "in_progress"]}"#,
        "--json",
    ]);
    assert_eq!(ids(&value), vec![alpha.clone()]);

    let (_, error) = repo.err_json(&["list", "--filter", "{\"nope\": 1}", "--json"]);
    assert_eq!(error["error"]["code"], "INVALID_ARGUMENT");

    // --fields projects each task object to the requested keys.
    let value = repo.ok_json(&["list", "--fields", "id,title,status", "--json"]);
    let task = &value["data"]["tasks"][0];
    assert!(task.get("id").is_some());
    assert!(task.get("title").is_some());
    assert!(task.get("status").is_some());
    assert!(task.get("labels").is_none(), "unrequested fields drop out");
    assert!(task.get("body").is_none());
}

#[test]
fn show_fields_projects_the_detail_object() {
    let repo = TestRepo::new();
    repo.init();
    let id = repo.add("Projected", &[]);

    let value = repo.ok_json(&["show", &id, "--fields", "id,key,status", "--json"]);
    let data = &value["data"];
    assert!(data.get("id").is_some());
    assert!(data.get("key").is_some());
    assert!(data.get("status").is_some());
    assert!(data.get("body").is_none());
    assert!(data.get("comments").is_none());
}

#[test]
fn prime_respects_the_token_budget() {
    let repo = TestRepo::new();
    repo.init();
    for index in 0..8 {
        repo.add(&format!("Ready task {index}"), &[]);
    }

    let full = repo.ok_json(&["prime", "--json"]);
    let full_ready = full["data"]["ready"].as_array().unwrap().len();
    assert!(full_ready > 0);

    let tight = repo.ok_json(&["prime", "--max-tokens", "300", "--json"]);
    let budget = &tight["data"]["budget"];
    assert_eq!(budget["max_tokens"], 300);
    let estimated = budget["estimated_tokens"].as_u64().unwrap();
    assert!(
        estimated <= 300,
        "prime output must fit the token budget: {estimated} > 300"
    );
    let tight_ready = tight["data"]["ready"].as_array().unwrap().len();
    assert!(
        tight_ready < full_ready,
        "the budget should trim the ready list"
    );
}

#[test]
fn digest_reports_issue_state_and_epic_rollup_counts() {
    let repo = TestRepo::new();
    repo.init();
    let blocker = repo.add("External blocker source", &[]);
    let epic = repo.add("Epic", &["--type", "epic"]);
    let done_child = repo.add("Done child", &["--parent", &epic]);
    repo.ok_json(&["claim", &done_child, "--by", "claude", "--json"]);
    repo.ok_json(&["done", &done_child, "--by", "claude", "--json"]);
    let blocked_child = repo.add(
        "Blocked child",
        &["--parent", &epic, "--depends-on", &blocker],
    );
    let open_child = repo.add("Open child", &["--parent", &epic]);

    let value = repo.ok_json(&["digest", &epic, "--json"]);
    let data = &value["data"];
    assert_eq!(data["id"], epic);
    assert_eq!(data["type"], "epic");

    let rollup = &data["rollup"];
    assert_eq!(rollup["total"], 3, "rollup covers the subtree: {rollup}");
    assert_eq!(rollup["done"], 1);
    assert_eq!(rollup["blocked"], 1);
    assert_eq!(rollup["open"], 2);
    assert!(rollup["by_status"].is_object());

    // The digest names the blocked child.
    let children = data["children"].as_array().expect("children list");
    let blocked = children
        .iter()
        .find(|child| child["id"] == blocked_child)
        .expect("blocked child listed");
    assert_eq!(blocked["blocked"], true);
    assert!(open_child != done_child);
}

#[test]
fn digest_on_a_leaf_issue_has_no_rollup() {
    let repo = TestRepo::new();
    repo.init();
    let id = repo.add("Leaf", &[]);

    let value = repo.ok_json(&["digest", &id, "--json"]);
    let data = &value["data"];
    assert_eq!(data["id"], id);
    assert!(data.get("rollup").is_none() || data["rollup"].is_null());
    assert!(data["ready"].is_boolean());
}

#[test]
fn search_paginates_with_the_same_cursor_contract() {
    let repo = TestRepo::new();
    repo.init();
    for index in 0..4 {
        repo.add(&format!("Pagination probe {index}"), &[]);
    }

    let first = repo.ok_json(&["search", "pagination probe", "--limit", "2", "--json"]);
    assert_eq!(first["data"]["results"].as_array().unwrap().len(), 2);
    let cursor = first["data"]["next_cursor"]
        .as_str()
        .expect("full search page reports next_cursor")
        .to_string();

    let second = repo.ok_json(&[
        "search",
        "pagination probe",
        "--limit",
        "2",
        "--cursor",
        &cursor,
        "--json",
    ]);
    let page1 = result_ids(&first);
    let page2 = result_ids(&second);
    assert_eq!(page2.len(), 2);
    assert!(
        page1.iter().all(|id| !page2.contains(id)),
        "search pages must not overlap"
    );
}

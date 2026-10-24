mod common;

use common::TestRepo;
use serde_json::Value;

fn ids(tasks: &Value) -> Vec<&str> {
    tasks
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|task| task["id"].as_str())
        .collect()
}

fn exported<'a>(value: &'a Value, id: &str) -> &'a Value {
    value["data"]["tasks"]
        .as_array()
        .unwrap()
        .iter()
        .find(|task| task["id"] == id)
        .unwrap_or_else(|| panic!("{id} not in export: {value}"))
}

#[test]
fn export_is_deterministic_and_includes_edges_comments_and_terminal_work() {
    let repo = TestRepo::new();
    repo.init();
    let parent = repo.add("Feature", &[]);
    let child = repo.ok_json(&[
        "add",
        "Part",
        "--parent",
        parent.as_str(),
        "--label",
        "repo:api",
        "--json",
    ])["data"]["id"]
        .as_str()
        .unwrap()
        .to_string();
    let done_task = repo.add("Done thing", &[]);
    repo.ok_json(&["done", &done_task, "--by", "test", "--json"]);
    repo.ok_json(&[
        "comment", "add", &child, "--by", "test", "--body", "note", "--json",
    ]);
    let dependent = repo.add("Downstream", &["--depends-on", child.as_str()]);

    let first = repo.ok_json(&["export", "--json"]);
    let second = repo.ok_json(&["export", "--json"]);

    assert_eq!(first["data"], second["data"]);
    assert_eq!(first["data"]["schema_version"], 2);
    assert_eq!(first["data"]["board"]["branch"], "orbit");
    assert_eq!(first["data"]["board"]["task_count"], 4);
    assert_eq!(first["data"]["board"]["comment_count"], 1);

    let task_ids = ids(&first["data"]["tasks"]);
    let mut sorted = task_ids.clone();
    sorted.sort();
    assert_eq!(task_ids, sorted);

    let parent_json = exported(&first, &parent);
    assert_eq!(parent_json["kind"], "task");
    assert_eq!(parent_json["type"], "task");
    assert!(parent_json["parent"].is_null());
    assert_eq!(parent_json["subtasks"][0], child.as_str());
    assert_eq!(parent_json["subtask_count"], 1);
    assert_eq!(parent_json["resolved_subtask_count"], 0);
    assert_eq!(parent_json["open_subtask_count"], 1);
    assert_eq!(parent_json["eligible_for_close"], false);

    let child_json = exported(&first, &child);
    assert_eq!(child_json["kind"], "subtask");
    assert_eq!(child_json["parent"], parent.as_str());
    assert_eq!(child_json["labels"][0], "repo:api");
    assert_eq!(child_json["comments"][0]["body"], "note");
    assert_eq!(child_json["comments"][0]["author"], "test");

    let done_json = exported(&first, &done_task);
    assert_eq!(done_json["status"], "done");

    let dependent_json = exported(&first, &dependent);
    assert_eq!(dependent_json["depends_on"][0], child.as_str());
    assert_eq!(dependent_json["ready"], false);
}

#[test]
fn repo_and_label_filters_scope_list_ready_and_prime() {
    let repo = TestRepo::new();
    repo.init();
    let api = repo.add(
        "API task",
        &["--label", "repo:api", "--label", "ready-for-agent"],
    );
    let web = repo.add(
        "Web task",
        &["--label", "repo:web", "--label", "ready-for-agent"],
    );
    let plain = repo.add("Plain task", &[]);
    let bug = repo.add("API bug", &["--label", "repo:api", "--label", "bug"]);

    let list = repo.ok_json(&["list", "--repo", "api", "--json"]);
    assert_eq!(list["data"]["count"], 2);
    let listed = ids(&list["data"]["tasks"]);
    assert!(listed.contains(&api.as_str()));
    assert!(listed.contains(&bug.as_str()));
    assert!(!listed.contains(&web.as_str()));
    assert!(!listed.contains(&plain.as_str()));

    let list = repo.ok_json(&[
        "list",
        "--label",
        "repo:api",
        "--label",
        "ready-for-agent",
        "--json",
    ]);
    assert_eq!(list["data"]["count"], 1);
    assert_eq!(list["data"]["tasks"][0]["id"], api.as_str());

    let list = repo.ok_json(&[
        "list", "--label", "repo:api", "--label", "missing", "--json",
    ]);
    assert_eq!(list["data"]["count"], 0);

    let ready = repo.ok_json(&["ready", "--repo", "api", "--json"]);
    assert_eq!(ready["data"]["count"], 2);
    let ready_ids = ids(&ready["data"]["tasks"]);
    assert!(ready_ids.contains(&api.as_str()));
    assert!(ready_ids.contains(&bug.as_str()));

    let ready = repo.ok_json(&["ready", "--label", "ready-for-agent", "--json"]);
    assert_eq!(ready["data"]["count"], 2);
    let ready_ids = ids(&ready["data"]["tasks"]);
    assert!(ready_ids.contains(&api.as_str()));
    assert!(ready_ids.contains(&web.as_str()));

    let ready = repo.ok_json(&[
        "ready",
        "--label",
        "repo:api",
        "--label",
        "ready-for-agent",
        "--json",
    ]);
    assert_eq!(ready["data"]["count"], 1);
    assert_eq!(ready["data"]["tasks"][0]["id"], api.as_str());

    let prime = repo.ok_json(&["prime", "--repo", "api", "--json"]);
    assert_eq!(prime["data"]["labels"], serde_json::json!(["repo:api"]));
    assert_eq!(prime["data"]["counts"]["total"], 2);
    assert_eq!(prime["data"]["counts"]["ready"], 2);
    let prime_ids = ids(&prime["data"]["ready"]);
    assert!(prime_ids.contains(&api.as_str()));
    assert!(prime_ids.contains(&bug.as_str()));
    assert!(!prime_ids.contains(&web.as_str()));
    assert!(!prime_ids.contains(&plain.as_str()));

    let prime = repo.ok_json(&["prime", "--label", "repo:api", "--json"]);
    assert_eq!(prime["data"]["labels"], serde_json::json!(["repo:api"]));
    assert_eq!(prime["data"]["counts"]["total"], 2);
}

#[test]
fn export_of_an_empty_board_has_the_schema_envelope() {
    let repo = TestRepo::new();
    repo.init();

    let value = repo.ok_json(&["export", "--json"]);

    assert_eq!(value["data"]["schema_version"], 2);
    assert_eq!(value["data"]["board"]["branch"], "orbit");
    assert_eq!(value["data"]["board"]["task_count"], 0);
    assert_eq!(value["data"]["board"]["comment_count"], 0);
    assert!(value["data"]["tasks"].as_array().unwrap().is_empty());
}

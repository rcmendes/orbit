mod common;

use common::TestRepo;
use serde_json::json;

/// Insert YAML frontmatter lines right after the opening `---` of a Task file.
fn insert_frontmatter(repo: &TestRepo, id: &str, yaml: &str) {
    let content = repo.read_task(id);
    let rest = content
        .strip_prefix("---\n")
        .expect("task file starts with frontmatter");
    repo.write_task(id, &format!("---\n{yaml}{rest}"));
}

fn with_property(repo: &TestRepo) -> String {
    let id = repo.add("With property", &[]);
    insert_frontmatter(repo, &id, "properties:\n  ci:\n    passed: true\n");
    id
}

#[test]
fn properties_appear_in_show_list_and_export() {
    let repo = TestRepo::new();
    repo.init();
    let id = with_property(&repo);

    let show = repo.ok_json(&["show", &id, "--json"]);
    assert_eq!(show["data"]["properties"], json!({"ci": {"passed": true}}));

    let list = repo.ok_json(&["list", "--json"]);
    assert_eq!(
        list["data"]["tasks"][0]["properties"],
        json!({"ci": {"passed": true}})
    );

    let export = repo.ok_json(&["export", "--json"]);
    assert_eq!(export["data"]["schema_version"], 5);
    assert_eq!(
        export["data"]["tasks"][0]["properties"],
        json!({"ci": {"passed": true}})
    );
}

#[test]
fn task_without_properties_reports_empty_object() {
    let repo = TestRepo::new();
    repo.init();
    let id = repo.add("Plain", &[]);
    let show = repo.ok_json(&["show", &id, "--json"]);
    assert_eq!(show["data"]["properties"], json!({}));
    assert!(!repo.read_task(&id).contains("properties"));
}

#[test]
fn human_show_prints_one_line_per_owner() {
    let repo = TestRepo::new();
    repo.init();
    let id = with_property(&repo);
    let output = repo.orbit(&["show", &id]);
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("property ci: {\"passed\":true}"),
        "{stdout}"
    );
}

#[test]
fn properties_do_not_affect_ready() {
    let repo = TestRepo::new();
    repo.init();
    let id = with_property(&repo);
    let ready = repo.ok_json(&["ready", "--json"]);
    assert_eq!(ready["data"]["count"], 1);
    assert_eq!(ready["data"]["tasks"][0]["id"], id);
}

#[test]
fn doctor_reports_non_object_property() {
    let repo = TestRepo::new();
    repo.init();
    let id = repo.add("Broken", &[]);
    insert_frontmatter(&repo, &id, "properties:\n  ci:\n  - 1\n");
    let (status, value) = repo.json(&["doctor", "--json"]);
    assert!(status == 0 || status == 1, "{value}");
    let codes: Vec<&str> = value["data"]["issues"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|issue| issue["code"].as_str())
        .collect();
    assert!(codes.contains(&"INVALID_PROPERTY"), "{value}");
}

#[test]
fn properties_survive_slug_migration() {
    let repo = TestRepo::new();
    repo.init();
    with_property(&repo);
    repo.ok_json(&["slug", "migrate", "atlas", "--json"]);
    let list = repo.ok_json(&["list", "--json"]);
    let task = &list["data"]["tasks"][0];
    assert!(task["id"].as_str().unwrap().starts_with("atlas-"), "{task}");
    assert_eq!(task["properties"], json!({"ci": {"passed": true}}));
}

#[test]
fn unknown_frontmatter_round_trip_keeps_properties() {
    let repo = TestRepo::new();
    repo.init();
    let id = with_property(&repo);
    repo.ok_json(&["label", "add", &id, "bug", "--json"]);
    assert!(
        repo.read_task(&id)
            .contains("properties:\n  ci:\n    passed: true\n"),
        "{}",
        repo.read_task(&id)
    );
    let show = repo.ok_json(&["show", &id, "--json"]);
    assert_eq!(show["data"]["properties"], json!({"ci": {"passed": true}}));
}

fn board_commits(repo: &TestRepo) -> usize {
    let output = repo.git_in(&repo.board_path(), &["rev-list", "--count", "HEAD"]);
    String::from_utf8_lossy(&output.stdout)
        .trim()
        .parse()
        .unwrap()
}

fn last_board_message(repo: &TestRepo) -> String {
    let output = repo.git_in(&repo.board_path(), &["log", "-1", "--format=%s"]);
    String::from_utf8_lossy(&output.stdout).trim().to_string()
}

#[test]
fn property_set_replaces_the_owner_value() {
    let repo = TestRepo::new();
    repo.init();
    let id = repo.add("Task", &[]);
    repo.ok_json(&[
        "property",
        "set",
        &id,
        "ci",
        "--value",
        r#"{"a":1}"#,
        "--json",
    ]);
    let value = repo.ok_json(&[
        "property",
        "set",
        &id,
        "ci",
        "--value",
        r#"{"b":2}"#,
        "--json",
    ]);
    assert_eq!(value["data"]["property"], json!({"b": 2}));
    assert_eq!(value["data"]["changed"], true);
    let show = repo.ok_json(&["show", &id, "--json"]);
    assert_eq!(show["data"]["properties"], json!({"ci": {"b": 2}}));
}

#[test]
fn property_set_keeps_other_owners() {
    let repo = TestRepo::new();
    repo.init();
    let id = repo.add("Task", &[]);
    repo.ok_json(&[
        "property",
        "set",
        &id,
        "ci",
        "--value",
        r#"{"a":1}"#,
        "--json",
    ]);
    repo.ok_json(&[
        "property",
        "set",
        &id,
        "planner",
        "--value",
        r#"{"p":true}"#,
        "--json",
    ]);
    let show = repo.ok_json(&["show", &id, "--json"]);
    assert_eq!(
        show["data"]["properties"],
        json!({"ci": {"a": 1}, "planner": {"p": true}})
    );
}

#[test]
fn property_remove_deletes_the_owner() {
    let repo = TestRepo::new();
    repo.init();
    let id = repo.add("Task", &[]);
    repo.ok_json(&[
        "property",
        "set",
        &id,
        "ci",
        "--value",
        r#"{"a":1}"#,
        "--json",
    ]);
    let removed = repo.ok_json(&["property", "remove", &id, "ci", "--json"]);
    assert_eq!(removed["data"]["changed"], true);
    assert_eq!(removed["data"]["property"], json!(null));
    let show = repo.ok_json(&["show", &id, "--json"]);
    assert_eq!(show["data"]["properties"], json!({}));
    let again = repo.ok_json(&["property", "remove", &id, "ci", "--json"]);
    assert_eq!(again["data"]["changed"], false);
}

#[test]
fn property_set_rejects_bad_input() {
    let repo = TestRepo::new();
    repo.init();
    let id = repo.add("Task", &[]);
    let before = repo.read_task(&id);
    let big = format!(r#"{{"x":"{}"}}"#, "a".repeat(9000));
    for (owner, value) in [("CI", r#"{"a":1}"#), ("ci", "[1]"), ("ci", big.as_str())] {
        let (_, error) =
            repo.err_json(&["property", "set", &id, owner, "--value", value, "--json"]);
        assert_eq!(
            error["error"]["code"], "INVALID_PROPERTY",
            "{owner}: {error}"
        );
    }
    assert_eq!(repo.read_task(&id), before);
}

#[test]
fn property_set_makes_one_commit() {
    let repo = TestRepo::new();
    repo.init();
    let id = repo.add("Task", &[]);
    let before = board_commits(&repo);
    repo.ok_json(&[
        "property",
        "set",
        &id,
        "ci",
        "--value",
        r#"{"a":1}"#,
        "--json",
    ]);
    assert_eq!(board_commits(&repo), before + 1);
    assert_eq!(
        last_board_message(&repo),
        format!("orbit: property {id} ci")
    );
}

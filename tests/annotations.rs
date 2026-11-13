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

fn annotated(repo: &TestRepo) -> String {
    let id = repo.add("Annotated", &[]);
    insert_frontmatter(repo, &id, "annotations:\n  ci:\n    passed: true\n");
    id
}

#[test]
fn annotations_appear_in_show_list_and_export() {
    let repo = TestRepo::new();
    repo.init();
    let id = annotated(&repo);

    let show = repo.ok_json(&["show", &id, "--json"]);
    assert_eq!(show["data"]["annotations"], json!({"ci": {"passed": true}}));

    let list = repo.ok_json(&["list", "--json"]);
    assert_eq!(
        list["data"]["tasks"][0]["annotations"],
        json!({"ci": {"passed": true}})
    );

    let export = repo.ok_json(&["export", "--json"]);
    assert_eq!(export["data"]["schema_version"], 3);
    assert_eq!(
        export["data"]["tasks"][0]["annotations"],
        json!({"ci": {"passed": true}})
    );
}

#[test]
fn task_without_annotations_reports_empty_object() {
    let repo = TestRepo::new();
    repo.init();
    let id = repo.add("Plain", &[]);
    let show = repo.ok_json(&["show", &id, "--json"]);
    assert_eq!(show["data"]["annotations"], json!({}));
    assert!(!repo.read_task(&id).contains("annotations"));
}

#[test]
fn human_show_prints_one_line_per_owner() {
    let repo = TestRepo::new();
    repo.init();
    let id = annotated(&repo);
    let output = repo.orbit(&["show", &id]);
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("annotation ci: {\"passed\":true}"),
        "{stdout}"
    );
}

#[test]
fn annotations_do_not_affect_ready() {
    let repo = TestRepo::new();
    repo.init();
    let id = annotated(&repo);
    let ready = repo.ok_json(&["ready", "--json"]);
    assert_eq!(ready["data"]["count"], 1);
    assert_eq!(ready["data"]["tasks"][0]["id"], id);
}

#[test]
fn doctor_reports_non_object_annotation() {
    let repo = TestRepo::new();
    repo.init();
    let id = repo.add("Broken", &[]);
    insert_frontmatter(&repo, &id, "annotations:\n  ci:\n  - 1\n");
    let (status, value) = repo.json(&["doctor", "--json"]);
    assert!(status == 0 || status == 1, "{value}");
    let codes: Vec<&str> = value["data"]["issues"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|issue| issue["code"].as_str())
        .collect();
    assert!(codes.contains(&"INVALID_ANNOTATION"), "{value}");
}

#[test]
fn annotations_survive_slug_migration() {
    let repo = TestRepo::new();
    repo.init();
    annotated(&repo);
    repo.ok_json(&["slug", "migrate", "atlas", "--json"]);
    let list = repo.ok_json(&["list", "--json"]);
    let task = &list["data"]["tasks"][0];
    assert!(task["id"].as_str().unwrap().starts_with("atlas-"), "{task}");
    assert_eq!(task["annotations"], json!({"ci": {"passed": true}}));
}

#[test]
fn unknown_frontmatter_round_trip_keeps_annotations() {
    let repo = TestRepo::new();
    repo.init();
    let id = annotated(&repo);
    repo.ok_json(&["label", "add", &id, "bug", "--json"]);
    assert!(
        repo.read_task(&id)
            .contains("annotations:\n  ci:\n    passed: true\n"),
        "{}",
        repo.read_task(&id)
    );
    let show = repo.ok_json(&["show", &id, "--json"]);
    assert_eq!(show["data"]["annotations"], json!({"ci": {"passed": true}}));
}

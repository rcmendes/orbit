mod common;

use common::TestRepo;

#[test]
fn type_defaults_to_task_and_is_serialized() {
    let repo = TestRepo::new();
    repo.init();

    let id = repo.add("Default type", &[]);
    let content = repo.read_task(&id);
    assert!(content.contains("type: task"), "{content}");

    let show = repo.ok_json(&["show", id.as_str(), "--json"]);
    assert_eq!(show["data"]["type"], "task");
    assert_eq!(show["data"]["kind"], "task");

    // A pre-type file still loads; the next write adds the explicit field.
    let legacy = content.replace("type: task\n", "");
    assert!(!legacy.contains("type: task"));
    repo.write_task(&id, &legacy);
    let show = repo.ok_json(&["show", id.as_str(), "--json"]);
    assert_eq!(show["data"]["type"], "task");
    repo.ok_json(&["update", id.as_str(), "--priority", "1", "--json"]);
    assert!(repo.read_task(&id).contains("type: task"));
}

#[test]
fn epic_is_top_level_only() {
    let repo = TestRepo::new();
    repo.init();

    let parent = repo.add("Feature", &[]);
    let child = repo.add("Part", &["--parent", parent.as_str()]);
    let epic = repo.add("Epic", &["--type", "epic"]);

    let (status, value) = repo.json(&[
        "add",
        "Nested epic",
        "--parent",
        parent.as_str(),
        "--type",
        "epic",
        "--json",
    ]);
    assert_eq!(status, 1);
    assert_eq!(value["error"]["code"], "INVALID_TYPE");

    let (status, value) = repo.json(&["update", child.as_str(), "--type", "epic", "--json"]);
    assert_eq!(status, 1);
    assert_eq!(value["error"]["code"], "INVALID_TYPE");

    let show = repo.ok_json(&["show", epic.as_str(), "--json"]);
    assert_eq!(show["data"]["type"], "epic");
    assert_eq!(show["data"]["kind"], "task");
    assert_eq!(show["data"]["parent"], serde_json::Value::Null);

    let updated = repo.ok_json(&["update", epic.as_str(), "--type", "spike", "--json"]);
    assert_eq!(updated["data"]["type"], "spike");
}

#[test]
fn type_filters_scope_list_and_ready() {
    let repo = TestRepo::new();
    repo.init();

    let task = repo.add("Task", &[]);
    let epic = repo.add("Epic", &["--type", "epic"]);
    let spike = repo.add("Spike", &["--type", "spike"]);
    let chore = repo.add("Chore", &["--type", "chore"]);
    let decision = repo.add("Decision", &["--type", "decision"]);

    let list = repo.ok_json(&["list", "--type", "epic", "--type", "spike", "--json"]);
    assert_eq!(list["data"]["count"], 2);
    let ids: Vec<&str> = list["data"]["tasks"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|entry| entry["id"].as_str())
        .collect();
    assert!(ids.contains(&epic.as_str()));
    assert!(ids.contains(&spike.as_str()));

    let list = repo.ok_json(&["list", "--exclude-type", "task", "--json"]);
    assert_eq!(list["data"]["count"], 4);
    let ids: Vec<&str> = list["data"]["tasks"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|entry| entry["id"].as_str())
        .collect();
    assert!(!ids.contains(&task.as_str()));
    assert!(ids.contains(&chore.as_str()));
    assert!(ids.contains(&decision.as_str()));

    let ready = repo.ok_json(&["ready", "--exclude-type", "epic", "--json"]);
    let ids: Vec<&str> = ready["data"]["tasks"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|entry| entry["id"].as_str())
        .collect();
    assert!(!ids.contains(&epic.as_str()));
    assert!(ids.contains(&spike.as_str()));

    let ready = repo.ok_json(&["ready", "--type", "decision", "--json"]);
    assert_eq!(ready["data"]["count"], 1);
    assert_eq!(ready["data"]["tasks"][0]["id"], decision.as_str());
}

#[test]
fn human_output_marks_non_task_types() {
    let repo = TestRepo::new();
    repo.init();

    let epic = repo.add("Epic", &["--type", "epic"]);
    let output = repo.orbit(&["list"]);
    assert!(output.status.success());
    let human = String::from_utf8_lossy(&output.stdout);
    assert!(human.lines().next().unwrap().contains("TYPE"), "{human}");
    let row = human.lines().nth(1).unwrap();
    let cells: Vec<&str> = row.split_whitespace().collect();
    assert_eq!(cells[3], "epic", "{human}");
    assert_eq!(cells[4], "-", "{human}");
    assert_eq!(cells[5], "Epic", "{human}");

    let output = repo.orbit(&["show", &epic]);
    assert!(output.status.success());
    let human = String::from_utf8_lossy(&output.stdout);
    assert!(human.contains("type: epic"), "{human}");
}

#[test]
fn doctor_reports_epic_subtask() {
    let repo = TestRepo::new();
    repo.init();

    let parent = repo.add("Feature", &[]);
    let child = repo.add("Part", &["--parent", parent.as_str()]);
    let content = repo.read_task(&child).replace("type: task", "type: epic");
    repo.write_task(&child, &content);

    let (status, value) = repo.json(&["doctor", "--json"]);
    assert_eq!(status, 1);
    assert!(
        value["data"]["issues"]
            .as_array()
            .unwrap()
            .iter()
            .any(|issue| issue["code"] == "INVALID_TYPE" && issue["task"] == child.as_str())
    );
}

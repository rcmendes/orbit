mod common;

use common::TestRepo;

fn subtask(repo: &TestRepo, parent: &str, title: &str) -> String {
    let value = repo.ok_json(&["add", title, "--parent", parent, "--json"]);
    value["data"]["id"].as_str().unwrap().to_string()
}

fn status(repo: &TestRepo, id: &str) -> String {
    let value = repo.ok_json(&["show", id, "--json"]);
    value["data"]["status"].as_str().unwrap().to_string()
}

#[test]
fn add_parent_exposes_kind_parent_and_subtasks() {
    let repo = TestRepo::new();
    repo.init();
    let parent = repo.add("Feature", &[]);
    let child = subtask(&repo, &parent, "Part");

    let parent_json = repo.ok_json(&["show", &parent, "--json"]);
    assert_eq!(parent_json["data"]["kind"], "task");
    assert!(parent_json["data"]["parent"].is_null());
    assert_eq!(parent_json["data"]["subtasks"][0], child.as_str());
    assert_eq!(parent_json["data"]["open_subtasks"][0], child.as_str());

    let child_json = repo.ok_json(&["show", &child, "--json"]);
    assert_eq!(child_json["data"]["kind"], "subtask");
    assert_eq!(child_json["data"]["parent"], parent.as_str());
    assert!(
        child_json["data"]["subtasks"]
            .as_array()
            .unwrap()
            .is_empty()
    );
}

#[test]
fn parent_progress_fields_track_children_and_close_eligibility() {
    let repo = TestRepo::new();
    repo.init();
    let parent = repo.add("Feature", &[]);
    let first = subtask(&repo, &parent, "First");
    let second = subtask(&repo, &parent, "Second");

    let value = repo.ok_json(&["show", &parent, "--json"]);
    assert_eq!(value["data"]["subtask_count"], 2);
    assert_eq!(value["data"]["resolved_subtask_count"], 0);
    assert_eq!(value["data"]["open_subtask_count"], 2);
    assert_eq!(value["data"]["eligible_for_close"], false);

    repo.ok_json(&["done", &first, "--by", "test", "--json"]);
    let value = repo.ok_json(&["show", &parent, "--json"]);
    assert_eq!(value["data"]["resolved_subtask_count"], 1);
    assert_eq!(value["data"]["open_subtask_count"], 1);
    assert_eq!(value["data"]["eligible_for_close"], false);

    repo.ok_json(&["cancel", &second, "--by", "test", "--json"]);
    let value = repo.ok_json(&["show", &parent, "--json"]);
    assert_eq!(value["data"]["subtask_count"], 2);
    assert_eq!(value["data"]["resolved_subtask_count"], 2);
    assert_eq!(value["data"]["open_subtask_count"], 0);
    assert_eq!(value["data"]["eligible_for_close"], true);

    let ready = repo.ok_json(&["ready", "--json"]);
    let parent_json = ready["data"]["tasks"]
        .as_array()
        .unwrap()
        .iter()
        .find(|task| task["id"] == parent.as_str())
        .expect("resolved parent should be ready to close");
    assert_eq!(parent_json["eligible_for_close"], true);

    repo.ok_json(&["done", &parent, "--by", "test", "--json"]);
    let value = repo.ok_json(&["show", &parent, "--json"]);
    assert_eq!(value["data"]["status"], "done");
    assert_eq!(value["data"]["eligible_for_close"], false);

    let ordinary = repo.add("Ordinary", &[]);
    let value = repo.ok_json(&["show", &ordinary, "--json"]);
    assert_eq!(value["data"]["subtask_count"], 0);
    assert_eq!(value["data"]["resolved_subtask_count"], 0);
    assert_eq!(value["data"]["open_subtask_count"], 0);
    assert_eq!(value["data"]["eligible_for_close"], false);
}

#[test]
fn tree_shows_the_parent_children_and_blockers() {
    let repo = TestRepo::new();
    repo.init();
    let parent = repo.add("Feature", &[]);
    let blocker = repo.add("Blocker", &[]);
    let first = subtask(&repo, &parent, "First");
    let second = repo.ok_json(&[
        "add",
        "Second",
        "--parent",
        &parent,
        "--depends-on",
        &blocker,
        "--json",
    ])["data"]["id"]
        .as_str()
        .unwrap()
        .to_string();

    let tree = repo.ok_json(&["tree", &parent, "--json"]);
    assert_eq!(tree["data"]["root"], parent.as_str());
    assert_eq!(tree["data"]["count"], 3);
    assert_eq!(tree["data"]["tasks"][0]["id"], parent.as_str());

    let ids: Vec<&str> = tree["data"]["tasks"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|task| task["id"].as_str())
        .collect();
    assert!(ids.contains(&first.as_str()));
    assert!(ids.contains(&second.as_str()));

    let second_json = tree["data"]["tasks"]
        .as_array()
        .unwrap()
        .iter()
        .find(|task| task["id"] == second.as_str())
        .unwrap();
    assert_eq!(second_json["blocking"][0], blocker.as_str());

    let output = repo.orbit(&["tree", &parent]);
    assert!(output.status.success());
    let human = String::from_utf8_lossy(&output.stdout);
    assert!(human.contains(&parent));
    assert!(human.contains(&first));
    assert!(human.contains("blocked by"));

    let (status, value) = repo.json(&["tree", &first, "--json"]);
    assert_eq!(status, 1);
    assert_eq!(value["error"]["code"], "INVALID_ARGUMENT");

    let ordinary = repo.add("Ordinary", &[]);
    let tree = repo.ok_json(&["tree", &ordinary, "--json"]);
    assert_eq!(tree["data"]["count"], 1);
    assert_eq!(tree["data"]["tasks"][0]["id"], ordinary.as_str());
}

#[test]
fn subtask_ids_encode_the_parent() {
    let repo = TestRepo::new();
    repo.init();
    let parent = repo.add("Feature", &[]);
    let child = subtask(&repo, &parent, "Part");

    assert!(
        child.starts_with(&format!("{parent}.")),
        "{parent} -> {child}"
    );
    assert_eq!(child.matches('.').count(), 1, "{child}");
    assert_eq!(
        repo.ok_json(&["show", &child, "--json"])["data"]["parent"],
        parent.as_str()
    );
}

#[test]
fn doctor_reports_parent_id_mismatch() {
    let repo = TestRepo::new();
    repo.init();
    let parent = repo.add("Feature", &[]);
    let other = repo.add("Other", &[]);
    let child = subtask(&repo, &parent, "Part");

    let content = repo
        .read_task(&child)
        .replace(&format!("parent: {parent}"), &format!("parent: {other}"));
    repo.write_task(&child, &content);

    let (status, value) = repo.json(&["doctor", "--json"]);
    assert_eq!(status, 1);
    assert_eq!(value["data"]["healthy"], false);
    assert!(
        value["data"]["issues"]
            .as_array()
            .unwrap()
            .iter()
            .any(|issue| issue["code"] == "PARENT_MISMATCH" && issue["task"] == child.as_str())
    );
}

#[test]
fn doctor_reports_subtask_id_without_parent() {
    let repo = TestRepo::new();
    repo.init();
    let parent = repo.add("Feature", &[]);
    let child = subtask(&repo, &parent, "Part");

    let content = repo
        .read_task(&child)
        .replace(&format!("parent: {parent}\n"), "");
    repo.write_task(&child, &content);

    let (status, value) = repo.json(&["doctor", "--json"]);
    assert_eq!(status, 1);
    assert!(
        value["data"]["issues"]
            .as_array()
            .unwrap()
            .iter()
            .any(|issue| issue["code"] == "PARENT_MISMATCH" && issue["task"] == child.as_str())
    );
}

#[test]
fn add_parent_rejects_missing_nested_and_terminal_parents() {
    let repo = TestRepo::new();
    repo.init();

    let (status, value) = repo.json(&["add", "Orphan", "--parent", "orb-missing", "--json"]);
    assert_eq!(status, 1);
    assert_eq!(value["error"]["code"], "NOT_FOUND");

    let parent = repo.add("Feature", &[]);
    let child = subtask(&repo, &parent, "Part");

    let (status, value) = repo.json(&["add", "Nested", "--parent", &child, "--json"]);
    assert_eq!(status, 1);
    assert_eq!(value["error"]["code"], "INVALID_ARGUMENT");
    assert!(
        value["error"]["message"]
            .as_str()
            .unwrap()
            .contains("one level")
    );

    let (status, value) = repo.json(&[
        "add",
        "Cycle",
        "--parent",
        &parent,
        "--depends-on",
        &parent,
        "--json",
    ]);
    assert_eq!(status, 1);
    assert_eq!(value["error"]["code"], "INVALID_ARGUMENT");
    assert!(
        value["error"]["message"]
            .as_str()
            .unwrap()
            .contains("cannot depend on its Parent Task")
    );

    repo.ok_json(&["done", &parent, "--by", "test", "--force", "--json"]);
    let (status, value) = repo.json(&["add", "Late", "--parent", &parent, "--json"]);
    assert_eq!(status, 1);
    assert_eq!(value["error"]["code"], "INVALID_ARGUMENT");
    assert!(
        value["error"]["message"]
            .as_str()
            .unwrap()
            .contains("active")
    );
}

#[test]
fn parent_becomes_ready_only_after_subtasks_resolve() {
    let repo = TestRepo::new();
    repo.init();
    let parent = repo.add("Feature", &[]);
    let child = subtask(&repo, &parent, "Part");

    let ready = repo.ok_json(&["ready", "--json"]);
    let ids: Vec<&str> = ready["data"]["tasks"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|task| task["id"].as_str())
        .collect();
    assert!(ids.contains(&child.as_str()));
    assert!(!ids.contains(&parent.as_str()));

    let children = repo.ok_json(&["ready", "--parent", &parent, "--json"]);
    assert_eq!(children["data"]["count"], 1);
    assert_eq!(children["data"]["tasks"][0]["id"], child.as_str());

    repo.ok_json(&["done", &child, "--by", "test", "--json"]);

    let ready = repo.ok_json(&["ready", "--json"]);
    let ids: Vec<&str> = ready["data"]["tasks"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|task| task["id"].as_str())
        .collect();
    assert!(ids.contains(&parent.as_str()));
    assert!(!ids.contains(&child.as_str()));

    let children = repo.ok_json(&["ready", "--parent", &parent, "--json"]);
    assert_eq!(children["data"]["count"], 0);
}

#[test]
fn open_subtasks_block_claim_done_and_cancel() {
    let repo = TestRepo::new();
    repo.init();
    let parent = repo.add("Feature", &[]);
    let child = subtask(&repo, &parent, "Part");

    for command in ["claim", "done", "cancel"] {
        let (status, value) = repo.json(&[command, &parent, "--by", "test", "--json"]);
        assert_eq!(status, 1, "{command}");
        assert_eq!(value["error"]["code"], "SUBTASKS_OPEN", "{command}");
        assert_eq!(value["error"]["details"]["subtasks"][0], child.as_str());
    }
}

#[test]
fn done_force_cancels_open_subtasks() {
    let repo = TestRepo::new();
    repo.init();
    let parent = repo.add("Feature", &[]);
    let child = subtask(&repo, &parent, "Part");

    let value = repo.ok_json(&["done", &parent, "--by", "test", "--force", "--json"]);

    assert_eq!(status(&repo, &parent), "done");
    assert_eq!(status(&repo, &child), "cancelled");
    let warnings = value["meta"]["warnings"].as_array().unwrap();
    assert!(warnings.iter().any(|warning| {
        warning
            .as_str()
            .unwrap()
            .contains("cancelled open SubTasks")
    }));
}

#[test]
fn done_force_does_not_cancel_subtasks_when_a_dependency_is_unfinished() {
    let repo = TestRepo::new();
    repo.init();
    let blocker = repo.add("Blocker", &[]);
    let parent = repo.add("Feature", &["--depends-on", blocker.as_str()]);
    let child = subtask(&repo, &parent, "Part");

    let (code, value) = repo.json(&["done", &parent, "--by", "test", "--force", "--json"]);

    assert_eq!(code, 1);
    assert_eq!(value["error"]["code"], "DEPENDENCY_BLOCKED");
    assert_eq!(status(&repo, &child), "todo");
}

#[test]
fn cancel_force_cancels_open_subtasks() {
    let repo = TestRepo::new();
    repo.init();
    let parent = repo.add("Feature", &[]);
    let child = subtask(&repo, &parent, "Part");

    repo.ok_json(&["cancel", &parent, "--by", "test", "--force", "--json"]);

    assert_eq!(status(&repo, &parent), "cancelled");
    assert_eq!(status(&repo, &child), "cancelled");
}

#[test]
fn delete_requires_force_and_cascades() {
    let repo = TestRepo::new();
    repo.init();
    let parent = repo.add("Feature", &[]);
    let child = subtask(&repo, &parent, "Part");
    repo.ok_json(&[
        "comment", "add", &child, "--by", "test", "--body", "note", "--json",
    ]);

    let (status, value) = repo.json(&["delete", &parent, "--json"]);
    assert_eq!(status, 1);
    assert_eq!(value["error"]["code"], "SUBTASKS_EXIST");

    let value = repo.ok_json(&["delete", &parent, "--force", "--json"]);
    let deleted: Vec<&str> = value["data"]["deleted_ids"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|id| id.as_str())
        .collect();
    assert!(deleted.contains(&parent.as_str()));
    assert!(deleted.contains(&child.as_str()));
    assert!(!repo.task_path(&parent).exists());
    assert!(!repo.task_path(&child).exists());
    assert!(
        !repo
            .board_path()
            .join("comments")
            .join(format!("{child}.md"))
            .exists()
    );
    let listed = repo.ok_json(&["list", "--all", "--json"]);
    assert_eq!(listed["data"]["count"], 0);
}

#[test]
fn delete_force_cleans_dependents_of_the_subtree() {
    let repo = TestRepo::new();
    repo.init();
    let parent = repo.add("Feature", &[]);
    let child = subtask(&repo, &parent, "Part");
    let downstream = repo.add("Downstream", &["--depends-on", child.as_str()]);

    let value = repo.ok_json(&["delete", &parent, "--force", "--json"]);

    let cleaned: Vec<&str> = value["data"]["cleaned"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|id| id.as_str())
        .collect();
    assert!(cleaned.contains(&downstream.as_str()));

    let downstream_json = repo.ok_json(&["show", &downstream, "--json"]);
    assert!(
        downstream_json["data"]["depends_on"]
            .as_array()
            .unwrap()
            .is_empty()
    );
}

#[test]
fn reopen_subtask_reopens_done_parent() {
    let repo = TestRepo::new();
    repo.init();
    let parent = repo.add("Feature", &[]);
    let child = subtask(&repo, &parent, "Part");

    repo.ok_json(&["done", &child, "--by", "test", "--json"]);
    repo.ok_json(&["done", &parent, "--by", "test", "--json"]);
    assert_eq!(status(&repo, &parent), "done");
    assert_eq!(status(&repo, &child), "done");

    let value = repo.ok_json(&["reopen", &child, "--json"]);

    assert_eq!(status(&repo, &parent), "todo");
    assert_eq!(status(&repo, &child), "todo");
    let warnings = value["meta"]["warnings"].as_array().unwrap();
    assert!(
        warnings
            .iter()
            .any(|warning| warning.as_str().unwrap().contains("reopened parent"))
    );
}

#[test]
fn reopen_subtask_under_cancelled_parent_is_rejected() {
    let repo = TestRepo::new();
    repo.init();
    let parent = repo.add("Feature", &[]);
    let child = subtask(&repo, &parent, "Part");

    repo.ok_json(&["cancel", &parent, "--by", "test", "--force", "--json"]);

    let (code, value) = repo.json(&["reopen", &child, "--json"]);
    assert_eq!(code, 1);
    assert_eq!(value["error"]["code"], "INVALID_TRANSITION");
    assert!(
        value["error"]["message"]
            .as_str()
            .unwrap()
            .contains("cancelled")
    );
    assert_eq!(status(&repo, &child), "cancelled");
}

#[test]
fn depend_rejects_parent_child_edges() {
    let repo = TestRepo::new();
    repo.init();
    let parent = repo.add("Feature", &[]);
    let child = subtask(&repo, &parent, "Part");

    for (id, dep) in [
        (parent.as_str(), child.as_str()),
        (child.as_str(), parent.as_str()),
    ] {
        let (status, value) = repo.json(&["depend", id, dep, "--json"]);
        assert_eq!(status, 1);
        assert_eq!(value["error"]["code"], "INVALID_ARGUMENT");
        assert!(
            value["error"]["message"]
                .as_str()
                .unwrap()
                .contains("--parent")
        );
    }
}

#[test]
fn list_parent_filters_and_tree_indents() {
    let repo = TestRepo::new();
    repo.init();
    let parent = repo.add("Feature", &[]);
    let child = subtask(&repo, &parent, "Part");
    let other = repo.add("Standalone", &[]);

    let filtered = repo.ok_json(&["list", "--parent", &parent, "--json"]);
    assert_eq!(filtered["data"]["count"], 1);
    assert_eq!(filtered["data"]["tasks"][0]["id"], child.as_str());

    let (status, value) = repo.json(&["list", "--parent", &child, "--json"]);
    assert_eq!(status, 1);
    assert_eq!(value["error"]["code"], "INVALID_ARGUMENT");

    let tree = repo.ok_json(&["list", "--tree", "--json"]);
    let ids: Vec<&str> = tree["data"]["tasks"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|task| task["id"].as_str())
        .collect();
    assert!(ids.contains(&parent.as_str()));
    assert!(ids.contains(&child.as_str()));
    assert!(ids.contains(&other.as_str()));

    let output = repo.orbit(&["list", "--tree"]);
    let stdout = String::from_utf8_lossy(&output.stdout);
    let child_line = stdout
        .lines()
        .find(|line| line.contains(&child))
        .unwrap_or_else(|| panic!("no child line in {stdout}"));
    assert!(child_line.starts_with("  "), "{stdout}");
    let parent_line = stdout
        .lines()
        .find(|line| line.contains(&parent))
        .unwrap_or_else(|| panic!("no parent line in {stdout}"));
    assert!(!parent_line.starts_with("  "), "{stdout}");
}

#[test]
fn doctor_reports_orphan_and_self_parents() {
    let repo = TestRepo::new();
    repo.init();
    let orphan = repo.add("Orphan", &[]);
    let content = repo
        .read_task(&orphan)
        .replace("status: todo", "status: todo\nparent: orb-missing");
    repo.write_task(&orphan, &content);

    let (status, value) = repo.json(&["doctor", "--json"]);
    assert_eq!(status, 1);
    assert_eq!(value["ok"], true);
    assert_eq!(value["data"]["healthy"], false);
    assert!(
        value["data"]["issues"]
            .as_array()
            .unwrap()
            .iter()
            .any(|issue| issue["code"] == "DANGLING_PARENT" && issue["task"] == orphan.as_str())
    );

    let repo = TestRepo::new();
    repo.init();
    let task = repo.add("Self", &[]);
    let content = repo
        .read_task(&task)
        .replace("status: todo", &format!("status: todo\nparent: {task}"));
    repo.write_task(&task, &content);

    let (status, value) = repo.json(&["doctor", "--json"]);
    assert_eq!(status, 1);
    assert!(
        value["data"]["issues"]
            .as_array()
            .unwrap()
            .iter()
            .any(|issue| issue["code"] == "NESTED_SUBTASK" && issue["task"] == task.as_str())
    );
}

#[test]
fn show_human_includes_kind_parent_and_subtasks() {
    let repo = TestRepo::new();
    repo.init();
    let parent = repo.add("Feature", &[]);
    let child = subtask(&repo, &parent, "Part");

    let output = repo.orbit(&["show", &parent]);
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("kind: task"), "{stdout}");
    assert!(stdout.contains(&format!("subtasks: {child}")), "{stdout}");

    let output = repo.orbit(&["show", &child]);
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("kind: subtask"), "{stdout}");
    assert!(stdout.contains(&format!("parent: {parent}")), "{stdout}");
}

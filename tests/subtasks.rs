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
fn prime_lists_parents_ready_to_close() {
    let repo = TestRepo::new();
    repo.init();
    let parent = repo.add("Feature", &[]);
    let child = subtask(&repo, &parent, "Part");
    let ordinary = repo.add("Ordinary", &[]);

    let prime = repo.ok_json(&["prime", "--json"]);
    assert!(
        prime["data"]["close_candidates"]
            .as_array()
            .unwrap()
            .is_empty()
    );

    repo.ok_json(&["done", &child, "--by", "test", "--json"]);
    let prime = repo.ok_json(&["prime", "--json"]);
    let candidates = prime["data"]["close_candidates"].as_array().unwrap();
    assert_eq!(candidates.len(), 1);
    assert_eq!(candidates[0]["id"], parent.as_str());
    assert_eq!(candidates[0]["eligible_for_close"], true);
    assert!(
        !candidates
            .iter()
            .any(|task| task["id"] == ordinary.as_str())
    );

    let output = repo.orbit(&["prime"]);
    assert!(output.status.success());
    let human = String::from_utf8_lossy(&output.stdout);
    assert!(human.contains("close candidates:"));
    assert!(human.contains("ready to close"));

    repo.ok_json(&["done", &parent, "--by", "test", "--json"]);
    let prime = repo.ok_json(&["prime", "--json"]);
    assert!(
        prime["data"]["close_candidates"]
            .as_array()
            .unwrap()
            .is_empty()
    );
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
    // Human output leads with project keys.
    assert!(human.contains(&repo.key(&parent)));
    assert!(human.contains(&repo.key(&first)));
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
fn issue_ids_do_not_encode_the_parent() {
    let repo = TestRepo::new();
    repo.init();
    let parent = repo.add("Feature", &[]);
    let child = subtask(&repo, &parent, "Part");

    // IDs are flat Board IDs: the parent edge lives in `parent`, never in
    // the ID (ADR 0022).
    assert!(!child.contains('.'), "{child}");
    assert_ne!(child, parent);
    assert_eq!(
        repo.ok_json(&["show", &child, "--json"])["data"]["parent"],
        parent.as_str()
    );
}

#[test]
fn nested_issue_at_depth_two_loads_lists_and_shows() {
    let repo = TestRepo::new();
    repo.init();

    // A hand-written epic > task > sub-task chain with flat IDs.
    repo.write_task(
        "orb-a0b1c2",
        "---\ntitle: Release\nstatus: todo\ntype: epic\ncreated_at: 2026-01-01T00:00:00Z\nupdated_at: 2026-01-01T00:00:00Z\n---\n\nBody.\n",
    );
    repo.write_task(
        "orb-d4b3df",
        "---\ntitle: Middle\nstatus: todo\ntype: task\nparent: orb-a0b1c2\ncreated_at: 2026-01-01T00:00:00Z\nupdated_at: 2026-01-01T00:00:00Z\n---\n\nBody.\n",
    );
    repo.write_task(
        "orb-e5f6g7",
        "---\ntitle: Nested step\nstatus: todo\ntype: sub-task\nparent: orb-d4b3df\ncreated_at: 2026-01-01T00:00:00Z\nupdated_at: 2026-01-01T00:00:00Z\n---\n\nBody.\n",
    );

    let listed = repo.ok_json(&["list", "--all", "--json"]);
    let ids: Vec<&str> = listed["data"]["tasks"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|task| task["id"].as_str())
        .collect();
    assert!(ids.contains(&"orb-e5f6g7"), "{listed}");

    let shown = repo.ok_json(&["show", "orb-e5f6g7", "--json"]);
    assert_eq!(shown["data"]["id"], "orb-e5f6g7");
    assert_eq!(shown["data"]["kind"], "subtask");
    assert_eq!(shown["data"]["parent"], "orb-d4b3df");
    assert_eq!(shown["data"]["depth"], 2);

    let output = repo.orbit(&["show", "orb-e5f6g7"]);
    assert!(output.status.success());
    let human = String::from_utf8_lossy(&output.stdout);
    assert!(human.contains("orb-e5f6g7"), "{human}");
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

    // `sub-task` is the leaf of the default hierarchy: it cannot parent.
    let (status, value) = repo.json(&["add", "Nested", "--parent", &child, "--json"]);
    assert_eq!(status, 1);
    assert_eq!(value["error"]["code"], "INVALID_TYPE");
    let message = value["error"]["message"].as_str().unwrap();
    assert!(message.contains("hierarchy"), "{message}");

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
fn hierarchy_allows_a_second_level_and_rejects_the_leaf() {
    let repo = TestRepo::new();
    repo.init();

    // epic > task > sub-task: the default model reaches depth two.
    let parent = repo.add("Feature", &["--type", "epic"]);
    let child = repo.ok_json(&[
        "add", "Part", "--parent", &parent, "--type", "task", "--json",
    ])["data"]["id"]
        .as_str()
        .unwrap()
        .to_string();
    let grandchild = subtask(&repo, &child, "Step");

    let shown = repo.ok_json(&["show", &grandchild, "--json"]);
    assert_eq!(shown["data"]["kind"], "subtask");
    assert_eq!(shown["data"]["type"], "sub-task");
    assert_eq!(shown["data"]["parent"], child.as_str());
    assert!(!grandchild.contains('.'), "{grandchild}");

    // `sub-task` is the leaf: nothing may parent under it.
    let (status, value) = repo.json(&["add", "Too deep", "--parent", &grandchild, "--json"]);
    assert_eq!(status, 1);
    assert_eq!(value["error"]["code"], "INVALID_TYPE");
    let message = value["error"]["message"].as_str().unwrap();
    assert!(message.contains("hierarchy"), "{message}");
}

#[test]
fn list_and_ready_reject_a_missing_parent_filter() {
    let repo = TestRepo::new();
    repo.init();
    repo.add("Feature", &[]);

    for command in ["list", "ready"] {
        let (status, value) = repo.json(&[command, "--parent", "orb-missing", "--json"]);
        assert_eq!(status, 1, "{command}");
        assert_eq!(value["error"]["code"], "NOT_FOUND", "{command}");
    }
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
    assert_eq!(status(&repo, &child), "ready");
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

    assert_eq!(status(&repo, &parent), "ready");
    assert_eq!(status(&repo, &child), "ready");
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

/// Builds an epic > task > sub-task chain, the deepest nesting the default
/// Board hierarchy allows.
fn nested_chain(repo: &TestRepo) -> (String, String, String) {
    let parent = repo.add("Feature", &["--type", "epic"]);
    let child = repo.ok_json(&[
        "add", "Part", "--parent", &parent, "--type", "task", "--json",
    ])["data"]["id"]
        .as_str()
        .unwrap()
        .to_string();
    let grandchild = subtask(repo, &child, "Step");
    (parent, child, grandchild)
}

#[test]
fn nested_subtasks_block_done_until_each_level_resolves() {
    let repo = TestRepo::new();
    repo.init();
    let (parent, child, grandchild) = nested_chain(&repo);

    let (code, value) = repo.json(&["done", &parent, "--by", "test", "--json"]);
    assert_eq!(code, 1);
    assert_eq!(value["error"]["code"], "SUBTASKS_OPEN");
    // The block names every open descendant, at any depth.
    assert_eq!(
        value["error"]["details"]["subtasks"],
        serde_json::json!([child.as_str(), grandchild.as_str()])
    );

    let (code, value) = repo.json(&["done", &child, "--by", "test", "--json"]);
    assert_eq!(code, 1);
    assert_eq!(value["error"]["code"], "SUBTASKS_OPEN");
    assert_eq!(
        value["error"]["details"]["subtasks"][0],
        grandchild.as_str()
    );

    repo.ok_json(&["done", &grandchild, "--by", "test", "--json"]);
    repo.ok_json(&["done", &child, "--by", "test", "--json"]);

    let shown = repo.ok_json(&["show", &parent, "--json"]);
    assert_eq!(shown["data"]["eligible_for_close"], true);
    let ready = repo.ok_json(&["ready", "--json"]);
    let ids: Vec<&str> = ready["data"]["tasks"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|task| task["id"].as_str())
        .collect();
    assert!(ids.contains(&parent.as_str()));
}

#[test]
fn done_force_cancels_every_open_descendant() {
    let repo = TestRepo::new();
    repo.init();
    let (parent, child, grandchild) = nested_chain(&repo);

    let value = repo.ok_json(&["done", &parent, "--by", "test", "--force", "--json"]);

    assert_eq!(status(&repo, &parent), "done");
    assert_eq!(status(&repo, &child), "cancelled");
    assert_eq!(status(&repo, &grandchild), "cancelled");
    let warnings = value["meta"]["warnings"].as_array().unwrap();
    let warning = warnings
        .iter()
        .filter_map(|warning| warning.as_str())
        .find(|warning| warning.contains("cancelled open SubTasks"))
        .expect("missing cancelled SubTasks warning");
    assert!(warning.contains(&child), "{warning}");
    assert!(warning.contains(&grandchild), "{warning}");
}

#[test]
fn cancel_force_cancels_every_open_descendant() {
    let repo = TestRepo::new();
    repo.init();
    let (parent, child, grandchild) = nested_chain(&repo);

    repo.ok_json(&["cancel", &parent, "--by", "test", "--force", "--json"]);

    assert_eq!(status(&repo, &parent), "cancelled");
    assert_eq!(status(&repo, &child), "cancelled");
    assert_eq!(status(&repo, &grandchild), "cancelled");
}

#[test]
fn nested_done_force_still_respects_unfinished_dependencies() {
    let repo = TestRepo::new();
    repo.init();
    let blocker = repo.add("Blocker", &[]);
    let parent = repo.add(
        "Feature",
        &["--type", "epic", "--depends-on", blocker.as_str()],
    );
    let child = repo.ok_json(&[
        "add", "Part", "--parent", &parent, "--type", "task", "--json",
    ])["data"]["id"]
        .as_str()
        .unwrap()
        .to_string();
    let grandchild = subtask(&repo, &child, "Step");

    let (code, value) = repo.json(&["done", &parent, "--by", "test", "--force", "--json"]);

    assert_eq!(code, 1);
    assert_eq!(value["error"]["code"], "DEPENDENCY_BLOCKED");
    assert_eq!(status(&repo, &child), "ready");
    assert_eq!(status(&repo, &grandchild), "ready");
}

#[test]
fn reopen_walks_done_ancestors_nearest_first() {
    let repo = TestRepo::new();
    repo.init();
    let (parent, child, grandchild) = nested_chain(&repo);

    repo.ok_json(&["done", &grandchild, "--by", "test", "--json"]);
    repo.ok_json(&["done", &child, "--by", "test", "--json"]);
    repo.ok_json(&["done", &parent, "--by", "test", "--json"]);

    let value = repo.ok_json(&["reopen", &grandchild, "--json"]);

    assert_eq!(status(&repo, &grandchild), "ready");
    assert_eq!(status(&repo, &child), "ready");
    assert_eq!(status(&repo, &parent), "ready");
    let warnings = value["meta"]["warnings"].as_array().unwrap();
    let reopened: Vec<String> = warnings
        .iter()
        .filter_map(|warning| warning.as_str())
        .filter(|warning| warning.starts_with("reopened parent "))
        .map(str::to_string)
        .collect();
    assert_eq!(
        reopened,
        vec![
            format!("reopened parent {child}"),
            format!("reopened parent {parent}"),
        ]
    );
}

#[test]
fn reopen_stops_at_an_active_ancestor() {
    let repo = TestRepo::new();
    repo.init();
    let (parent, child, grandchild) = nested_chain(&repo);

    repo.ok_json(&["done", &grandchild, "--by", "test", "--json"]);
    repo.ok_json(&["done", &child, "--by", "test", "--json"]);
    assert_eq!(status(&repo, &parent), "ready");

    let value = repo.ok_json(&["reopen", &grandchild, "--json"]);

    assert_eq!(status(&repo, &grandchild), "ready");
    assert_eq!(status(&repo, &child), "ready");
    assert_eq!(status(&repo, &parent), "ready");
    let warnings = value["meta"]["warnings"].as_array().unwrap();
    let reopened: Vec<String> = warnings
        .iter()
        .filter_map(|warning| warning.as_str())
        .filter(|warning| warning.starts_with("reopened parent "))
        .map(str::to_string)
        .collect();
    assert_eq!(reopened, vec![format!("reopened parent {child}")]);
}

#[test]
fn reopen_fails_naming_a_cancelled_ancestor() {
    let repo = TestRepo::new();
    repo.init();
    let (parent, child, grandchild) = nested_chain(&repo);

    repo.ok_json(&["cancel", &child, "--by", "test", "--force", "--json"]);
    assert_eq!(status(&repo, &grandchild), "cancelled");

    let (code, value) = repo.json(&["reopen", &grandchild, "--json"]);
    assert_eq!(code, 1);
    assert_eq!(value["error"]["code"], "INVALID_TRANSITION");
    let message = value["error"]["message"].as_str().unwrap();
    assert!(message.contains(&child), "{message}");
    assert!(message.contains("cancelled"), "{message}");
    assert_eq!(status(&repo, &grandchild), "cancelled");
    assert_eq!(status(&repo, &parent), "ready");
}

#[test]
fn delete_nested_subtasks_requires_force_and_cascades() {
    let repo = TestRepo::new();
    repo.init();
    let (parent, child, grandchild) = nested_chain(&repo);

    let (code, value) = repo.json(&["delete", &parent, "--json"]);
    assert_eq!(code, 1);
    assert_eq!(value["error"]["code"], "SUBTASKS_EXIST");

    let value = repo.ok_json(&["delete", &parent, "--force", "--json"]);
    let deleted: Vec<&str> = value["data"]["deleted_ids"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|id| id.as_str())
        .collect();
    for id in [&parent, &child, &grandchild] {
        assert!(deleted.contains(&id.as_str()), "{deleted:?}");
        assert!(!repo.task_path(id).exists());
    }
    let listed = repo.ok_json(&["list", "--all", "--json"]);
    assert_eq!(listed["data"]["count"], 0);
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
fn list_parent_filters_and_group_layout() {
    let repo = TestRepo::new();
    repo.init();
    let parent = repo.add("Feature", &["--type", "epic"]);
    let child = subtask(&repo, &parent, "Part");
    let other = repo.add("Standalone", &[]);

    let filtered = repo.ok_json(&["list", "--parent", &parent, "--json"]);
    assert_eq!(filtered["data"]["count"], 1);
    assert_eq!(filtered["data"]["tasks"][0]["id"], child.as_str());

    // A leaf Issue simply has no children; the filter still resolves.
    let listed = repo.ok_json(&["list", "--parent", &child, "--json"]);
    assert_eq!(listed["data"]["count"], 0);
    let ready = repo.ok_json(&["ready", "--parent", &child, "--json"]);
    assert_eq!(ready["data"]["count"], 0);

    let grouped = repo.ok_json(&["list", "--group", "--json"]);
    let ids: Vec<&str> = grouped["data"]["tasks"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|task| task["id"].as_str())
        .collect();
    assert_eq!(ids, vec![parent.as_str(), child.as_str(), other.as_str()]);

    let output = repo.orbit(&["list", "--group"]);
    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert_eq!(stdout.matches("CLAIMED BY").count(), 1, "{stdout}");
    assert!(stdout.contains("╭"), "{stdout}");
    assert!(stdout.contains("No parent"), "{stdout}");
    assert_eq!(stdout.matches("Feature").count(), 1, "{stdout}");
    let widths: Vec<usize> = stdout.lines().map(|line| line.chars().count()).collect();
    assert!(
        widths.windows(2).all(|pair| pair[0] == pair[1]),
        "outline lines must align: {stdout}"
    );

    let parent_line = stdout
        .lines()
        .position(|line| line.contains("Feature"))
        .unwrap_or_else(|| panic!("no parent banner in {stdout}"));
    let child_line = stdout
        .lines()
        .position(|line| line.contains(&repo.key(&child)))
        .unwrap_or_else(|| panic!("no child line in {stdout}"));
    let other_line = stdout
        .lines()
        .position(|line| line.contains(&repo.key(&other)))
        .unwrap_or_else(|| panic!("no standalone line in {stdout}"));
    assert!(parent_line < child_line, "{stdout}");
    assert!(child_line < other_line, "{stdout}");
}

#[test]
fn list_orders_by_priority_then_parent_then_subtasks() {
    let repo = TestRepo::new();
    repo.init();
    let parent = repo.add("Parent", &["-p", "medium", "--type", "epic"]);
    let child = subtask(&repo, &parent, "Urgent child");
    repo.ok_json(&["update", &child, "--priority", "highest", "--json"]);
    let middle = repo.add("Middle", &["-p", "high"]);
    let first = repo.add("First", &["-p", "highest"]);

    let output = repo.orbit(&["list"]);
    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    let position = |needle: &str| {
        stdout
            .lines()
            .position(|line| line.contains(needle))
            .unwrap_or_else(|| panic!("missing {needle} in {stdout}"))
    };

    let first_key = repo.key(&first);
    let middle_key = repo.key(&middle);
    let parent_key = repo.key(&parent);
    let child_key = repo.key(&child);
    assert!(position(&first_key) < position(&middle_key), "{stdout}");
    assert!(position(&middle_key) < position(&parent_key), "{stdout}");
    assert!(position(&parent_key) < position(&child_key), "{stdout}");
}

#[test]
fn tree_recurses_through_nested_subtasks() {
    let repo = TestRepo::new();
    repo.init();
    let (parent, child, grandchild) = nested_chain(&repo);

    let tree = repo.ok_json(&["tree", &parent, "--json"]);
    assert_eq!(tree["data"]["root"], parent.as_str());
    assert_eq!(tree["data"]["count"], 3);
    let tasks = tree["data"]["tasks"].as_array().unwrap();
    let ids: Vec<&str> = tasks
        .iter()
        .filter_map(|task| task["id"].as_str())
        .collect();
    assert_eq!(
        ids,
        vec![parent.as_str(), child.as_str(), grandchild.as_str()],
        "root first, then descendants depth-first: {ids:?}"
    );
    assert_eq!(tasks[0]["depth"], 0);
    assert_eq!(tasks[1]["depth"], 1);
    assert_eq!(tasks[2]["depth"], 2);

    let shown = repo.ok_json(&["show", &grandchild, "--json"]);
    assert_eq!(shown["data"]["depth"], 2);

    let output = repo.orbit(&["tree", &parent]);
    assert!(output.status.success());
    let human = String::from_utf8_lossy(&output.stdout);
    let line_of = |needle: &str, exclude: Option<&str>| {
        human
            .lines()
            .find(|line| line.contains(needle) && exclude.is_none_or(|skip| !line.contains(skip)))
            .unwrap_or_else(|| panic!("missing {needle} in {human}"))
            .to_string()
    };
    let grandchild_key = repo.key(&grandchild);
    let child_key = repo.key(&child);
    let grandchild_line = line_of(&grandchild_key, None);
    assert!(grandchild_line.starts_with("    "), "{grandchild_line}");
    let child_line = line_of(&child_key, Some(&grandchild_key));
    assert!(child_line.starts_with("  "), "{child_line}");
    assert!(!child_line.starts_with("    "), "{child_line}");
    assert!(!human.lines().next().unwrap().starts_with(' '));
}

#[test]
fn tree_accepts_a_subtask_root_that_has_subtasks() {
    let repo = TestRepo::new();
    repo.init();
    let (parent, child, grandchild) = nested_chain(&repo);

    let tree = repo.ok_json(&["tree", &child, "--json"]);
    assert_eq!(tree["data"]["root"], child.as_str());
    assert_eq!(tree["data"]["count"], 2);
    let ids: Vec<&str> = tree["data"]["tasks"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|task| task["id"].as_str())
        .collect();
    assert_eq!(ids, vec![child.as_str(), grandchild.as_str()]);

    // A SubTask without SubTasks keeps the current error.
    let leaf = subtask(&repo, &parent, "Leaf");
    let (status, value) = repo.json(&["tree", &leaf, "--json"]);
    assert_eq!(status, 1);
    assert_eq!(value["error"]["code"], "INVALID_ARGUMENT");
}

#[test]
fn list_and_ready_parent_accept_a_subtask() {
    let repo = TestRepo::new();
    repo.init();
    let (parent, child, grandchild) = nested_chain(&repo);
    let sibling = subtask(&repo, &child, "Side");

    let listed = repo.ok_json(&["list", "--parent", &child, "--json"]);
    assert_eq!(listed["data"]["count"], 2);
    let ids: Vec<&str> = listed["data"]["tasks"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|task| task["id"].as_str())
        .collect();
    assert!(ids.contains(&grandchild.as_str()));
    assert!(ids.contains(&sibling.as_str()));
    assert!(!ids.contains(&parent.as_str()));

    let ready = repo.ok_json(&["ready", "--parent", &child, "--json"]);
    assert_eq!(ready["data"]["count"], 2);
    let ready_ids: Vec<&str> = ready["data"]["tasks"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|task| task["id"].as_str())
        .collect();
    assert!(ready_ids.contains(&grandchild.as_str()));
    assert!(ready_ids.contains(&sibling.as_str()));

    // Direct children only: the grandchild's own children would not appear.
    let empty = repo.ok_json(&["list", "--parent", &grandchild, "--json"]);
    assert_eq!(empty["data"]["count"], 0);
}

#[test]
fn list_group_places_nested_subtasks_under_their_direct_parent() {
    let repo = TestRepo::new();
    repo.init();
    let (parent, child, grandchild) = nested_chain(&repo);
    let other = repo.add("Standalone", &[]);

    let grouped = repo.ok_json(&["list", "--group", "--all", "--json"]);
    let ids: Vec<&str> = grouped["data"]["tasks"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|task| task["id"].as_str())
        .collect();
    assert!(ids.contains(&parent.as_str()));
    assert!(ids.contains(&child.as_str()));
    assert!(ids.contains(&grandchild.as_str()));
    assert!(ids.contains(&other.as_str()));

    let output = repo.orbit(&["list", "--group", "--all"]);
    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    // Human output leads with project keys.
    assert!(stdout.contains(&repo.key(&parent)), "{stdout}");
    assert!(stdout.contains(&repo.key(&child)), "{stdout}");
    assert!(stdout.contains(&repo.key(&grandchild)), "{stdout}");
    assert!(stdout.contains(&repo.key(&other)), "{stdout}");
}

#[test]
fn ready_offers_a_task_only_when_its_subtasks_are_resolved_at_any_depth() {
    let repo = TestRepo::new();
    repo.init();
    let (parent, child, grandchild) = nested_chain(&repo);

    let ready_ids = |repo: &TestRepo| {
        repo.ok_json(&["ready", "--all", "--json"])["data"]["tasks"]
            .as_array()
            .unwrap()
            .iter()
            .filter_map(|task| task["id"].as_str().map(str::to_string))
            .collect::<Vec<_>>()
    };

    let ready = ready_ids(&repo);
    assert!(ready.contains(&grandchild));
    assert!(!ready.contains(&child));
    assert!(!ready.contains(&parent));

    repo.ok_json(&["done", &grandchild, "--by", "test", "--json"]);
    let ready = ready_ids(&repo);
    assert!(ready.contains(&child));
    assert!(!ready.contains(&parent));

    repo.ok_json(&["done", &child, "--by", "test", "--json"]);
    let ready = ready_ids(&repo);
    assert!(ready.contains(&parent));
}

#[test]
fn tree_flag_is_removed() {
    let repo = TestRepo::new();
    repo.init();

    let output = repo.orbit(&["list", "--tree"]);
    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("--tree"), "{stderr}");
}

#[test]
fn group_shows_a_parent_that_the_filter_hides() {
    let repo = TestRepo::new();
    repo.init();
    let parent = repo.add("Feature", &["--type", "epic"]);
    let child = subtask(&repo, &parent, "Part");

    let listed = repo.ok_json(&["list", "--group", "--type", "sub-task", "--json"]);
    assert_eq!(listed["data"]["count"], 1);
    assert_eq!(listed["data"]["tasks"][0]["id"], child.as_str());

    let output = repo.orbit(&["list", "--group", "--type", "sub-task"]);
    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains(&format!("{}  Feature", repo.key(&parent))),
        "{stdout}"
    );
    assert!(stdout.contains(&repo.key(&child)), "{stdout}");
    assert!(!stdout.contains("No parent"), "{stdout}");
}

#[test]
fn doctor_reports_orphan_and_self_parents() {
    let repo = TestRepo::new();
    repo.init();
    let orphan = repo.add("Orphan", &[]);
    let content = repo
        .read_task(&orphan)
        .replace("status: ready", "status: ready\nparent: orb-missing");
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
        .replace("status: ready", &format!("status: ready\nparent: {task}"));
    repo.write_task(&task, &content);

    let (status, value) = repo.json(&["doctor", "--json"]);
    assert_eq!(status, 1);
    assert!(
        value["data"]["issues"]
            .as_array()
            .unwrap()
            .iter()
            .any(|issue| issue["code"] == "PARENT_CYCLE" && issue["task"] == task.as_str())
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
    assert!(
        stdout.contains(&format!("subtasks: {}", repo.key(&child))),
        "{stdout}"
    );

    let output = repo.orbit(&["show", &child]);
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("kind: subtask"), "{stdout}");
    assert!(
        stdout.contains(&format!("parent: {}", repo.key(&parent))),
        "{stdout}"
    );
}

#[test]
fn forced_cancel_of_a_terminal_parent_still_commits_its_cancelled_subtasks() {
    let repo = TestRepo::new();
    repo.init();
    let parent = repo.add("Feature", &[]);
    let child = subtask(&repo, &parent, "Part");
    let content = repo
        .read_task(&parent)
        .replace("status: ready", "status: cancelled");
    repo.write_task(&parent, &content);
    repo.ok_json(&["sync", "--json"]);

    repo.ok_json(&["cancel", &parent, "--by", "test", "--force", "--json"]);

    assert_eq!(status(&repo, &child), "cancelled");
    let board = repo.board_path();
    assert_eq!(repo.git_in(&board, &["status", "--porcelain"]).stdout, b"");
    let subject = repo.git_in(&board, &["log", "-1", "--format=%s"]);
    assert_eq!(
        String::from_utf8_lossy(&subject.stdout).trim(),
        format!("orbit: cancel {parent}")
    );
}

#[test]
fn a_board_written_by_the_previous_release_loads_everywhere() {
    let repo = TestRepo::new();
    repo.init();
    let parent = repo.add("Feature", &[]);
    let child = subtask(&repo, &parent, "Part");
    let loose = repo.add("Loose", &[]);

    // A previous-release Board never wrote an Issue model, and the current
    // release still omits `board.types`/`board.hierarchy` at the defaults.
    let config = repo.read(".orbit/board/config.toml");
    assert!(!config.contains("max_subtask_depth"), "{config}");
    assert!(!config.contains("[[board.types]]"), "{config}");

    // Every command performs the same strict load and validation; doctor
    // reports the Board healthy.
    let (status, value) = repo.json(&["doctor", "--json"]);
    assert_eq!(status, 0, "{value}");
    assert_eq!(value["data"]["healthy"], true, "{value}");

    let list = repo.ok_json(&["list", "--json"]);
    assert_eq!(list["data"]["count"], 3, "{list}");

    let tree = repo.ok_json(&["tree", parent.as_str(), "--json"]);
    assert_eq!(tree["data"]["count"], 2, "{tree}");
    assert_eq!(tree["data"]["tasks"][0]["id"], parent.as_str());
    assert_eq!(tree["data"]["tasks"][1]["id"], child.as_str());

    let ready = repo.ok_json(&["ready", "--json"]);
    let ids: Vec<&str> = ready["data"]["tasks"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|task| task["id"].as_str())
        .collect();
    assert!(ids.contains(&child.as_str()), "{ids:?}");
    assert!(ids.contains(&loose.as_str()), "{ids:?}");
    assert!(!ids.contains(&parent.as_str()), "{ids:?}");

    let export = repo.ok_json(&["export", "--json"]);
    assert_eq!(export["data"]["schema_version"], 5, "{export}");
    let exported = export["data"]["tasks"]
        .as_array()
        .unwrap()
        .iter()
        .find(|task| task["id"] == child.as_str())
        .expect("child missing from export");
    assert_eq!(exported["kind"], "subtask");
    assert_eq!(exported["parent"], parent.as_str());
}

#[test]
fn a_shared_board_at_depth_two_never_offers_holds_or_open_parents() {
    let repo = TestRepo::new();
    repo.init();
    let (parent, child, grandchild) = nested_chain(&repo);

    // Two agents share the Board: agent-a claims an unrelated Task while
    // agent-b puts a hold label on the deepest SubTask.
    let claimed = repo.add("Claimed elsewhere", &[]);
    repo.ok_json(&["claim", claimed.as_str(), "--by", "agent-a", "--json"]);
    repo.ok_json(&[
        "label",
        "add",
        grandchild.as_str(),
        "needs-triage",
        "--json",
    ]);

    let ready = repo.ok_json(&["ready", "--all", "--json"]);
    let ids: Vec<&str> = ready["data"]["tasks"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|task| task["id"].as_str())
        .collect();

    // The hold-labelled SubTask is never offered, and neither is any ancestor
    // while a descendant stays open — at any configured depth.
    assert!(!ids.contains(&grandchild.as_str()), "{ids:?}");
    assert!(!ids.contains(&child.as_str()), "{ids:?}");
    assert!(!ids.contains(&parent.as_str()), "{ids:?}");
    assert!(!ids.contains(&claimed.as_str()), "{ids:?}");

    // Removing the hold makes only the leaf claimable again.
    repo.ok_json(&[
        "label",
        "remove",
        grandchild.as_str(),
        "needs-triage",
        "--json",
    ]);
    let ready = repo.ok_json(&["ready", "--all", "--json"]);
    let ids: Vec<&str> = ready["data"]["tasks"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|task| task["id"].as_str())
        .collect();
    assert_eq!(ids, vec![grandchild.as_str()], "{ids:?}");
}

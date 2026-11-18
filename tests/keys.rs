//! Sequential project keys (`ORB-N`): allocation under the Board lock,
//! resolution by key and alias, and re-keying on Board sync (ADR 0022).

mod common;

use common::TestRepo;

fn key_of(repo: &TestRepo, id: &str) -> String {
    repo.ok_json(&["show", id, "--json"])["data"]["key"]
        .as_str()
        .unwrap_or_else(|| panic!("show {id} returned no key"))
        .to_string()
}

#[test]
fn adds_allocate_sequential_project_keys() {
    let repo = TestRepo::new();
    repo.init();

    let first = repo.add("First", &[]);
    let second = repo.add("Second", &[]);
    let third = repo.add("Third", &[]);

    assert_eq!(key_of(&repo, &first), "ORB-1");
    assert_eq!(key_of(&repo, &second), "ORB-2");
    assert_eq!(key_of(&repo, &third), "ORB-3");

    // The allocator high-water mark is one past the highest handed-out key.
    let state = repo.read(".orbit/board/keys.toml");
    assert!(state.contains("next_key = 4"), "{state}");

    // Deleting an Issue never frees its key for reuse.
    repo.ok_json(&["delete", &second, "--force", "--json"]);
    let fourth = repo.add("Fourth", &[]);
    assert_eq!(key_of(&repo, &fourth), "ORB-4");
}

#[test]
fn project_keys_resolve_like_internal_ids() {
    let repo = TestRepo::new();
    repo.init();

    let blocker = repo.add("Blocker", &[]);
    let waiting = repo.add("Waiting", &[]);
    repo.ok_json(&["depend", waiting.as_str(), "ORB-1", "--json"]);

    // `show` accepts the key, case-insensitively, and reports both handles.
    let shown = repo.ok_json(&["show", "orb-1", "--json"]);
    assert_eq!(shown["data"]["id"], blocker);
    assert_eq!(shown["data"]["key"], "ORB-1");
    assert_eq!(shown["data"]["key_aliases"], serde_json::json!([]));

    // The dependency stored the immutable internal ID, not the key.
    let waiting_json = repo.ok_json(&["show", "ORB-2", "--json"]);
    assert_eq!(waiting_json["data"]["id"], waiting);
    let links = waiting_json["data"]["links"].as_array().unwrap();
    assert!(
        links
            .iter()
            .any(|link| link["target"] == serde_json::json!(blocker)),
        "{links:?}"
    );

    // Mutations accept keys too: depend by key, then comment by key.
    repo.ok_json(&["depend", "ORB-2", "ORB-1", "--json"]);
    let commented = repo.ok_json(&["comment", "add", "ORB-1", "--body", "hi", "--json"]);
    assert_eq!(commented["data"]["id"], blocker);
    let comments = repo.ok_json(&["comment", "list", "ORB-1", "--json"]);
    assert_eq!(comments["data"]["comments"].as_array().unwrap().len(), 1);

    // Human output leads with the key and still shows the internal ID.
    let human = repo.orbit(&["show", "ORB-1"]);
    assert!(human.status.success());
    let stdout = String::from_utf8_lossy(&human.stdout);
    assert!(stdout.contains("ORB-1"), "{stdout}");
    assert!(stdout.contains(&blocker), "{stdout}");

    // `list` prints keys in the ID column.
    let list = repo.orbit(&["list"]);
    let stdout = String::from_utf8_lossy(&list.stdout);
    assert!(stdout.contains("ORB-1"), "{stdout}");
    assert!(stdout.contains("ORB-2"), "{stdout}");
}

#[test]
fn batch_and_delete_accept_project_keys() {
    let repo = TestRepo::new();
    repo.init();

    let first = repo.add("First", &[]);
    let plan = serde_json::json!({
        "operations": [
            { "op": "add", "title": "Batch child", "parent": "ORB-1" },
            { "op": "comment", "id": "ORB-1", "body": "via batch" }
        ]
    });
    let plan_path = repo.path().join("plan.json");
    std::fs::write(&plan_path, serde_json::to_vec_pretty(&plan).unwrap()).unwrap();
    let batch = repo.ok_json(&["batch", "--file", plan_path.to_str().unwrap(), "--json"]);
    assert_eq!(
        batch["data"]["results"].as_array().unwrap().len(),
        2,
        "{batch}"
    );
    assert_eq!(batch["data"]["committed"], true, "{batch}");

    // The batch-created child got the next sequential key.
    let child_key = repo.ok_json(&["list", "--json"])["data"]["issues"]
        .as_array()
        .unwrap()
        .iter()
        .find(|task| task["title"] == "Batch child")
        .and_then(|task| task["key"].as_str())
        .unwrap()
        .to_string();
    assert_eq!(child_key, "ORB-2");

    // Deleting by key removes the Task; its key is never reused.
    repo.ok_json(&["delete", "ORB-2", "--force", "--json"]);
    let replacement = repo.add("Replacement", &[]);
    assert_eq!(key_of(&repo, &replacement), "ORB-3");
    assert!(repo.task_path(&first).exists());
}

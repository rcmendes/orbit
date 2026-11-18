mod common;

use common::TestRepo;
use serde_json::Value;

fn link_json(task: &Value) -> Vec<Value> {
    task["links"].as_array().unwrap().clone()
}

#[test]
fn blocking_links_drive_readiness() {
    let repo = TestRepo::new();
    repo.init();
    let blocker = repo.add("Blocker", &[]);
    let dependent = repo.add("Dependent", &[]);

    let added = repo.ok_json(&[
        "link", "add", &dependent, "--type", "blocks", "--to", &blocker, "--json",
    ]);
    assert_eq!(added["data"]["id"], dependent);

    let show = repo.ok_json(&["show", &dependent, "--json"]);
    let links = link_json(&show["data"]);
    assert_eq!(links.len(), 1);
    assert_eq!(links[0]["type"], "blocks");
    assert_eq!(links[0]["target"], blocker);
    assert_eq!(links[0]["blocking"], true);
    assert_eq!(links[0]["inward"], "is blocked by");
    assert_eq!(links[0]["outward"], "blocks");

    // The dependent is not ready; the blocker is.
    let ready = repo.ok_json(&["ready", "--json"]);
    let ids: Vec<&str> = ready["data"]["issues"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|task| task["id"].as_str())
        .collect();
    assert!(ids.contains(&blocker.as_str()));
    assert!(!ids.contains(&dependent.as_str()));
    assert_eq!(show["data"]["ready"], false);
    assert_eq!(show["data"]["blocking"][0], blocker);

    // Completing the blocker makes the dependent ready.
    repo.ok_json(&[
        "done",
        &blocker,
        "--by",
        "devin",
        "--comment",
        "done",
        "--json",
    ]);
    let ready = repo.ok_json(&["ready", "--json"]);
    let ids: Vec<&str> = ready["data"]["issues"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|task| task["id"].as_str())
        .collect();
    assert!(ids.contains(&dependent.as_str()));
}

#[test]
fn non_blocking_links_do_not_drive_readiness() {
    let repo = TestRepo::new();
    repo.init();
    let other = repo.add("Other", &[]);
    let task = repo.add("Task", &[]);

    repo.ok_json(&[
        "link",
        "add",
        &task,
        "--type",
        "relates-to",
        "--to",
        &other,
        "--json",
    ]);

    let show = repo.ok_json(&["show", &task, "--json"]);
    let links = link_json(&show["data"]);
    assert_eq!(links[0]["blocking"], false);
    assert_eq!(links[0]["inward"], "relates to");
    assert_eq!(links[0]["outward"], "relates to");

    // Non-blocking: the task stays ready even though the target is open.
    assert_eq!(show["data"]["ready"], true);
    let ready = repo.ok_json(&["ready", "--json"]);
    let ids: Vec<&str> = ready["data"]["issues"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|task| task["id"].as_str())
        .collect();
    assert!(ids.contains(&task.as_str()));
}

#[test]
fn blocking_links_reject_cycles_non_blocking_links_allow_them() {
    let repo = TestRepo::new();
    repo.init();
    let a = repo.add("A", &[]);
    let b = repo.add("B", &[]);

    // A blocking cycle is rejected.
    repo.ok_json(&["link", "add", &a, "--type", "blocks", "--to", &b, "--json"]);
    let (status, error) = repo.json(&["link", "add", &b, "--type", "blocks", "--to", &a, "--json"]);
    assert_ne!(status, 0, "{error}");

    // The same shape with a non-blocking type is fine.
    repo.ok_json(&[
        "link",
        "add",
        &b,
        "--type",
        "relates-to",
        "--to",
        &a,
        "--json",
    ]);
    let (status, error) = repo.json(&[
        "link",
        "add",
        &a,
        "--type",
        "relates-to",
        "--to",
        &b,
        "--json",
    ]);
    assert_eq!(status, 0, "{error}");
}

#[test]
fn mixed_blocking_and_non_blocking_cycles_are_checked_on_blocking_only() {
    let repo = TestRepo::new();
    repo.init();
    let a = repo.add("A", &[]);
    let b = repo.add("B", &[]);

    // B is blocked by A (blocking), and A relates to B (non-blocking): no
    // blocking cycle, both links are allowed.
    repo.ok_json(&["link", "add", &b, "--type", "blocks", "--to", &a, "--json"]);
    repo.ok_json(&[
        "link",
        "add",
        &a,
        "--type",
        "relates-to",
        "--to",
        &b,
        "--json",
    ]);

    // Flipping the relates-to link to blocking would close a blocking cycle.
    repo.ok_json(&[
        "link",
        "remove",
        &a,
        "--type",
        "relates-to",
        "--to",
        &b,
        "--json",
    ]);
    let (status, error) = repo.json(&["link", "add", &a, "--type", "blocks", "--to", &b, "--json"]);
    assert_ne!(status, 0, "{error}");
}

#[test]
fn link_attributes_round_trip_through_frontmatter_and_json() {
    let repo = TestRepo::new();
    repo.init();
    let canonical = repo.add("Canonical", &[]);
    let duplicate = repo.add("Duplicate", &[]);

    repo.ok_json(&[
        "link",
        "add",
        &duplicate,
        "--type",
        "duplicates",
        "--to",
        &canonical,
        "--attribute",
        "reason=same root cause",
        "--attribute",
        "score=3",
        "--json",
    ]);

    let show = repo.ok_json(&["show", &duplicate, "--json"]);
    let links = link_json(&show["data"]);
    assert_eq!(links[0]["type"], "duplicates");
    assert_eq!(links[0]["blocking"], false);
    assert_eq!(links[0]["attributes"]["reason"], "same root cause");
    assert_eq!(links[0]["attributes"]["score"], 3);

    // The attributes are persisted in the task file, not just the JSON view.
    let body = repo.read_task(&duplicate);
    assert!(body.contains("attributes:"), "{body}");
    assert!(body.contains("reason: same root cause"), "{body}");
    assert!(body.contains("score: 3"), "{body}");

    // `link list` reports the same links.
    let list = repo.ok_json(&["link", "list", &duplicate, "--json"]);
    assert_eq!(list["data"]["links"].as_array().unwrap().len(), 1);
    assert_eq!(
        list["data"]["links"][0]["attributes"]["reason"],
        "same root cause"
    );
}

#[test]
fn external_blocker_is_a_link_and_blocks_readiness() {
    let repo = TestRepo::new();
    repo.init();
    let task = repo.add("Task", &[]);

    repo.ok_json(&[
        "link",
        "add",
        &task,
        "--type",
        "blocks",
        "--external",
        "jira:PROJ-123",
        "--url",
        "https://jira.example.com/browse/PROJ-123",
        "--json",
    ]);

    let show = repo.ok_json(&["show", &task, "--json"]);
    let links = link_json(&show["data"]);
    assert_eq!(links.len(), 1);
    assert_eq!(links[0]["target"]["system"], "jira");
    assert_eq!(links[0]["target"]["id"], "PROJ-123");
    assert_eq!(
        links[0]["target"]["url"],
        "https://jira.example.com/browse/PROJ-123"
    );
    assert_eq!(links[0]["blocking"], true);

    // The external blocker keeps the task out of `ready`.
    assert_eq!(show["data"]["ready"], false);
    assert_eq!(show["data"]["blocking"][0], "jira:PROJ-123");
    let ready = repo.ok_json(&["ready", "--json"]);
    let ids: Vec<&str> = ready["data"]["issues"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|task| task["id"].as_str())
        .collect();
    assert!(!ids.contains(&task.as_str()));

    // Claiming the task is refused while an external blocker is open.
    let (status, error) = repo.json(&["claim", &task, "--by", "devin", "--json"]);
    assert_ne!(status, 0, "{error}");

    // Removing the link unblocks the task.
    repo.ok_json(&[
        "link",
        "remove",
        &task,
        "--external",
        "jira:PROJ-123",
        "--json",
    ]);
    let show = repo.ok_json(&["show", &task, "--json"]);
    assert_eq!(link_json(&show["data"]).len(), 0);
    assert_eq!(show["data"]["ready"], true);
}

#[test]
fn non_blocking_external_links_do_not_block() {
    let repo = TestRepo::new();
    repo.init();
    let task = repo.add("Task", &[]);

    repo.ok_json(&[
        "link",
        "add",
        &task,
        "--type",
        "relates-to",
        "--external",
        "confluence:PAGE-9",
        "--json",
    ]);

    let show = repo.ok_json(&["show", &task, "--json"]);
    assert_eq!(show["data"]["ready"], true);
    assert!(show["data"]["blocking"].as_array().unwrap().is_empty());
}

#[test]
fn custom_link_types_and_blocking_overrides_work() {
    let repo = TestRepo::new();
    repo.init();
    let a = repo.add("A", &[]);
    let b = repo.add("B", &[]);

    // A custom type with --blocking participates in readiness and cycles.
    repo.ok_json(&[
        "link",
        "add",
        &a,
        "--type",
        "custom-gate",
        "--to",
        &b,
        "--blocking",
        "--json",
    ]);
    let show = repo.ok_json(&["show", &a, "--json"]);
    let links = link_json(&show["data"]);
    assert_eq!(links[0]["type"], "custom-gate");
    assert_eq!(links[0]["blocking"], true);
    assert_eq!(show["data"]["ready"], false);

    let (status, error) = repo.json(&["link", "add", &b, "--type", "blocks", "--to", &a, "--json"]);
    assert_ne!(status, 0, "{error}");

    // --no-blocking overrides the blocks default.
    let c = repo.add("C", &[]);
    repo.ok_json(&[
        "link",
        "add",
        &c,
        "--type",
        "blocks",
        "--to",
        &a,
        "--no-blocking",
        "--json",
    ]);
    let show = repo.ok_json(&["show", &c, "--json"]);
    assert_eq!(show["data"]["ready"], true);
}

#[test]
fn link_remove_matches_type_and_target() {
    let repo = TestRepo::new();
    repo.init();
    let a = repo.add("A", &[]);
    let b = repo.add("B", &[]);

    repo.ok_json(&[
        "link",
        "add",
        &a,
        "--type",
        "relates-to",
        "--to",
        &b,
        "--json",
    ]);
    repo.ok_json(&[
        "link",
        "add",
        &a,
        "--type",
        "duplicates",
        "--to",
        &b,
        "--json",
    ]);

    // Removing with a type only removes that link.
    repo.ok_json(&[
        "link",
        "remove",
        &a,
        "--type",
        "relates-to",
        "--to",
        &b,
        "--json",
    ]);
    let show = repo.ok_json(&["show", &a, "--json"]);
    let links = link_json(&show["data"]);
    assert_eq!(links.len(), 1);
    assert_eq!(links[0]["type"], "duplicates");

    // Removing an absent link is idempotent.
    let removed = repo.ok_json(&[
        "link",
        "remove",
        &a,
        "--type",
        "discovered-from",
        "--to",
        &b,
        "--json",
    ]);
    assert_eq!(removed["meta"]["changed"], false);
}

#[test]
fn depend_and_undepend_still_manage_blocking_links() {
    let repo = TestRepo::new();
    repo.init();
    let first = repo.add("First", &[]);
    let second = repo.add("Second", &[]);
    repo.ok_json(&["depend", second.as_str(), first.as_str(), "--json"]);

    let show = repo.ok_json(&["show", &second, "--json"]);
    let links = link_json(&show["data"]);
    assert_eq!(links.len(), 1);
    assert_eq!(links[0]["type"], "blocks");
    assert_eq!(links[0]["target"], first);
    assert_eq!(links[0]["blocking"], true);

    repo.ok_json(&["undepend", &second, &first, "--json"]);
    let show = repo.ok_json(&["show", &second, "--json"]);
    assert_eq!(link_json(&show["data"]).len(), 0);
    assert_eq!(show["data"]["ready"], true);
}

#[test]
fn batch_link_operations_apply_typed_links() {
    let repo = TestRepo::new();
    repo.init();
    let a = repo.add("A", &[]);
    let b = repo.add("B", &[]);

    let plan = serde_json::json!({
        "operations": [
            {
                "op": "link",
                "id": a,
                "type": "blocks",
                "to": b,
                "attributes": { "via": "batch" }
            },
            {
                "op": "link",
                "id": b,
                "type": "relates-to",
                "external": "linear:ENG-1"
            }
        ]
    });
    let plan_path = repo.path().join("plan.json");
    std::fs::write(&plan_path, plan.to_string()).unwrap();
    repo.ok_json(&["batch", "--file", plan_path.to_str().unwrap(), "--json"]);

    let show = repo.ok_json(&["show", &a, "--json"]);
    let links = link_json(&show["data"]);
    assert_eq!(links[0]["type"], "blocks");
    assert_eq!(links[0]["attributes"]["via"], "batch");
    assert_eq!(show["data"]["ready"], false);

    let show = repo.ok_json(&["show", &b, "--json"]);
    let links = link_json(&show["data"]);
    assert_eq!(links[0]["target"]["system"], "linear");
    assert_eq!(links[0]["blocking"], false);
}

mod common;

use common::TestRepo;

const KEY: &str = "planner:plan-7:step-3";

fn board_commits(repo: &TestRepo) -> usize {
    let output = repo.git_in(&repo.board_path(), &["rev-list", "--count", "HEAD"]);
    String::from_utf8_lossy(&output.stdout)
        .trim()
        .parse()
        .unwrap()
}

#[test]
fn add_with_source_key_stores_and_returns_it() {
    let repo = TestRepo::new();
    repo.init();
    let added = repo.ok_json(&["add", "Step", "--source-key", KEY, "--json"]);
    assert_eq!(added["data"]["source_key"], KEY);
    assert_eq!(added["meta"]["matched"], false);
    let id = added["data"]["id"].as_str().unwrap();
    let show = repo.ok_json(&["show", id, "--json"]);
    assert_eq!(show["data"]["source_key"], KEY);
}

#[test]
fn add_with_existing_source_key_returns_the_same_task() {
    let repo = TestRepo::new();
    repo.init();
    let first = repo.add("Step", &["--source-key", KEY]);
    let commits = board_commits(&repo);
    let again = repo.ok_json(&["add", "Other title", "--source-key", KEY, "--json"]);
    assert_eq!(again["data"]["id"], first);
    assert_eq!(again["data"]["title"], "Step");
    assert_eq!(again["meta"]["matched"], true);
    assert_eq!(board_commits(&repo), commits);
    let list = repo.ok_json(&["list", "--json"]);
    assert_eq!(list["data"]["count"], 1);
}

#[test]
fn source_key_must_be_valid() {
    let repo = TestRepo::new();
    repo.init();
    let long = "k".repeat(201);
    for key in ["", "has space", long.as_str()] {
        let (_, error) = repo.err_json(&["add", "Step", "--source-key", key, "--json"]);
        assert_eq!(
            error["error"]["code"], "INVALID_SOURCE_KEY",
            "{key:?}: {error}"
        );
    }
    let list = repo.ok_json(&["list", "--json"]);
    assert_eq!(list["data"]["count"], 0);
}

#[test]
fn task_without_source_key_reports_null() {
    let repo = TestRepo::new();
    repo.init();
    let id = repo.add("Plain", &[]);
    let show = repo.ok_json(&["show", &id, "--json"]);
    assert!(show["data"]["source_key"].is_null());
    assert!(!repo.read_task(&id).contains("source_key"));
}

#[test]
fn duplicate_source_key_is_reported() {
    let repo = TestRepo::new();
    repo.init();
    let first = repo.add("One", &["--source-key", KEY]);
    let second = repo.add("Two", &[]);
    let content = repo.read_task(&second);
    let rest = content.strip_prefix("---\n").unwrap();
    repo.write_task(&second, &format!("---\nsource_key: {KEY}\n{rest}"));
    assert!(repo.read_task(&first).contains(KEY));

    let (status, doctor) = repo.json(&["doctor", "--json"]);
    assert!(status == 0 || status == 1, "{doctor}");
    let codes: Vec<&str> = doctor["data"]["issues"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|issue| issue["code"].as_str())
        .collect();
    assert!(codes.contains(&"DUPLICATE_SOURCE_KEY"), "{doctor}");

    let (_, error) = repo.err_json(&["add", "Three", "--source-key", KEY, "--json"]);
    assert_eq!(error["error"]["code"], "DUPLICATE_SOURCE_KEY");
}

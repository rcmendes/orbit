mod common;

use common::TestRepo;
use std::path::Path;

/// A git repository with one commit inside `dir`, returning its path.
fn git_repo(repo: &TestRepo, dir: &Path) -> std::path::PathBuf {
    std::fs::create_dir_all(dir).unwrap();
    for args in [
        &["init", "-q"][..],
        &["config", "user.name", "Test"][..],
        &["config", "user.email", "test@example.com"][..],
        &["commit", "--allow-empty", "-q", "-m", "init"][..],
    ] {
        let output = repo.git_in(dir, args);
        assert!(
            output.status.success(),
            "git {args:?}: {}",
            String::from_utf8_lossy(&output.stderr)
        );
    }
    dir.to_path_buf()
}

fn repo_add(repo: &TestRepo, board: &Path, member: &Path, name: &str) -> serde_json::Value {
    repo.ok_json(&[
        "--dir",
        board.to_str().unwrap(),
        "repo",
        "add",
        member.to_str().unwrap(),
        "--name",
        name,
        "--json",
    ])
}

#[test]
fn repo_add_records_explicit_membership_and_the_link_pointer() {
    let repo = TestRepo::new();
    repo.init();
    let member = git_repo(&repo, &repo.path().join("api"));

    let value = repo_add(&repo, &repo.orbit_path(), &member, "api");

    assert_eq!(value["data"]["repository"]["name"], "api");
    assert_eq!(value["data"]["repository"]["kind"], "explicit");
    assert_eq!(
        value["data"]["repository"]["path"].as_str().unwrap(),
        dunce::canonicalize(&member).unwrap().to_str().unwrap()
    );
    // The member's link pointer resolves commands back to this Board.
    assert!(member.join(".git/orbit/link").is_file());
    let config = std::fs::read_to_string(repo.board_path().join("config.toml")).unwrap();
    assert!(config.contains("[repositories]"), "{config}");
    assert!(config.contains("api"), "{config}");
}

#[test]
fn repo_add_defaults_the_name_to_the_repository_folder() {
    let repo = TestRepo::new();
    repo.init();
    let member = git_repo(&repo, &repo.path().join("billing"));

    let value = repo.ok_json(&[
        "--dir",
        repo.orbit_path().to_str().unwrap(),
        "repo",
        "add",
        member.to_str().unwrap(),
        "--json",
    ]);

    assert_eq!(value["data"]["repository"]["name"], "billing");
}

#[test]
fn repo_add_rejects_the_boards_own_repository_and_duplicates() {
    let repo = TestRepo::new();
    repo.init();

    // The Board's own repository cannot join as a member.
    let (status, value) = repo.json(&["repo", "add", repo.path().to_str().unwrap(), "--json"]);
    assert_eq!(status, 1, "{value}");
    assert_eq!(value["error"]["code"], "INVALID_ARGUMENT");

    let member = git_repo(&repo, &repo.path().join("api"));
    repo_add(&repo, &repo.orbit_path(), &member, "api");
    let (status, value) = repo.json(&[
        "repo",
        "add",
        member.to_str().unwrap(),
        "--name",
        "api",
        "--json",
    ]);
    assert_eq!(status, 1, "{value}");
    assert_eq!(value["error"]["code"], "INVALID_ARGUMENT");
}

#[test]
fn repo_add_from_a_worktree_records_the_main_checkout() {
    let repo = TestRepo::new();
    repo.init();
    let member = git_repo(&repo, &repo.path().join("api"));
    let worktree_dir = tempfile::tempdir().unwrap();
    let worktree = worktree_dir.path().join("api-wt");
    let output = repo.git_in(
        &member,
        &[
            "worktree",
            "add",
            worktree.to_str().unwrap(),
            "-b",
            "feature",
        ],
    );
    assert!(output.status.success());

    let value = repo_add(&repo, &repo.orbit_path(), &worktree, "api");

    // Membership and the link pointer live on the main checkout, shared by
    // every worktree.
    assert_eq!(
        value["data"]["repository"]["path"].as_str().unwrap(),
        dunce::canonicalize(&member).unwrap().to_str().unwrap()
    );
    assert!(member.join(".git/orbit/link").is_file());
    let listed = repo.ok_json_in(&worktree, &["repo", "list", "--json"]);
    let repositories = listed["data"]["repositories"].as_array().unwrap();
    assert_eq!(repositories.len(), 1, "{listed}");
    assert_eq!(repositories[0]["name"], "api");
}

#[test]
fn repo_list_reports_child_and_explicit_members() {
    let repo = TestRepo::new();
    let parent = tempfile::tempdir().unwrap();
    let anchor = parent.path().join("acme");
    std::fs::create_dir(&anchor).unwrap();
    repo.ok_json_in(
        parent.path(),
        &[
            "init",
            anchor.to_str().unwrap(),
            "--agents",
            "none",
            "--json",
        ],
    );
    // A child Repository discovered beneath the anchor.
    let child = git_repo(&repo, &anchor.join("web"));
    // An explicit member elsewhere.
    let explicit = git_repo(&repo, &parent.path().join("tools"));
    repo_add(&repo, &anchor, &explicit, "tools");

    let value = repo.ok_json_in(&anchor, &["repo", "list", "--json"]);
    let repositories = value["data"]["repositories"].as_array().unwrap();
    assert_eq!(repositories.len(), 2, "{value}");
    let child_entry = repositories
        .iter()
        .find(|entry| entry["name"] == "web")
        .unwrap_or_else(|| panic!("missing child member: {value}"));
    assert_eq!(child_entry["kind"], "child");
    assert_eq!(
        child_entry["path"].as_str().unwrap(),
        dunce::canonicalize(&child).unwrap().to_str().unwrap()
    );
    let explicit_entry = repositories
        .iter()
        .find(|entry| entry["name"] == "tools")
        .unwrap();
    assert_eq!(explicit_entry["kind"], "explicit");
}

#[test]
fn repo_remove_drops_membership_and_clears_the_link() {
    let repo = TestRepo::new();
    repo.init();
    // The member lives outside the Board's repository so discovery cannot
    // fall back to the parent `.orbit` once the link is gone.
    let outside = tempfile::tempdir().unwrap();
    let member = git_repo(&repo, &outside.path().join("api"));
    repo_add(&repo, &repo.orbit_path(), &member, "api");
    assert!(member.join(".git/orbit/link").is_file());

    let value = repo.ok_json(&["repo", "remove", "api", "--json"]);
    assert_eq!(value["data"]["link_cleared"], true, "{value}");
    assert!(!member.join(".git/orbit/link").exists());

    let listed = repo.ok_json(&["repo", "list", "--json"]);
    assert_eq!(
        listed["data"]["repositories"].as_array().unwrap().len(),
        0,
        "{listed}"
    );
    let config = std::fs::read_to_string(repo.board_path().join("config.toml")).unwrap();
    assert!(!config.contains("[repositories]"), "{config}");

    // The member no longer resolves the Board.
    let (status, value) = repo.json_in(&member, &["list", "--json"]);
    assert_eq!(status, 1, "{value}");
    assert_eq!(value["error"]["code"], "BOARD_NOT_FOUND");
}

#[test]
fn repo_remove_rejects_a_discovered_child_repository() {
    let repo = TestRepo::new();
    let parent = tempfile::tempdir().unwrap();
    let anchor = parent.path().join("acme");
    std::fs::create_dir(&anchor).unwrap();
    repo.ok_json_in(
        parent.path(),
        &[
            "init",
            anchor.to_str().unwrap(),
            "--agents",
            "none",
            "--json",
        ],
    );
    git_repo(&repo, &anchor.join("web"));

    let (status, value) = repo.json_in(
        &anchor,
        &["repo", "remove", "web", "--json"],
    );

    assert_eq!(status, 1, "{value}");
    assert_eq!(value["error"]["code"], "INVALID_ARGUMENT");
    assert!(
        value["error"]["message"]
            .as_str()
            .unwrap()
            .contains("child"),
        "{value}"
    );
}

#[test]
fn repo_refresh_writes_the_member_inventory_and_reports_deltas() {
    let repo = TestRepo::new();
    repo.init();
    let anchor = repo.orbit_path();
    let inventory = repo.orbit_path().join("cache/member-repositories.json");

    let first = repo.ok_json(&["repo", "refresh", "--json"]);
    assert_eq!(first["data"]["repositories"].as_array().unwrap().len(), 0);
    assert!(inventory.is_file(), "{first}");

    let member = git_repo(&repo, &repo.path().join("api"));
    repo_add(&repo, &anchor, &member, "api");
    let second = repo.ok_json(&["repo", "refresh", "--json"]);
    assert_eq!(second["data"]["added"], serde_json::json!(["api"]));
    assert_eq!(second["data"]["removed"], serde_json::json!([]));
    let record = &second["data"]["repositories"][0];
    assert_eq!(record["name"], "api");
    assert_eq!(record["kind"], "explicit");
    assert_eq!(
        record["path"].as_str().unwrap(),
        dunce::canonicalize(&member).unwrap().to_str().unwrap()
    );

    // Dropping the membership shows up as a removal on refresh.
    repo.ok_json(&["repo", "remove", "api", "--json"]);
    let third = repo.ok_json(&["repo", "refresh", "--json"]);
    assert_eq!(third["data"]["removed"], serde_json::json!(["api"]));
    assert_eq!(
        third["data"]["repositories"].as_array().unwrap().len(),
        0
    );
}

#[test]
fn orbit_add_records_the_member_repository_on_the_task() {
    let repo = TestRepo::new();
    repo.init();
    let member = git_repo(&repo, &repo.path().join("api"));
    repo_add(&repo, &repo.orbit_path(), &member, "api");

    // A task created inside the member Repository records `repo: api`.
    let added = repo.ok_json_in(&member, &["add", "Member task", "--json"]);
    let id = added["data"]["id"].as_str().unwrap();
    let content = repo.read_task(id);
    assert!(content.contains("repo: api"), "{content}");

    // A task created at the Board root records no Repository.
    let root_task = repo.add("Root task", &[]);
    let content = repo.read_task(&root_task);
    assert!(!content.contains("repo:"), "{content}");
}

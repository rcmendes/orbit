mod common;

use std::path::Path;

use common::TestRepo;

fn write_legacy_board_at(path: &Path) {
    std::fs::create_dir_all(path.join("tasks")).unwrap();
    std::fs::create_dir_all(path.join("comments")).unwrap();
    std::fs::write(
        path.join("config.toml"),
        "[board]\nbranch = \"orbit\"\n\n[id]\nprefix = \"orb-\"\n",
    )
    .unwrap();
}

fn assert_warning(value: &serde_json::Value, needle: &str) {
    assert!(
        value["meta"]["warnings"]
            .as_array()
            .unwrap()
            .iter()
            .any(|warning| warning.as_str().unwrap().contains(needle)),
        "missing `{needle}` warning in {value}"
    );
}

fn canonical(path: &Path) -> String {
    dunce::canonicalize(path)
        .unwrap()
        .to_str()
        .unwrap()
        .to_string()
}

#[test]
fn link_reports_a_missing_target_with_link_not_found() {
    let repo = TestRepo::new();
    let missing = repo.path().join("does-not-exist");

    let (status, value) = repo.json(&["link", missing.to_str().unwrap(), "--json"]);

    assert_eq!(status, 1);
    assert_eq!(value["error"]["code"], "LINK_NOT_FOUND");
    let message = value["error"]["message"].as_str().unwrap();
    assert!(message.contains("does-not-exist"), "{message}");
    assert!(message.contains("orbit init"), "{message}");
}

#[test]
fn a_linked_legacy_board_keeps_warning_on_later_commands() {
    let repo = TestRepo::new();
    let parent = tempfile::tempdir().unwrap();
    let board = parent.path().join("old-board");
    write_legacy_board_at(&board);

    let linked = repo.link_at(&board);
    assert_warning(&linked, "LEGACY_LAYOUT");

    let listed = repo.ok_json(&["list", "--json"]);
    assert_eq!(listed["data"]["count"], 0);
    assert_warning(&listed, "LEGACY_LAYOUT");
}

#[test]
fn a_linked_legacy_workspace_keeps_warning_on_later_commands() {
    let repo = TestRepo::new();
    let parent = tempfile::tempdir().unwrap();
    let workspace = parent.path().join("acme");
    let board = workspace.join("board");
    write_legacy_board_at(&board);

    let linked = repo.link_at(&workspace);
    assert_eq!(linked["data"]["board"].as_str().unwrap(), canonical(&board));
    assert_warning(&linked, "LEGACY_LAYOUT");

    let listed = repo.ok_json(&["list", "--json"]);
    assert_eq!(listed["data"]["count"], 0);
    assert_warning(&listed, "LEGACY_LAYOUT");
}

#[test]
fn link_warns_when_it_shadows_the_legacy_local_board() {
    let repo = TestRepo::new();
    let legacy = repo.path().join(".git/orbit/board");
    write_legacy_board_at(&legacy);

    let parent = tempfile::tempdir().unwrap();
    let shared = parent.path().join(".orbit");
    let other = TestRepo::new();
    other.init_at(&shared);

    let linked = repo.link_at(&shared);

    assert_eq!(linked["data"]["shadowed"], true);
    assert_eq!(
        linked["data"]["shadowed_boards"],
        serde_json::json!([canonical(&legacy)])
    );
    assert_warning(&linked, "shadowed");
    assert_warning(&linked, &canonical(&legacy));
    assert!(legacy.join("config.toml").is_file());
}

#[test]
fn unlink_refreshes_managed_files_without_a_local_board() {
    let repo = TestRepo::new();
    let parent = tempfile::tempdir().unwrap();
    let shared = parent.path().join(".orbit");
    let other = TestRepo::new();
    other.init_at(&shared);

    repo.link_at(&shared);
    repo.ok_json(&["setup", "claude", "--json"]);
    let linked = repo.read_json(".mcp.json");
    assert!(
        !linked["mcpServers"]["orbit"]["args"]
            .as_array()
            .unwrap()
            .iter()
            .any(|arg| arg == "--dir"),
        "{linked}"
    );
    let instructions = repo
        .read("CLAUDE.md")
        .replace("never edit task files", "stale managed content");
    std::fs::write(repo.path().join("CLAUDE.md"), instructions).unwrap();

    let unlinked = repo.ok_json(&["unlink", "--json"]);

    assert_eq!(unlinked["data"]["unlinked"], true);
    assert_eq!(unlinked["data"]["board"], serde_json::Value::Null);
    assert_warning(&unlinked, "no local Board");
    assert!(repo.read("CLAUDE.md").contains("never edit task files"));
}

#[test]
fn link_from_a_worktree_writes_the_common_pointer_and_shares_the_lock() {
    let repo = TestRepo::new();
    repo.init();

    let parent = tempfile::tempdir().unwrap();
    let shared = parent.path().join(".orbit");
    let other = TestRepo::new();
    other.init_at(&shared);

    let worktree_dir = tempfile::tempdir().unwrap();
    let worktree = worktree_dir.path().join("wt");
    repo.add_worktree(&worktree, "feature");

    repo.ok_json_in(&worktree, &["link", shared.to_str().unwrap(), "--json"]);

    let pointer = repo.path().join(".git/orbit/link");
    assert_eq!(
        std::fs::read_to_string(&pointer).unwrap().trim(),
        canonical(&shared)
    );

    let id = repo.ok_json_in(&worktree, &["add", "From a worktree", "--json"])["data"]["id"]
        .as_str()
        .unwrap()
        .to_string();
    let listed = repo.ok_json(&["list", "--all", "--json"]);
    assert!(
        listed["data"]["tasks"]
            .as_array()
            .unwrap()
            .iter()
            .any(|task| task["id"] == id),
        "{listed}"
    );

    repo.ok_json(&["claim", id.as_str(), "--by", "main-agent", "--json"]);
    let shown = repo.ok_json_in(&worktree, &["show", id.as_str(), "--json"]);
    assert_eq!(shown["data"]["claimed_by"], "main-agent");

    assert!(shared.join(".lock").is_file());
    assert!(!repo.path().join(".orbit/.lock").exists());
}

#[test]
fn link_shares_an_orbit_folder_between_repositories() {
    let parent = tempfile::tempdir().unwrap();
    let workspace = parent.path().join("acme");
    let shared = workspace.join(".orbit");

    let first = TestRepo::new();
    first.init_at(&shared);
    first.link_at(&shared);
    let id = first.add("Cross-repo task", &[]);

    let second = TestRepo::new();
    second.link_at(&shared);
    let listed = second.ok_json(&["list", "--all", "--json"]);
    assert_eq!(listed["data"]["tasks"][0]["id"], id);

    second.ok_json(&["done", id.as_str(), "--by", "second", "--json"]);
    let shown = first.ok_json(&["show", id.as_str(), "--json"]);
    assert_eq!(shown["data"]["status"], "done");
    assert_eq!(shown["data"]["claimed_by"], serde_json::Value::Null);
}

#[test]
fn unlink_from_a_worktree_returns_to_the_main_checkout_board() {
    let repo = TestRepo::new();
    repo.init();
    let local_id = repo.add("Local task", &[]);

    let parent = tempfile::tempdir().unwrap();
    let shared = parent.path().join(".orbit");
    let other = TestRepo::new();
    other.init_at(&shared);

    repo.link_at(&shared);
    let shared_id = repo.add("Shared task", &[]);
    assert_ne!(local_id, shared_id);

    let worktree_dir = tempfile::tempdir().unwrap();
    let worktree = worktree_dir.path().join("wt");
    repo.add_worktree(&worktree, "feature");

    let unlinked = repo.ok_json_in(&worktree, &["unlink", "--json"]);

    assert_eq!(
        unlinked["data"]["board"].as_str().unwrap(),
        canonical(&repo.board_path())
    );
    assert!(!repo.path().join(".git/orbit/link").exists());
    let listed = repo.ok_json_in(&worktree, &["list", "--all", "--json"]);
    let ids: Vec<&str> = listed["data"]["tasks"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|task| task["id"].as_str())
        .collect();
    assert_eq!(ids, vec![local_id.as_str()]);
}

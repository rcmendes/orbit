mod common;

use common::TestRepo;

fn write_legacy_board(repo: &TestRepo) {
    let board = repo.path().join(".git/orbit/board");
    std::fs::create_dir_all(board.join("tasks")).unwrap();
    std::fs::create_dir_all(board.join("comments")).unwrap();
    std::fs::write(
        board.join("config.toml"),
        "[board]\nbranch = \"orbit\"\n\n[id]\nprefix = \"orb-\"\n",
    )
    .unwrap();
}

fn init_child_repo(repo: &TestRepo, path: &std::path::Path) {
    std::fs::create_dir_all(path).unwrap();
    for args in [
        &["init", "-q"][..],
        &["config", "user.name", "Test"][..],
        &["config", "user.email", "test@example.com"][..],
        &["commit", "--allow-empty", "-q", "-m", "init"][..],
    ] {
        let output = repo.git_in(path, args);
        assert!(
            output.status.success(),
            "git {args:?} failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
    }
}

fn assert_warning(value: &serde_json::Value, code: &str) {
    assert!(
        value["meta"]["warnings"]
            .as_array()
            .unwrap()
            .iter()
            .any(|warning| warning.as_str().unwrap().contains(code)),
        "missing {code} warning in {value}"
    );
}

#[test]
fn init_creates_the_orbit_folder_worktree_and_excludes_it() {
    let repo = TestRepo::new();

    let value = repo.ok_json(&["init", "--agents", "none", "--slug", "orb", "--json"]);

    let dir = repo.path().join(".orbit");
    assert_eq!(value["data"]["storage"], "worktree");
    assert!(dir.join("config.toml").is_file());
    assert!(!repo.path().join(".git/orbit/board").exists());

    let config = std::fs::read_to_string(dir.join("config.toml")).unwrap();
    assert!(config.contains("storage = \"worktree\""), "{config}");

    let exclude = std::fs::read_to_string(repo.path().join(".git/info/exclude")).unwrap();
    assert!(exclude.lines().any(|line| line == "/.orbit/"), "{exclude}");

    let status = repo.git(&["status", "--porcelain", "--", ".orbit"]);
    assert!(
        String::from_utf8_lossy(&status.stdout).trim().is_empty(),
        "{}",
        String::from_utf8_lossy(&status.stdout)
    );
}

#[test]
fn commands_from_a_nested_directory_find_the_project_orbit_folder() {
    let repo = TestRepo::new();
    repo.init();
    let id = repo.add("Nested discovery", &[]);
    let nested = repo.path().join("src/deep");
    std::fs::create_dir_all(&nested).unwrap();

    let listed = repo.ok_json_in(&nested, &["list", "--all", "--json"]);

    assert_eq!(listed["data"]["tasks"][0]["id"], id);
}

#[test]
fn a_secondary_worktree_resolves_the_main_orbit_folder() {
    let repo = TestRepo::new();
    repo.init();
    let id = repo.add("Shared with worktrees", &[]);
    let worktree_dir = tempfile::tempdir().unwrap();
    let worktree = worktree_dir.path().join("wt");
    repo.add_worktree(&worktree, "feature");

    let listed = repo.ok_json_in(&worktree, &["list", "--all", "--json"]);

    assert_eq!(listed["data"]["tasks"][0]["id"], id);
}

#[test]
fn explicit_dir_and_orbit_dir_name_the_orbit_folder_directly() {
    let first = TestRepo::new();
    first.init();
    let first_id = first.add("First board", &[]);

    let second = TestRepo::new();
    second.init();
    let second_id = second.add("Second board", &[]);

    let first_dir = first.path().join(".orbit");
    let second_dir = second.path().join(".orbit");

    let listed = second.ok_json(&[
        "--dir",
        first_dir.to_str().unwrap(),
        "list",
        "--all",
        "--json",
    ]);
    assert_eq!(listed["data"]["tasks"][0]["id"], first_id);

    let (status, value) = second.json_with_env(
        &["list", "--all", "--json"],
        &[("ORBIT_DIR", second_dir.to_str().unwrap())],
    );
    assert_eq!(status, 0, "{value}");
    assert_eq!(value["data"]["tasks"][0]["id"], second_id);
}

#[test]
fn the_data_lock_lives_inside_the_orbit_folder() {
    let repo = TestRepo::new();
    repo.init();

    repo.ok_json(&["list", "--json"]);

    assert!(repo.path().join(".orbit/.lock").is_file());
    assert!(!repo.path().join(".git/orbit/lock").exists());
}

#[test]
fn a_legacy_local_board_resolves_with_a_warning_and_keeps_its_lock() {
    let repo = TestRepo::new();
    write_legacy_board(&repo);

    let value = repo.ok_json(&["list", "--json"]);

    assert_eq!(value["data"]["count"], 0);
    assert_warning(&value, "LEGACY_LAYOUT");
    assert!(repo.path().join(".git/orbit/lock").is_file());
    assert!(!repo.path().join(".git/orbit/board/.lock").exists());
}

#[test]
fn an_explicit_legacy_local_board_keeps_its_old_lock() {
    let repo = TestRepo::new();
    write_legacy_board(&repo);
    let board = repo.legacy_board_path();

    let value = repo.ok_json(&["--dir", board.to_str().unwrap(), "list", "--json"]);

    assert_warning(&value, "LEGACY_LAYOUT");
    assert!(repo.path().join(".git/orbit/lock").is_file());
    assert!(!board.join(".lock").exists());
}

#[test]
fn a_new_orbit_folder_and_a_legacy_board_emit_an_ambiguous_warning() {
    let repo = TestRepo::new();
    repo.init();
    write_legacy_board(&repo);

    let value = repo.ok_json(&["list", "--json"]);

    assert_warning(&value, "AMBIGUOUS_BOARD");
}

#[test]
fn init_reports_an_ambiguous_layout_when_reusing_the_new_board() {
    let repo = TestRepo::new();
    repo.init();
    write_legacy_board(&repo);

    let value = repo.ok_json(&["init", "--agents", "none", "--json"]);

    assert_warning(&value, "AMBIGUOUS_BOARD");
}

#[test]
fn init_refuses_a_second_repository_board() {
    let repo = TestRepo::new();
    repo.init();
    let other = repo.path().join("other/.orbit");

    let (status, value) = repo.json(&[
        "init",
        other.to_str().unwrap(),
        "--agents",
        "none",
        "--json",
    ]);

    assert_eq!(status, 1);
    assert_eq!(value["error"]["code"], "ALREADY_INITIALIZED");
}

#[test]
fn init_refuses_when_only_a_legacy_board_exists() {
    let repo = TestRepo::new();
    write_legacy_board(&repo);

    let (status, value) = repo.json(&["init", "--agents", "none", "--json"]);

    assert_eq!(status, 1);
    assert_eq!(value["error"]["code"], "ALREADY_INITIALIZED");
    assert!(!repo.path().join(".orbit").exists());
}

#[test]
fn a_repository_mode_board_records_its_storage() {
    let repo = TestRepo::new();
    let parent = tempfile::tempdir().unwrap();
    let board = parent.path().join("shared");
    let internal = board.join(".orbit/board");

    let value = repo.ok_json(&[
        "init",
        board.to_str().unwrap(),
        "--agents",
        "none",
        "--json",
    ]);

    assert_eq!(value["data"]["storage"], "repository");
    let config = std::fs::read_to_string(internal.join("config.toml")).unwrap();
    assert!(config.contains("storage = \"repository\""), "{config}");
}

#[test]
fn init_reattaches_an_orphaned_worktree_without_losing_tasks() {
    let repo = TestRepo::new();
    repo.init();
    let id = repo.add("Survives a forced clean", &[]);
    let dir = repo.path().join(".orbit");

    std::fs::remove_dir_all(&dir).unwrap();
    repo.ok_json(&["init", "--agents", "none", "--json"]);

    assert!(dir.join("config.toml").is_file());
    let listed = repo.ok_json(&["list", "--all", "--json"]);
    assert_eq!(listed["data"]["tasks"][0]["id"], id);
}

#[test]
fn the_global_orbit_folder_is_a_last_resort() {
    let repo = TestRepo::new();
    let home = tempfile::tempdir().unwrap();
    let home_str = home.path().to_str().unwrap();
    let global = home.path().join(".orbit");
    let env = [("HOME", home_str), ("USERPROFILE", home_str)];

    let output = repo.orbit_with_env(&["init", "--global", "--agents", "none", "--json"], &env);
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let init: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(init["data"]["storage"], "repository");
    assert_eq!(init["data"]["global"], true);
    assert_eq!(init["data"]["prefix"], "orb-");
    assert!(global.join("config.toml").is_file());

    let (status, value) = repo.json_with_env(&["list", "--json"], &env);

    assert_eq!(status, 0, "{value}");
    assert_eq!(value["data"]["count"], 0);
}

#[test]
fn an_explicit_legacy_board_root_is_still_rejected_when_it_has_no_board() {
    let repo = TestRepo::new();
    let empty = tempfile::tempdir().unwrap();

    let (status, value) = repo.json(&["--dir", empty.path().to_str().unwrap(), "list", "--json"]);

    assert_eq!(status, 1);
    assert_eq!(value["error"]["code"], "BOARD_NOT_FOUND");
}

#[test]
fn link_accepts_an_orbit_folder_directly() {
    let first = TestRepo::new();
    first.init();
    let id = first.add("Shared from .orbit", &[]);
    let dir = first.path().join(".orbit");

    let second = TestRepo::new();
    second.ok_json(&["link", dir.to_str().unwrap(), "--json"]);

    let listed = second.ok_json(&["list", "--all", "--json"]);
    assert_eq!(listed["data"]["tasks"][0]["id"], id);
}

#[test]
fn a_legacy_workspace_root_link_resolves_the_nested_board_with_a_warning() {
    let creator = TestRepo::new();
    let parent = tempfile::tempdir().unwrap();
    let workspace = parent.path().join("acme");
    creator.ok_json_in(
        parent.path(),
        &["init", "acme/board", "--agents", "none", "--json"],
    );

    let joiner = TestRepo::new();
    let linked = joiner.ok_json(&["link", workspace.to_str().unwrap(), "--json"]);
    assert_eq!(
        linked["data"]["board"].as_str().unwrap(),
        dunce::canonicalize(workspace.join("board"))
            .unwrap()
            .to_str()
            .unwrap()
    );
    assert_warning(&linked, "LEGACY_LAYOUT");

    let listed = joiner.ok_json(&["list", "--json"]);
    assert_eq!(listed["data"]["count"], 0);
}

#[test]
fn a_new_board_whose_path_ends_in_orbit_board_uses_its_own_lock() {
    let repo = TestRepo::new();
    let parent = tempfile::tempdir().unwrap();
    let board = parent.path().join("orbit/board");

    repo.ok_json(&[
        "init",
        board.to_str().unwrap(),
        "--agents",
        "none",
        "--json",
    ]);
    repo.ok_json(&["--dir", board.to_str().unwrap(), "list", "--json"]);

    assert!(board.join(".lock").is_file());
    assert!(!parent.path().join("orbit/lock").exists());
}

#[test]
fn a_legacy_config_without_storage_still_loads_without_a_command_warning() {
    let repo = TestRepo::new();
    let parent = tempfile::tempdir().unwrap();
    let board = parent.path().join("old-board");
    std::fs::create_dir_all(board.join("tasks")).unwrap();
    std::fs::create_dir_all(board.join("comments")).unwrap();
    std::fs::write(
        board.join("config.toml"),
        "[board]\nbranch = \"orbit\"\n\n[id]\nprefix = \"orb-\"\n",
    )
    .unwrap();

    let value = repo.ok_json(&["--dir", board.to_str().unwrap(), "list", "--json"]);

    assert_eq!(value["data"]["count"], 0);
    assert!(value["meta"].get("warnings").is_none(), "{value}");
}

#[test]
fn init_creates_a_workspace_board_inside_a_non_git_root() {
    let repo = TestRepo::new();
    let parent = tempfile::tempdir().unwrap();
    let workspace = parent.path().join("acme");
    std::fs::create_dir(&workspace).unwrap();
    let cache = workspace.join(".orbit/cache");
    std::fs::create_dir_all(&cache).unwrap();
    std::fs::write(cache.join("keep.json"), "cached snapshot").unwrap();

    let value = repo.ok_json_in(
        parent.path(),
        &[
            "init",
            workspace.to_str().unwrap(),
            "--agents",
            "none",
            "--json",
        ],
    );

    let internal = workspace.join(".orbit");
    assert_eq!(
        value["data"]["board"],
        dunce::canonicalize(internal.join("board"))
            .unwrap()
            .to_str()
            .unwrap()
    );
    assert!(internal.join("board/config.toml").is_file());
    assert!(internal.join("cache").is_dir());
    assert_eq!(
        std::fs::read_to_string(cache.join("keep.json")).unwrap(),
        "cached snapshot"
    );
    assert!(!workspace.join("config.toml").exists());
}

#[test]
fn workspace_board_resolves_from_root_and_every_nested_repository_level() {
    let repo = TestRepo::new();
    let parent = tempfile::tempdir().unwrap();
    let workspace = parent.path().join("acme");
    std::fs::create_dir(&workspace).unwrap();
    repo.ok_json_in(
        parent.path(),
        &[
            "init",
            workspace.to_str().unwrap(),
            "--agents",
            "none",
            "--json",
        ],
    );
    let child = workspace.join("api");
    init_child_repo(&repo, &child);
    let nested = child.join("src/deep");
    std::fs::create_dir_all(&nested).unwrap();

    let initialized = repo.ok_json_in(&child, &["init", "--agents", "none", "--json"]);
    assert_eq!(initialized["data"]["created"], false);
    assert_eq!(
        initialized["data"]["board"],
        dunce::canonicalize(workspace.join(".orbit/board"))
            .unwrap()
            .to_str()
            .unwrap()
    );
    assert!(!child.join(".orbit").exists());

    let added = repo.ok_json_in(&workspace, &["add", "Workspace task", "--json"]);
    let id = added["data"]["id"].as_str().unwrap();
    for cwd in [&workspace, &child, &nested] {
        let listed = repo.ok_json_in(cwd, &["list", "--all", "--json"]);
        assert_eq!(
            listed["data"]["tasks"][0]["id"],
            id,
            "from {}",
            cwd.display()
        );
    }
    assert!(!child.join(".git/orbit/link").exists());
}

#[test]
fn dir_and_orbit_dir_accept_a_workspace_root() {
    let repo = TestRepo::new();
    let parent = tempfile::tempdir().unwrap();
    let workspace = parent.path().join("acme");
    std::fs::create_dir(&workspace).unwrap();
    repo.ok_json_in(
        parent.path(),
        &[
            "init",
            workspace.to_str().unwrap(),
            "--agents",
            "none",
            "--json",
        ],
    );
    let added = repo.ok_json_in(&workspace, &["add", "Workspace override", "--json"]);
    let id = added["data"]["id"].as_str().unwrap();

    let listed = repo.ok_json(&[
        "--dir",
        workspace.to_str().unwrap(),
        "list",
        "--all",
        "--json",
    ]);
    assert_eq!(listed["data"]["tasks"][0]["id"], id);

    let (status, listed) = repo.json_with_env(
        &["list", "--all", "--json"],
        &[("ORBIT_DIR", workspace.to_str().unwrap())],
    );
    assert_eq!(status, 0, "{listed}");
    assert_eq!(listed["data"]["tasks"][0]["id"], id);
}

#[test]
fn an_outside_repository_can_link_to_the_workspace_root() {
    let creator = TestRepo::new();
    let parent = tempfile::tempdir().unwrap();
    let workspace = parent.path().join("acme");
    std::fs::create_dir(&workspace).unwrap();
    creator.ok_json_in(
        parent.path(),
        &[
            "init",
            workspace.to_str().unwrap(),
            "--agents",
            "none",
            "--json",
        ],
    );
    let added = creator.ok_json_in(&workspace, &["add", "Linked Workspace task", "--json"]);
    let id = added["data"]["id"].as_str().unwrap();

    let outside = TestRepo::new();
    outside.ok_json(&["link", workspace.to_str().unwrap(), "--json"]);
    let listed = outside.ok_json(&["list", "--all", "--json"]);

    assert_eq!(listed["data"]["tasks"][0]["id"], id);
}

#[test]
fn workspace_init_refuses_to_shadow_a_child_repository_board() {
    let repo = TestRepo::new();
    let parent = tempfile::tempdir().unwrap();
    let workspace = parent.path().join("acme");
    std::fs::create_dir(&workspace).unwrap();
    let child = workspace.join("api");
    init_child_repo(&repo, &child);
    repo.ok_json_in(&child, &["init", "--agents", "none", "--json"]);

    let (status, value) = repo.json_in(
        parent.path(),
        &[
            "init",
            workspace.to_str().unwrap(),
            "--agents",
            "none",
            "--json",
        ],
    );

    assert_eq!(status, 1);
    assert_eq!(value["error"]["code"], "ALREADY_INITIALIZED");
    let message = value["error"]["message"].as_str().unwrap();
    assert!(
        message.contains(&format!("api{}.orbit", std::path::MAIN_SEPARATOR)),
        "unexpected message: {message}"
    );
    assert!(!workspace.join(".orbit/board/config.toml").exists());
}

#[test]
fn workspace_init_refuses_an_unrecognized_orbit_layout() {
    let repo = TestRepo::new();
    let parent = tempfile::tempdir().unwrap();
    let workspace = parent.path().join("acme");
    std::fs::create_dir_all(workspace.join(".orbit")).unwrap();
    std::fs::write(workspace.join(".orbit/user-data"), "keep me").unwrap();

    let (status, value) = repo.json_in(
        parent.path(),
        &[
            "init",
            workspace.to_str().unwrap(),
            "--agents",
            "none",
            "--json",
        ],
    );

    assert_eq!(status, 1);
    assert_eq!(value["error"]["code"], "ALREADY_INITIALIZED");
    assert!(workspace.join(".orbit/user-data").is_file());
}

#[test]
fn a_child_repository_board_is_reported_as_ambiguous_after_workspace_init() {
    let repo = TestRepo::new();
    let parent = tempfile::tempdir().unwrap();
    let workspace = parent.path().join("acme");
    std::fs::create_dir(&workspace).unwrap();
    repo.ok_json_in(
        parent.path(),
        &[
            "init",
            workspace.to_str().unwrap(),
            "--agents",
            "none",
            "--json",
        ],
    );
    let child = workspace.join("api");
    init_child_repo(&repo, &child);
    repo.ok_json_in(&child, &["init", ".orbit", "--agents", "none", "--json"]);
    let added = repo.ok_json_in(&workspace, &["add", "Workspace task", "--json"]);
    let id = added["data"]["id"].as_str().unwrap();

    let listed = repo.ok_json_in(&child, &["list", "--all", "--json"]);

    assert_eq!(listed["data"]["tasks"][0]["id"], id);
    assert_warning(&listed, "AMBIGUOUS_BOARD");
}

#[test]
fn workspace_init_reports_the_legacy_dot_orbit_board_for_migration() {
    let repo = TestRepo::new();
    let parent = tempfile::tempdir().unwrap();
    let workspace = parent.path().join("acme");
    let legacy = workspace.join(".orbit");
    std::fs::create_dir_all(legacy.join("tasks")).unwrap();
    std::fs::create_dir_all(legacy.join("comments")).unwrap();
    std::fs::write(
        legacy.join("config.toml"),
        "[board]\nbranch = \"orbit\"\n\n[id]\nprefix = \"orb-\"\n",
    )
    .unwrap();

    let (status, value) = repo.json_in(
        parent.path(),
        &[
            "init",
            workspace.to_str().unwrap(),
            "--agents",
            "none",
            "--json",
        ],
    );

    assert_eq!(status, 1);
    assert_eq!(value["error"]["code"], "CONFIG_ERROR");
    assert!(
        value["error"]["message"]
            .as_str()
            .unwrap()
            .contains("--dir")
    );
    assert!(legacy.join("config.toml").is_file());
    assert!(!workspace.join(".orbit/board").exists());

    let compatible = repo.ok_json(&["--dir", legacy.to_str().unwrap(), "list", "--json"]);
    assert_eq!(compatible["data"]["count"], 0);
}

#[test]
fn scoped_board_access_holds_and_releases_the_selected_lock() {
    use orbit_tracker::board::{AccessMode, BoardAccess};
    let repo = TestRepo::new();
    repo.init();
    let root = repo.path().join(".orbit");
    let access = BoardAccess::open(Some(&root), AccessMode::Write).unwrap();
    assert_eq!(access.board().root, dunce::canonicalize(&root).unwrap());
    assert!(access.board().lock_shared().is_err());
    drop(access);
    let first = BoardAccess::open(Some(&root), AccessMode::Inspect).unwrap();
    let second = BoardAccess::open(Some(&root), AccessMode::Read).unwrap();
    assert!(first.board().lock_exclusive().is_err());
    drop(first);
    drop(second);
    let _writer = BoardAccess::open(Some(&root), AccessMode::Write).unwrap();
}

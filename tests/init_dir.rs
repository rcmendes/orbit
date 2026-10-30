mod common;

use common::TestRepo;

fn canonical(path: &std::path::Path) -> String {
    dunce::canonicalize(path)
        .unwrap()
        .to_str()
        .unwrap()
        .to_string()
}

#[test]
fn init_dir_creates_a_repository_board() {
    let repo = TestRepo::new();
    let parent = tempfile::tempdir().unwrap();
    let folder = parent.path().join("acme");
    let board = folder.join(".orbit/board");

    let value = repo.ok_json_in(
        parent.path(),
        &["init", "acme", "--agents", "none", "--json"],
    );

    assert_eq!(value["data"]["created"], true);
    assert_eq!(value["data"]["branch"], "orbit");
    assert_eq!(value["data"]["storage"], "repository");
    assert_eq!(value["data"]["prefix"], "acme-");
    assert_eq!(value["data"]["board"].as_str().unwrap(), canonical(&board));
    assert!(board.join("config.toml").exists());
    assert!(board.join(".git").exists());
    assert!(folder.join(".orbit/cache").is_dir());
}

#[test]
fn init_dir_is_idempotent() {
    let repo = TestRepo::new();
    let parent = tempfile::tempdir().unwrap();

    let first = repo.ok_json_in(
        parent.path(),
        &["init", "acme", "--agents", "none", "--json"],
    );
    let second = repo.ok_json_in(
        parent.path(),
        &["init", "acme", "--agents", "none", "--json"],
    );

    assert_eq!(first["data"]["created"], true);
    assert_eq!(second["data"]["created"], false);
    assert_eq!(second["data"]["board"], first["data"]["board"]);
}

#[test]
fn init_dir_inside_a_repository_creates_an_excluded_worktree() {
    let repo = TestRepo::new();
    let dir = repo.path().join("shared-board");

    let value = repo.ok_json(&["init", "shared-board", "--agents", "none", "--json"]);

    assert_eq!(value["data"]["storage"], "worktree");
    assert_eq!(value["data"]["exclude"], "/shared-board/");
    assert!(dir.join("config.toml").is_file());
    let exclude = repo.read(".git/info/exclude");
    assert!(exclude.lines().any(|line| line == "/shared-board/"));
    assert!(!repo.path().join(".git/orbit/link").exists());
}

#[test]
fn init_dir_does_not_write_agent_files_without_agents() {
    let repo = TestRepo::new();
    let parent = tempfile::tempdir().unwrap();

    repo.ok_json_in(
        parent.path(),
        &["init", "acme", "--agents", "none", "--json"],
    );

    assert!(!repo.exists("CLAUDE.md"));
    assert!(!repo.exists("AGENTS.md"));
    assert!(!repo.exists(".mcp.json"));
    assert!(!repo.exists("opencode.json"));
    assert!(!repo.path().join(".git/orbit/link").exists());
}

#[test]
fn init_defaults_to_the_current_directory_orbit_folder() {
    let repo = TestRepo::new();
    let sub = repo.path().join("sub");
    std::fs::create_dir_all(&sub).unwrap();

    let value = repo.ok_json_in(&sub, &["init", "--agents", "none", "--json"]);

    assert_eq!(value["data"]["prefix"], "sub-");
    assert_eq!(value["data"]["storage"], "worktree");
    assert!(sub.join(".orbit/config.toml").is_file());
    assert!(!repo.path().join(".orbit").exists());
}

#[test]
fn init_print_reports_the_exclusion_without_writing() {
    let repo = TestRepo::new();

    let value = repo.ok_json(&[
        "init",
        "shared-board",
        "--agents",
        "none",
        "--print",
        "--json",
    ]);

    assert_eq!(value["data"]["preview"], true);
    assert_eq!(value["data"]["exclude"], "/shared-board/");
    assert!(!repo.path().join("shared-board").exists());
    let exclude = repo.read(".git/info/exclude");
    assert!(!exclude.lines().any(|line| line == "/shared-board/"));
    assert!(!repo.path().join(".git/orbit/link").exists());
}

#[test]
fn removed_init_surface_is_rejected() {
    let repo = TestRepo::new();

    for args in [
        &["workspace", "acme"][..],
        &["init", "--board", "acme"][..],
        &["init", "--local"][..],
    ] {
        let output = repo.orbit(args);
        assert!(
            !output.status.success(),
            "{} unexpectedly succeeded",
            args.join(" ")
        );
    }
}

#[test]
fn init_global_rejects_a_positional_dir() {
    let repo = TestRepo::new();

    let output = repo.orbit(&["init", "acme", "--global"]);

    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("--global"), "{stderr}");
}

#[test]
fn init_dir_prints_a_next_step() {
    let repo = TestRepo::new();
    let parent = tempfile::tempdir().unwrap();

    let output = repo.orbit_in(parent.path(), &["init", "acme", "--agents", "none"]);
    let stdout = String::from_utf8_lossy(&output.stdout);

    assert!(output.status.success());
    assert!(stdout.contains("Orbit setup complete."), "{stdout}");
    assert!(stdout.contains("1. Board (created)"), "{stdout}");
    assert!(
        stdout.contains("Workspace Board at <workspace>/.orbit/board"),
        "{stdout}"
    );
    assert!(!stdout.contains("orbit link"), "{stdout}");
    assert!(stdout.contains("orbit setup <client>"), "{stdout}");
    assert!(stdout.contains("3. Verify"), "{stdout}");
}

#[test]
fn workspace_init_preserves_existing_workspace_files() {
    let repo = TestRepo::new();
    let parent = tempfile::tempdir().unwrap();
    let folder = parent.path().join("acme");
    std::fs::create_dir_all(&folder).unwrap();
    std::fs::write(folder.join("keep.txt"), "keep").unwrap();

    let (status, value) = repo.json_in(
        parent.path(),
        &["init", "acme", "--agents", "none", "--json"],
    );

    assert_eq!(status, 0, "{value}");
    assert_eq!(value["data"]["storage"], "repository");
    assert_eq!(
        std::fs::read_to_string(folder.join("keep.txt")).unwrap(),
        "keep"
    );
    assert!(folder.join(".orbit/board/config.toml").exists());
}

#[test]
fn init_dir_refuses_a_file_path() {
    let repo = TestRepo::new();
    let parent = tempfile::tempdir().unwrap();
    std::fs::write(parent.path().join("acme"), "not a directory").unwrap();

    let (status, value) = repo.json_in(
        parent.path(),
        &["init", "acme", "--agents", "none", "--json"],
    );

    assert_eq!(status, 1);
    assert_eq!(
        std::fs::read_to_string(parent.path().join("acme")).unwrap(),
        "not a directory"
    );
    assert_eq!(value["error"]["code"], "IO_ERROR");
}

#[test]
fn init_dir_option_creates_that_dir_without_linking() {
    let repo = TestRepo::new();
    let parent = tempfile::tempdir().unwrap();
    let target = parent.path().join("acme");

    let value = repo.ok_json(&[
        "--dir",
        target.to_str().unwrap(),
        "init",
        "--agents",
        "none",
        "--json",
    ]);

    assert_eq!(
        value["data"]["board"].as_str().unwrap(),
        canonical(&target.join(".orbit/board"))
    );
    assert_eq!(value["data"]["storage"], "repository");
    assert!(!repo.path().join(".git/orbit/link").exists());
}

#[test]
fn init_in_a_worktree_reuses_the_main_checkout_board() {
    let repo = TestRepo::new();
    repo.init();
    let worktree_dir = tempfile::tempdir().unwrap();
    let worktree = worktree_dir.path().join("wt");
    repo.add_worktree(&worktree, "feature");

    let value = repo.ok_json_in(&worktree, &["init", "--agents", "none", "--json"]);

    assert_eq!(value["data"]["created"], false);
    assert_eq!(
        value["data"]["board"].as_str().unwrap(),
        canonical(&repo.path().join(".orbit"))
    );
    assert!(!worktree.join(".orbit").exists());
}

#[test]
fn init_dot_at_the_repository_root_uses_the_orbit_folder() {
    let repo = TestRepo::new();

    let value = repo.ok_json(&["init", ".", "--agents", "none", "--json"]);

    assert_eq!(value["data"]["created"], true);
    assert_eq!(value["data"]["storage"], "worktree");
    assert_eq!(
        value["data"]["board"].as_str().unwrap(),
        canonical(&repo.path().join(".orbit"))
    );
    assert!(repo.path().join(".orbit/config.toml").is_file());
    assert!(!repo.path().join(".git/orbit/link").exists());
}

#[test]
fn init_dot_at_the_repository_root_is_idempotent() {
    let repo = TestRepo::new();

    repo.ok_json(&["init", ".", "--agents", "none", "--json"]);
    let second = repo.ok_json(&["init", ".", "--agents", "none", "--json"]);

    assert_eq!(second["data"]["created"], false);
    assert_eq!(
        second["data"]["board"].as_str().unwrap(),
        canonical(&repo.path().join(".orbit"))
    );
}

#[test]
fn init_dot_print_reports_the_repository_orbit_folder() {
    let repo = TestRepo::new();

    let value = repo.ok_json(&["init", ".", "--agents", "none", "--print", "--json"]);

    assert_eq!(value["data"]["preview"], true);
    assert_eq!(value["data"]["storage"], "worktree");
    assert_eq!(
        std::path::Path::new(value["data"]["board"].as_str().unwrap()),
        dunce::canonicalize(repo.path()).unwrap().join(".orbit")
    );
    assert_eq!(value["data"]["exclude"], "/.orbit/");
    assert!(!repo.path().join(".orbit").exists());
}

#[test]
fn init_repository_root_path_uses_the_orbit_folder() {
    let repo = TestRepo::new();
    let project = repo.named_repo("project");

    let value = repo.ok_json(&["init", "project", "--agents", "none", "--json"]);

    assert_eq!(
        value["data"]["board"].as_str().unwrap(),
        canonical(&project.join(".orbit"))
    );
    assert_eq!(value["data"]["prefix"], "project-");
    assert!(project.join(".orbit/config.toml").is_file());
}

#[test]
fn init_dot_in_a_linked_worktree_reuses_the_main_checkout_board() {
    let repo = TestRepo::new();
    repo.init();
    let worktree_dir = tempfile::tempdir().unwrap();
    let worktree = worktree_dir.path().join("wt");
    repo.add_worktree(&worktree, "feature");

    let value = repo.ok_json_in(&worktree, &["init", ".", "--agents", "none", "--json"]);

    assert_eq!(value["data"]["created"], false);
    assert_eq!(
        value["data"]["board"].as_str().unwrap(),
        canonical(&repo.path().join(".orbit"))
    );
}

#[test]
fn init_refuses_a_non_empty_target_directory() {
    let repo = TestRepo::new();
    let target = repo.path().join("populated");
    std::fs::create_dir_all(&target).unwrap();
    std::fs::write(target.join("keep.txt"), "keep").unwrap();

    let (status, value) = repo.json(&["init", "populated", "--agents", "none", "--json"]);

    assert_eq!(status, 1);
    assert_eq!(value["error"]["code"], "CONFIG_ERROR");
    let message = value["error"]["message"].as_str().unwrap();
    assert!(message.contains("must be empty"), "{message}");
    assert_eq!(
        std::fs::read_to_string(target.join("keep.txt")).unwrap(),
        "keep"
    );
    assert!(!target.join("config.toml").exists());
}

#[test]
fn init_refreshes_a_missing_worktree_exclusion() {
    let repo = TestRepo::new();
    repo.ok_json(&["init", "shared-board", "--agents", "none", "--json"]);
    let exclude_path = repo.path().join(".git/info/exclude");
    let original = std::fs::read_to_string(&exclude_path).unwrap();
    let removed = original
        .lines()
        .filter(|line| *line != "/shared-board/")
        .collect::<Vec<_>>()
        .join("\n");
    std::fs::write(&exclude_path, format!("{removed}\n")).unwrap();

    repo.ok_json(&["init", "shared-board", "--agents", "none", "--json"]);

    let repaired = std::fs::read_to_string(&exclude_path).unwrap();
    assert!(repaired.lines().any(|line| line == "/shared-board/"));
}

#[test]
fn init_ignores_the_global_fallback_as_a_conflict() {
    let repo = TestRepo::new();
    let home = repo.path().to_str().unwrap();
    let env = [("HOME", home), ("USERPROFILE", home)];

    let global = repo.orbit_with_env(&["init", "--global", "--agents", "none", "--json"], &env);
    assert!(
        global.status.success(),
        "{}",
        String::from_utf8_lossy(&global.stderr)
    );

    let (status, value) =
        repo.json_with_env(&["init", "sub/.orbit", "--agents", "none", "--json"], &env);

    assert_eq!(status, 0, "{value}");
    assert_eq!(value["data"]["storage"], "worktree");
    assert!(repo.path().join("sub/.orbit/config.toml").is_file());
}

#[test]
fn init_refuses_a_second_board_in_the_same_working_tree() {
    let repo = TestRepo::new();
    let sub = repo.path().join("sub");
    std::fs::create_dir_all(&sub).unwrap();

    repo.ok_json_in(&sub, &["init", "--agents", "none", "--json"]);
    let (status, value) = repo.json(&["init", "--agents", "none", "--json"]);

    assert_eq!(status, 1);
    assert_eq!(value["error"]["code"], "ALREADY_INITIALIZED");
    assert!(!repo.path().join(".orbit").exists());
}

#[test]
fn init_global_conflicts_with_dir() {
    let repo = TestRepo::new();
    let target = repo.path().join("elsewhere");

    let (status, value) = repo.json(&[
        "--dir",
        target.to_str().unwrap(),
        "init",
        "--global",
        "--agents",
        "none",
        "--json",
    ]);

    assert_eq!(status, 1);
    assert_eq!(value["error"]["code"], "INVALID_ARGUMENT");
    assert!(!target.exists());
}

#[test]
fn init_global_conflicts_with_orbit_dir_env() {
    let repo = TestRepo::new();

    let (status, value) = repo.json_with_env(
        &["init", "--global", "--agents", "none", "--json"],
        &[("ORBIT_DIR", repo.path().to_str().unwrap())],
    );

    assert_eq!(status, 1);
    assert_eq!(value["error"]["code"], "INVALID_ARGUMENT");
}

#[test]
fn init_positional_dir_wins_over_dir_override() {
    let repo = TestRepo::new();
    let parent = tempfile::tempdir().unwrap();
    let ignored = repo.path().join("ignored");
    let target = parent.path().join("chosen");

    let value = repo.ok_json(&[
        "--dir",
        ignored.to_str().unwrap(),
        "init",
        target.to_str().unwrap(),
        "--agents",
        "none",
        "--json",
    ]);

    assert_eq!(
        value["data"]["board"].as_str().unwrap(),
        canonical(&target.join(".orbit/board"))
    );
    assert!(target.join(".orbit/board/config.toml").exists());
    assert!(!ignored.exists());
}

#[test]
fn link_warns_for_a_legacy_board_without_storage() {
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

    let linked = repo.link_at(&board);

    let warnings = linked["meta"]["warnings"].as_array().unwrap();
    assert!(
        warnings
            .iter()
            .any(|warning| warning.as_str().unwrap().contains("LEGACY_LAYOUT"))
    );
}

#[test]
fn link_accepts_a_legacy_workspace_root() {
    let creator = TestRepo::new();
    let parent = tempfile::tempdir().unwrap();
    let workspace = parent.path().join("acme");
    creator.ok_json_in(
        parent.path(),
        &["init", "acme/board", "--agents", "none", "--json"],
    );

    let joiner = TestRepo::new();
    let linked = joiner.link_at(&workspace);

    assert_eq!(
        linked["data"]["board"].as_str().unwrap(),
        canonical(&workspace.join("board"))
    );
    let warnings = linked["meta"]["warnings"].as_array().unwrap();
    assert!(
        warnings
            .iter()
            .any(|warning| warning.as_str().unwrap().contains("LEGACY_LAYOUT"))
    );
}

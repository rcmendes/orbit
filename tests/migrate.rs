mod common;

use common::TestRepo;
use std::path::Path;

fn task_content(title: &str, extra_frontmatter: &str) -> String {
    format!(
        "---\ntitle: {title}\nstatus: todo\n{extra_frontmatter}created_at: 2026-01-01T00:00:00Z\nupdated_at: 2026-01-01T00:00:00Z\n---\n\nBody\n"
    )
}

fn write_task(board: &Path, id: &str, content: &str) {
    std::fs::write(board.join("tasks").join(format!("{id}.md")), content).unwrap();
}

fn commit_all(repo: &TestRepo, cwd: &Path, message: &str) {
    for args in [&["add", "-A"][..], &["commit", "-q", "-m", message][..]] {
        let output = repo.git_in(cwd, args);
        assert!(
            output.status.success(),
            "git {args:?} failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
    }
}

fn config_value(board: &Path) -> toml::Value {
    toml::from_str(&std::fs::read_to_string(board.join("config.toml")).unwrap()).unwrap()
}

#[test]
fn migrate_reports_nothing_to_move_on_a_standalone_board() {
    let repo = TestRepo::new();
    repo.init();

    let value = repo.ok_json(&["migrate", "--json"]);

    assert_eq!(value["data"]["migrated"], false);
    assert!(
        value["data"]["reason"]
            .as_str()
            .unwrap()
            .contains("already"),
        "{value}"
    );
}

#[test]
fn migrate_previews_the_legacy_local_board_without_writing() {
    let repo = TestRepo::new();
    let board = repo.legacy_board_path();
    repo.legacy_board_files(&board, "orb-");
    write_task(&board, "orb-a7b2c9", &task_content("Legacy task", ""));
    let config_before = std::fs::read_to_string(board.join("config.toml")).unwrap();

    let value = repo.ok_json(&["migrate", "--json"]);

    assert_eq!(value["data"]["applied"], false);
    assert_eq!(value["data"]["needs_layout"], true);
    assert_eq!(value["data"]["layout"], "convert");
    assert_eq!(
        value["data"]["to"].as_str().unwrap(),
        dunce::canonicalize(repo.path())
            .unwrap()
            .join(".orbit")
            .join("board")
            .to_str()
            .unwrap()
    );
    assert_eq!(value["data"]["tasks"], 1);
    // The preview is read-only: the legacy Board is untouched.
    assert!(board.join("config.toml").is_file());
    assert!(!repo.board_path().exists());
    assert_eq!(
        std::fs::read_to_string(board.join("config.toml")).unwrap(),
        config_before
    );
}

#[test]
fn migrate_apply_moves_the_legacy_local_board() {
    let repo = TestRepo::new();
    let board = repo.legacy_board_path();
    repo.legacy_board_files(&board, "orb-");
    write_task(&board, "orb-a7b2c9", &task_content("Legacy task", ""));

    let value = repo.ok_json(&["migrate", "--apply", "--json"]);

    assert_eq!(value["data"]["applied"], true);
    assert!(value["data"]["commit"].as_str().is_some(), "{value}");
    assert!(!board.exists());
    let migrated = repo.board_path();
    assert!(migrated.join("config.toml").is_file());
    assert!(migrated.join("tasks").join("orb-a7b2c9.md").is_file());
    // The Board folder is its own repository on branch `orbit`.
    assert!(migrated.join(".git").exists());
    let branch = repo.git_in(&migrated, &["rev-parse", "--abbrev-ref", "HEAD"]);
    assert_eq!(String::from_utf8_lossy(&branch.stdout).trim(), "orbit");
    let log = repo.git_in(&migrated, &["log", "--format=%s", "-1"]);
    assert_eq!(
        String::from_utf8_lossy(&log.stdout).trim(),
        "orbit: migrate board"
    );
    // `.orbit/cache` sits beside the Board folder inside the Orbit folder.
    assert!(repo.orbit_path().join("cache").is_dir());
    // The Orbit folder inside the code repository is hidden via
    // `.git/info/exclude`, never `.gitignore`.
    let exclude = std::fs::read_to_string(repo.path().join(".git/info/exclude")).unwrap();
    assert!(exclude.lines().any(|line| line == "/.orbit/"), "{exclude}");
    assert!(!repo.path().join(".gitignore").exists());

    let listed = repo.ok_json(&["list", "--all", "--json"]);
    assert_eq!(listed["data"]["tasks"][0]["id"], "orb-a7b2c9");
}

#[test]
fn migrate_apply_is_idempotent() {
    let repo = TestRepo::new();
    let board = repo.legacy_board_path();
    repo.legacy_board_files(&board, "orb-");

    repo.ok_json(&["migrate", "--apply", "--json"]);
    let again = repo.ok_json(&["migrate", "--apply", "--json"]);

    assert_eq!(again["data"]["migrated"], false);
    assert!(
        again["data"]["reason"]
            .as_str()
            .unwrap()
            .contains("already"),
        "{again}"
    );
}

#[test]
fn migrate_apply_converts_a_worktree_backed_board() {
    let repo = TestRepo::new();
    let board = repo.orbit_path();
    repo.legacy_worktree_board(&board);
    write_task(&board, "orb-b8c3d0", &task_content("Worktree task", ""));
    commit_all(&repo, &board, "task");

    let value = repo.ok_json(&["migrate", "--apply", "--json"]);

    assert_eq!(value["data"]["applied"], true, "{value}");
    let migrated = repo.board_path();
    assert!(migrated.join("config.toml").is_file());
    assert!(migrated.join("tasks").join("orb-b8c3d0.md").is_file());
    // `.orbit` is now only the Orbit folder: `board/` plus `cache/`.
    assert!(!board.join("config.toml").exists());
    assert!(board.join("cache").is_dir());
    // The code repository no longer carries the Board's `orbit` branch.
    let branches = repo.git(&["branch", "--list", "orbit"]);
    assert!(String::from_utf8_lossy(&branches.stdout).trim().is_empty());
    // `board.storage` is gone from the migrated config.
    let config = std::fs::read_to_string(migrated.join("config.toml")).unwrap();
    assert!(!config.contains("storage"), "{config}");

    let listed = repo.ok_json(&["list", "--all", "--json"]);
    assert_eq!(listed["data"]["tasks"][0]["id"], "orb-b8c3d0");
}

#[test]
fn migrate_apply_keeps_uncommitted_worktree_board_edits() {
    let repo = TestRepo::new();
    let board = repo.orbit_path();
    repo.legacy_worktree_board(&board);
    write_task(&board, "orb-b8c3d0", &task_content("Committed task", ""));
    commit_all(&repo, &board, "task");
    // Left uncommitted, as with `board.auto_commit = false`.
    write_task(&board, "orb-e1f6a3", &task_content("Pending task", ""));

    let value = repo.ok_json(&["migrate", "--apply", "--json"]);

    assert_eq!(value["data"]["applied"], true, "{value}");
    let migrated = repo.board_path();
    assert!(migrated.join("tasks").join("orb-e1f6a3.md").is_file());
    assert!(migrated.join("tasks").join("orb-b8c3d0.md").is_file());
}

#[test]
fn migrate_apply_refuses_an_existing_target_before_touching_the_worktree() {
    let repo = TestRepo::new();
    let board = repo.orbit_path();
    repo.legacy_worktree_board(&board);
    write_task(&board, "orb-b8c3d0", &task_content("Worktree task", ""));
    commit_all(&repo, &board, "task");
    std::fs::create_dir_all(repo.board_path()).unwrap();

    let (code, value) = repo.err_json(&["migrate", "--apply", "--json"]);

    assert_ne!(code, 0);
    assert!(value.to_string().contains("already exists"), "{value}");
    // The worktree Board and its branch are untouched.
    assert!(board.join("tasks").join("orb-b8c3d0.md").is_file());
    let branches = repo.git(&["branch", "--list", "orbit"]);
    assert!(!String::from_utf8_lossy(&branches.stdout).trim().is_empty());
}

#[test]
fn migrate_apply_moves_a_legacy_workspace_board() {
    let creator = TestRepo::new();
    let parent = tempfile::tempdir().unwrap();
    let workspace = parent.path().join("acme");
    let board = workspace.join("board");
    creator.legacy_board_files(&board, "orb-");
    write_task(&board, "orb-c9d4e1", &task_content("Workspace task", ""));

    let value = creator.ok_json(&[
        "--dir",
        workspace.to_str().unwrap(),
        "migrate",
        "--apply",
        "--json",
    ]);

    assert_eq!(value["data"]["applied"], true, "{value}");
    assert!(!board.exists());
    let migrated = workspace.join(".orbit/board");
    assert!(migrated.join("config.toml").is_file());
    assert!(workspace.join(".orbit/cache").is_dir());
    assert!(migrated.join("tasks").join("orb-c9d4e1.md").is_file());

    // A Repository added as a member keeps resolving the Board through its
    // link pointer.
    let joiner = TestRepo::new();
    creator.ok_json(&[
        "--dir",
        workspace.to_str().unwrap(),
        "repo",
        "add",
        joiner.path().to_str().unwrap(),
        "--name",
        "joiner",
        "--json",
    ]);
    let listed = joiner.ok_json(&["list", "--all", "--json"]);
    assert_eq!(listed["data"]["tasks"][0]["id"], "orb-c9d4e1");
}

#[test]
fn migrate_rewrites_github_links_repo_labels_and_areas() {
    let repo = TestRepo::new();
    let board = repo.legacy_board_path();
    repo.legacy_board_files(&board, "orb-");
    write_task(
        &board,
        "orb-d0e5f2",
        &task_content(
            "Linked task",
            "area: cli\nlabels:\n- repo:api\n- bug\ngithub:\n  repo: acme/api\n  number: 42\n",
        ),
    );

    let value = repo.ok_json(&["migrate", "--apply", "--json"]);

    assert_eq!(value["data"]["applied"], true, "{value}");
    assert_eq!(value["data"]["github_links"], 1);
    assert_eq!(value["data"]["repo_labels"], 1);

    let migrated = repo.board_path();
    let task = std::fs::read_to_string(migrated.join("tasks").join("orb-d0e5f2.md")).unwrap();
    assert!(!task.contains("github:"), "{task}");
    assert!(task.contains("remote:"), "{task}");
    assert!(task.contains("tracker: github"), "{task}");
    assert!(task.contains("project: acme/api"), "{task}");
    assert!(task.contains("id:"), "{task}");
    assert!(task.contains("42"), "{task}");
    assert!(task.contains("repo: api"), "{task}");
    assert!(task.contains("- bug"), "{task}");
    assert!(!task.contains("repo:api"), "{task}");

    // `board.areas` is seeded from the Area values the Tasks use.
    let config = config_value(&migrated);
    let areas = config["board"]["areas"].as_array().unwrap();
    assert_eq!(areas.len(), 1);
    assert_eq!(areas[0].as_str().unwrap(), "cli");
}

#[test]
fn migrate_rewrites_the_github_section_to_remote_projects() {
    let repo = TestRepo::new();
    let board = repo.legacy_board_path();
    repo.legacy_board_files(&board, "orb-");
    std::fs::write(
        board.join("config.toml"),
        "[board]\nbranch = \"orbit\"\n\n[id]\nprefix = \"orb-\"\n\n[github]\nmode = \"basic\"\nrepositories = [\"acme/api\", \"acme/web\"]\nauto_push = true\n",
    )
    .unwrap();

    let value = repo.ok_json(&["migrate", "--apply", "--json"]);

    assert_eq!(value["data"]["applied"], true, "{value}");
    assert_eq!(value["data"]["remote_projects"], 2);

    let config = config_value(&repo.board_path());
    // Every config.toml carries the default `[github.issue_fields]` table;
    // what matters is that no provider settings survive under `[github]`.
    let github = config["github"].as_table().unwrap();
    for key in ["mode", "organization", "repositories", "auto_push"] {
        assert!(!github.contains_key(key), "{github:?}");
    }
    let remote = &config["remote"];
    assert!(remote["auto_push"].as_bool().unwrap());
    let projects = remote["projects"].as_array().unwrap();
    assert_eq!(projects.len(), 2);
    for project in projects {
        assert_eq!(project["tracker"].as_str().unwrap(), "github");
        assert!(
            ["acme/api", "acme/web"].contains(&project["project"].as_str().unwrap()),
            "{project}"
        );
    }
    // The GitHub adapter settings survive under `remote.github` without the
    // project list, which now lives in `remote.projects`.
    assert_eq!(remote["github"]["mode"].as_str().unwrap(), "basic");
    assert!(
        remote["github"]
            .get("repositories")
            .is_none_or(|repos| repos.as_array().is_some_and(Vec::is_empty)),
        "{remote}"
    );
}

#[test]
fn migrate_keeps_existing_remote_projects_and_counts_them() {
    let repo = TestRepo::new();
    let board = repo.legacy_board_path();
    repo.legacy_board_files(&board, "orb-");
    std::fs::write(
        board.join("config.toml"),
        "[board]\nbranch = \"orbit\"\n\n[id]\nprefix = \"orb-\"\n\n[github]\nrepositories = [\"acme/api\"]\n\n[[remote.projects]]\ntracker = \"github\"\nproject = \"acme/api\"\n\n[[remote.projects]]\ntracker = \"github\"\nproject = \"acme/ops\"\n",
    )
    .unwrap();

    let preview = repo.ok_json(&["migrate", "--json"]);
    // acme/api is already a Remote Project; only acme/ops is added by hand
    // and acme/api comes from [github] — the merged set has two projects.
    assert_eq!(preview["data"]["remote_projects"], 2, "{preview}");

    repo.ok_json(&["migrate", "--apply", "--json"]);
    let config = config_value(&repo.board_path());
    let projects = config["remote"]["projects"].as_array().unwrap();
    assert_eq!(projects.len(), 2, "{config}");
}

#[test]
fn migrate_apply_on_a_visible_board_keeps_the_pointer() {
    let repo = TestRepo::new();
    let parent = tempfile::tempdir().unwrap();
    // A visible Board folder anchored by `.orbit/board` pointer file, but
    // still a plain directory — a pre-ADR-0019 Board that needs `.git`.
    let workspace = parent.path().join("acme");
    let board = workspace.join("tracker");
    repo.legacy_board_files(&board, "orb-");
    let orbit = workspace.join(".orbit");
    std::fs::create_dir_all(&orbit).unwrap();
    std::fs::write(orbit.join("board"), format!("{}\n", board.display())).unwrap();

    let value = repo.ok_json(&[
        "--dir",
        workspace.to_str().unwrap(),
        "migrate",
        "--apply",
        "--json",
    ]);

    assert_eq!(value["data"]["applied"], true, "{value}");
    // The visible folder stays in place; it just becomes a repository.
    assert!(board.join(".git").exists());
    assert!(board.join("config.toml").is_file());
    // The pointer file still names the same Board folder.
    let pointer = std::fs::read_to_string(orbit.join("board")).unwrap();
    assert_eq!(pointer.trim(), board.to_str().unwrap());

    let listed = repo.ok_json(&["--dir", workspace.to_str().unwrap(), "list", "--json"]);
    assert_eq!(listed["data"]["count"], 0);
}

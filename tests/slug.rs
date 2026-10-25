mod common;

use common::TestRepo;
use std::path::Path;

fn prefix_of(repo: &TestRepo, board: &Path) -> String {
    let value = repo.ok_json(&["--dir", board.to_str().unwrap(), "export", "--json"]);
    value["data"]["board"]["prefix"]
        .as_str()
        .unwrap()
        .to_string()
}

fn set_config_prefix(repo: &TestRepo, board: &Path, prefix: &str) {
    let config = board.join("config.toml");
    let content = std::fs::read_to_string(&config).unwrap();
    let content = content
        .lines()
        .map(|line| {
            if line.starts_with("prefix = ") {
                format!("prefix = \"{prefix}\"")
            } else {
                line.to_string()
            }
        })
        .collect::<Vec<_>>()
        .join("\n")
        + "\n";
    std::fs::write(&config, content).unwrap();
    let add = repo.git_in(board, &["add", "config.toml"]);
    assert!(add.status.success());
    let commit = repo.git_in(board, &["commit", "-m", "legacy prefix"]);
    assert!(commit.status.success());
}

fn commit_count(repo: &TestRepo, board: &Path) -> usize {
    let output = repo.git_in(board, &["rev-list", "--count", "HEAD"]);
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8_lossy(&output.stdout)
        .trim()
        .parse()
        .unwrap()
}

#[test]
fn init_auto_adopts_the_repository_folder_name() {
    let repo = TestRepo::new();
    let named = repo.named_repo("teramigo");

    let value = repo.ok_json_in(&named, &["init", "--agents", "none", "--json"]);

    assert_eq!(value["data"]["prefix"], "teramigo-");
    let board = named.join(".git/orbit/board");
    assert_eq!(prefix_of(&repo, &board), "teramigo-");

    let id = repo.ok_json_in(&named, &["add", "Task", "--json"])["data"]["id"]
        .as_str()
        .unwrap()
        .to_string();
    assert!(id.starts_with("teramigo-"), "{id}");
}

#[test]
fn init_slug_overrides_and_rejects_invalid_input() {
    let repo = TestRepo::new();
    let named = repo.named_repo("teramigo-api");
    let value = repo.ok_json_in(
        &named,
        &["init", "--agents", "none", "--slug", "teramigo", "--json"],
    );
    assert_eq!(value["data"]["prefix"], "teramigo-");

    let other = repo.named_repo("other");
    let (status, value) = repo.json_in(
        &other,
        &[
            "init",
            "--agents",
            "none",
            "--slug",
            "Teramigo_API",
            "--json",
        ],
    );
    assert_eq!(status, 1);
    assert_eq!(value["error"]["code"], "INVALID_SLUG");
    assert!(
        value["error"]["message"]
            .as_str()
            .unwrap()
            .contains("use `teramigo-api`")
    );
    assert!(!other.join(".git/orbit/board/config.toml").exists());
}

#[test]
fn init_board_defaults_to_orb_and_accepts_a_slug() {
    let repo = TestRepo::new();
    let parent = tempfile::tempdir().unwrap();
    let board = parent.path().join("board");
    let board_str = board.to_str().unwrap();

    let value = repo.ok_json(&["init", "--board", board_str, "--agents", "none", "--json"]);
    assert_eq!(value["data"]["prefix"], "orb-");
    assert_eq!(prefix_of(&repo, &board), "orb-");

    let id = repo.ok_json(&["--dir", board_str, "add", "Task", "--json"])["data"]["id"]
        .as_str()
        .unwrap()
        .to_string();
    assert!(id.starts_with("orb-"), "{id}");

    let other = parent.path().join("other-board");
    let value = repo.ok_json(&[
        "init",
        "--board",
        other.to_str().unwrap(),
        "--agents",
        "none",
        "--slug",
        "acme",
        "--json",
    ]);
    assert_eq!(value["data"]["prefix"], "acme-");
}

#[test]
fn workspace_defaults_to_its_folder_and_empty_boards_can_change_slug() {
    let repo = TestRepo::new();
    let parent = tempfile::tempdir().unwrap();

    let first = repo.ok_json_in(parent.path(), &["workspace", "Acme_API", "--json"]);
    assert_eq!(first["data"]["prefix"], "acme-api-");
    let board = parent.path().join("Acme_API/board");
    assert_eq!(prefix_of(&repo, &board), "acme-api-");

    let second = repo.ok_json_in(parent.path(), &["workspace", "Acme_API", "--json"]);
    assert_eq!(second["data"]["prefix"], "acme-api-");
    assert_eq!(second["data"]["created"], false);

    let changed = repo.ok_json_in(
        parent.path(),
        &["workspace", "Acme_API", "--slug", "acme", "--json"],
    );
    assert_eq!(changed["data"]["prefix"], "acme-");
    assert_eq!(prefix_of(&repo, &board), "acme-");

    let id =
        repo.ok_json(&["--dir", board.to_str().unwrap(), "add", "Task", "--json"])["data"]["id"]
            .as_str()
            .unwrap()
            .to_string();
    assert!(id.starts_with("acme-"), "{id}");

    let (status, value) = repo.json_in(
        parent.path(),
        &["workspace", "Acme_API", "--slug", "other", "--json"],
    );
    assert_eq!(status, 1);
    assert_eq!(value["error"]["code"], "SLUG_LOCKED");
}

#[test]
fn workspace_rejects_an_invalid_slug_with_a_suggestion() {
    let repo = TestRepo::new();
    let parent = tempfile::tempdir().unwrap();
    let (status, value) = repo.json_in(
        parent.path(),
        &["workspace", "bad", "--slug", "Bad Slug", "--json"],
    );

    assert_eq!(status, 1);
    assert_eq!(value["error"]["code"], "INVALID_SLUG");
    assert!(
        value["error"]["message"]
            .as_str()
            .unwrap()
            .contains("use `bad-slug`")
    );
}

#[test]
fn reruns_preserve_the_prefix_and_same_slug_does_not_commit() {
    let repo = TestRepo::new();
    let named = repo.named_repo("teramigo");
    let board = named.join(".git/orbit/board");

    let first = repo.ok_json_in(&named, &["init", "--agents", "none", "--json"]);
    assert_eq!(first["data"]["prefix"], "teramigo-");
    let baseline = commit_count(&repo, &board);

    let second = repo.ok_json_in(&named, &["init", "--agents", "none", "--json"]);
    assert_eq!(second["data"]["prefix"], "teramigo-");
    assert_eq!(second["data"]["created"], false);
    assert_eq!(commit_count(&repo, &board), baseline);

    let same = repo.ok_json_in(
        &named,
        &["init", "--agents", "none", "--slug", "teramigo", "--json"],
    );
    assert_eq!(same["data"]["prefix"], "teramigo-");
    assert_eq!(commit_count(&repo, &board), baseline);

    let changed = repo.ok_json_in(
        &named,
        &["init", "--agents", "none", "--slug", "other", "--json"],
    );
    assert_eq!(changed["data"]["prefix"], "other-");
    assert_eq!(prefix_of(&repo, &board), "other-");
    assert_eq!(commit_count(&repo, &board), baseline + 1);

    let log = repo.git_in(&board, &["log", "--oneline"]);
    assert!(String::from_utf8_lossy(&log.stdout).contains("orbit: set task prefix to other-"));
}

#[test]
fn a_legacy_no_dash_prefix_treats_the_same_slug_as_a_no_op() {
    let repo = TestRepo::new();
    repo.init();
    let board = repo.board_path();
    set_config_prefix(&repo, &board, "orb");

    let baseline = commit_count(&repo, &board);
    let value = repo.ok_json(&["init", "--agents", "none", "--slug", "orb", "--json"]);
    assert_eq!(value["data"]["prefix"], "orb");
    assert_eq!(commit_count(&repo, &board), baseline);
    assert_eq!(prefix_of(&repo, &board), "orb");

    let id = repo.add("Legacy task", &[]);
    assert!(id.starts_with("orb") && !id.starts_with("orb-"), "{id}");

    let after_add = commit_count(&repo, &board);
    let value = repo.ok_json(&["init", "--agents", "none", "--slug", "orb", "--json"]);
    assert_eq!(value["data"]["prefix"], "orb");
    assert_eq!(commit_count(&repo, &board), after_add);
}

#[test]
fn init_local_uses_the_main_checkout_slug_and_accepts_an_override() {
    let repo = TestRepo::new();
    let named = repo.named_repo("teramigo");
    let shared = tempfile::tempdir().unwrap();
    let shared_board = shared.path().join("board");

    repo.ok_json_in(
        &named,
        &[
            "init",
            "--board",
            shared_board.to_str().unwrap(),
            "--agents",
            "none",
            "--json",
        ],
    );

    let local = repo.ok_json_in(&named, &["init", "--local", "--agents", "none", "--json"]);
    assert_eq!(local["data"]["prefix"], "teramigo-");
    assert_eq!(local["data"]["external"], false);

    let changed = repo.ok_json_in(
        &named,
        &[
            "init", "--local", "--agents", "none", "--slug", "other", "--json",
        ],
    );
    assert_eq!(changed["data"]["prefix"], "other-");
}

#[test]
fn invalid_slug_is_rejected_even_when_it_matches_the_stored_word() {
    let repo = TestRepo::new();
    repo.init();

    let (status, value) = repo.json(&["init", "--agents", "none", "--slug", "orb-", "--json"]);

    assert_eq!(status, 1);
    assert_eq!(value["error"]["code"], "INVALID_SLUG");
    assert!(
        value["error"]["message"]
            .as_str()
            .unwrap()
            .contains("use `orb`")
    );
}

#[test]
fn populated_boards_refuse_a_slug_change() {
    let repo = TestRepo::new();
    let named = repo.named_repo("teramigo");
    repo.ok_json_in(&named, &["init", "--agents", "none", "--json"]);
    repo.ok_json_in(&named, &["add", "Existing task", "--json"]);

    let (status, value) = repo.json_in(
        &named,
        &["init", "--agents", "none", "--slug", "other", "--json"],
    );

    assert_eq!(status, 1);
    assert_eq!(value["error"]["code"], "SLUG_LOCKED");
    assert_eq!(value["error"]["details"]["current"], "teramigo-");
    assert_eq!(value["error"]["details"]["requested"], "other-");
    assert_eq!(value["error"]["details"]["tasks"], 1);
    let message = value["error"]["message"].as_str().unwrap();
    assert!(message.contains("1 task"), "{message}");
    assert!(message.contains("orbit slug migrate other"), "{message}");
}

#[test]
fn hand_editing_the_prefix_yields_one_prefix_mismatch() {
    let repo = TestRepo::new();
    repo.init();
    repo.add("One", &[]);
    repo.add("Two", &[]);

    let config = repo.board_path().join("config.toml");
    let content = std::fs::read_to_string(&config)
        .unwrap()
        .replace("prefix = \"orb-\"", "prefix = \"teramigo-\"");
    std::fs::write(&config, content).unwrap();

    let (status, value) = repo.json(&["list", "--json"]);
    assert_eq!(status, 1);
    assert_eq!(value["error"]["code"], "PREFIX_MISMATCH");
    assert_eq!(value["error"]["details"]["configured"], "teramigo-");
    assert_eq!(value["error"]["details"]["observed"], "orb-");
    assert_eq!(value["error"]["details"]["tasks"], 2);

    let (status, value) = repo.json(&["doctor", "--json"]);
    assert_eq!(status, 1);
    assert_eq!(value["data"]["healthy"], false);
    let issues = value["data"]["issues"].as_array().unwrap();
    assert_eq!(issues.len(), 1, "{value}");
    assert_eq!(issues[0]["code"], "PREFIX_MISMATCH");
    assert_eq!(value["data"]["tasks"], 2);
}

#[test]
fn a_legacy_no_dash_prefix_mismatch_keeps_per_file_corrupt_tasks() {
    let repo = TestRepo::new();
    repo.init();
    let board = repo.board_path();
    set_config_prefix(&repo, &board, "orb");

    repo.add("One", &[]);
    repo.add("Two", &[]);
    set_config_prefix(&repo, &board, "teramigo");

    let (status, value) = repo.json(&["list", "--json"]);
    assert_eq!(status, 1);
    assert_eq!(value["error"]["code"], "CORRUPT_TASK");

    let (status, value) = repo.json(&["doctor", "--json"]);
    assert_eq!(status, 1);
    let issues = value["data"]["issues"].as_array().unwrap();
    assert_eq!(issues.len(), 2, "{value}");
    assert!(issues.iter().all(|issue| issue["code"] == "CORRUPT_TASK"));
}

#[test]
fn export_carries_the_prefix_and_prime_does_not() {
    let repo = TestRepo::new();
    repo.init();

    let export = repo.ok_json(&["export", "--json"]);
    assert_eq!(export["data"]["schema_version"], 2);
    assert_eq!(export["data"]["board"]["prefix"], "orb-");

    let prime = repo.ok_json(&["prime", "--json"]);
    assert!(prime["data"].get("prefix").is_none());

    let named = repo.named_repo("teramigo");
    repo.ok_json_in(&named, &["init", "--agents", "none", "--json"]);
    let export = repo.ok_json_in(&named, &["export", "--json"]);
    assert_eq!(export["data"]["board"]["prefix"], "teramigo-");
}

#[test]
fn a_secondary_worktree_uses_the_main_checkout_slug() {
    let repo = TestRepo::new();
    let worktree_dir = tempfile::tempdir().unwrap();
    let worktree = worktree_dir.path().join("wt");
    repo.add_worktree(&worktree, "feature");

    let value = repo.ok_json_in(&worktree, &["init", "--agents", "none", "--json"]);

    let main_name = repo.path().file_name().unwrap().to_str().unwrap();
    let expected = format!("{}-", orbit_tracker::slug::from_folder_name(main_name));
    assert_eq!(value["data"]["prefix"], expected);
    assert!(repo.board_path().join("config.toml").exists());
    assert!(worktree.join(".git").is_file());
}

#[test]
fn workspace_warns_on_stderr_when_the_anchor_is_home() {
    let repo = TestRepo::new();
    let home = tempfile::tempdir().unwrap();
    let home_str = home.path().to_str().unwrap();

    let output = repo.orbit_with_env(
        &["workspace", home_str, "--json"],
        &[("HOME", home_str), ("USERPROFILE", home_str)],
    );

    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("anchor is the home directory"), "{stderr}");
    let value: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert!(
        value["meta"]["warnings"][0]
            .as_str()
            .unwrap()
            .contains("anchor is the home directory")
    );
}

#[test]
fn init_warns_on_stderr_when_the_main_checkout_is_home() {
    let repo = TestRepo::new();
    let home = repo.path().to_str().unwrap();

    let output = repo.orbit_with_env(
        &["init", "--agents", "none", "--json"],
        &[("HOME", home), ("USERPROFILE", home)],
    );

    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("anchor is the home directory"), "{stderr}");
    let value: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert!(
        value["meta"]["warnings"][0]
            .as_str()
            .unwrap()
            .contains("anchor is the home directory")
    );
}

#[test]
fn init_print_computes_the_prefix_without_writing() {
    let repo = TestRepo::new();
    let named = repo.named_repo("teramigo");

    let value = repo.ok_json_in(&named, &["init", "--agents", "none", "--print", "--json"]);

    assert_eq!(value["data"]["prefix"], "teramigo-");
    assert_eq!(value["data"]["created"], true);
    assert!(!named.join(".git/orbit/board/config.toml").exists());

    repo.ok_json_in(&named, &["init", "--agents", "none", "--json"]);
    let board = named.join(".git/orbit/board");
    let value = repo.ok_json_in(
        &named,
        &[
            "init", "--agents", "none", "--slug", "acme", "--print", "--json",
        ],
    );
    assert_eq!(value["data"]["prefix"], "acme-");
    assert_eq!(prefix_of(&repo, &board), "teramigo-");
}

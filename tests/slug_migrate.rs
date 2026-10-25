mod common;

use common::TestRepo;
use std::path::Path;

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

fn git_stdout(repo: &TestRepo, board: &Path, args: &[&str]) -> String {
    let output = repo.git_in(board, args);
    assert!(
        output.status.success(),
        "git {args:?}: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8_lossy(&output.stdout).to_string()
}

fn mapped_to(value: &serde_json::Value, from: &str) -> String {
    value["data"]["mappings"]
        .as_array()
        .unwrap()
        .iter()
        .find(|mapping| mapping["from"] == from)
        .unwrap_or_else(|| panic!("no mapping for {from}: {value}"))["to"]
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

#[test]
fn slug_show_reports_identity_and_counts() {
    let repo = TestRepo::new();
    repo.init();
    let parent = repo.add("Parent", &[]);
    repo.add("Child", &["--parent", &parent]);
    repo.ok_json(&[
        "comment", "add", &parent, "--by", "alice", "--body", "A note.", "--json",
    ]);

    let value = repo.ok_json(&["slug", "show", "--json"]);
    assert_eq!(value["data"]["slug"], "orb");
    assert_eq!(value["data"]["prefix"], "orb-");
    assert_eq!(value["data"]["tasks"], 2);
    assert_eq!(value["data"]["subtasks"], 1);
    assert_eq!(value["data"]["comment_files"], 1);

    let output = repo.orbit(&["slug", "show"]);
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let human = String::from_utf8_lossy(&output.stdout);
    assert!(human.contains("slug     orb"), "{human}");
    assert!(human.contains("prefix   orb-"), "{human}");
    assert!(human.contains("tasks    2 (1 SubTask)"), "{human}");
    assert!(human.contains("comments 1 file"), "{human}");
}

#[test]
fn dry_run_prints_every_count_and_writes_nothing() {
    let repo = TestRepo::new();
    repo.init();
    let board = repo.board_path();
    let parent = repo.add(
        "Parent",
        &[
            "--body",
            "Context mentioning the parent ID stays untouched.",
        ],
    );
    let child = repo.add("Child", &["--parent", &parent]);
    let dependent = repo.add("Dependent", &["--depends-on", &parent]);
    repo.ok_json(&[
        "comment", "add", &parent, "--by", "alice", "--body", "A note.", "--json",
    ]);
    let baseline = commit_count(&repo, &board);
    let before = repo.read_task(&parent);

    let value = repo.ok_json(&["slug", "migrate", "atlas", "--dry-run", "--json"]);

    assert_eq!(value["data"]["current"]["slug"], "orb");
    assert_eq!(value["data"]["current"]["prefix"], "orb-");
    assert_eq!(value["data"]["target"]["slug"], "atlas");
    assert_eq!(value["data"]["target"]["prefix"], "atlas-");
    assert_eq!(value["data"]["changed"], true);
    assert_eq!(value["data"]["tasks"], 3);
    assert_eq!(value["data"]["subtasks"], 1);
    assert_eq!(value["data"]["comment_files"], 1);
    assert_eq!(value["data"]["references"], 2);
    assert_eq!(value["data"]["mappings"].as_array().unwrap().len(), 3);
    assert_eq!(value["data"]["applied"], false);
    assert_eq!(value["data"]["dry_run"], true);
    assert!(value["data"]["commit"].is_null());

    assert_eq!(commit_count(&repo, &board), baseline);
    assert_eq!(repo.read_task(&parent), before);
    assert!(repo.task_path(&child).exists());
    assert!(repo.task_path(&dependent).exists());
    assert!(
        std::fs::read_to_string(board.join("config.toml"))
            .unwrap()
            .contains("prefix = \"orb-\"")
    );
}

#[test]
fn migrate_rewrites_ids_references_comments_and_commits_once() {
    let repo = TestRepo::new();
    repo.init();
    let board = repo.board_path();
    let parent = repo.add("Parent", &["--body", "The body keeps the old ID in prose."]);
    let child = repo.add("Child", &["--parent", &parent]);
    let dependent = repo.add("Dependent", &["--depends-on", &parent]);
    repo.ok_json(&[
        "comment",
        "add",
        &parent,
        "--by",
        "alice",
        "--body",
        &format!("Comment prose keeps {parent} too."),
        "--json",
    ]);
    let baseline = commit_count(&repo, &board);

    let value = repo.ok_json(&["slug", "migrate", "atlas", "--json"]);
    let new_parent = mapped_to(&value, &parent);
    let new_child = mapped_to(&value, &child);
    let new_dependent = mapped_to(&value, &dependent);

    assert_eq!(value["data"]["applied"], true);
    assert_eq!(value["data"]["changed"], true);
    assert_eq!(value["data"]["commit"].as_str().unwrap().len(), 40);
    assert!(new_parent.starts_with("atlas-"), "{new_parent}");
    assert!(new_child.starts_with(&format!("{new_parent}.")));

    assert!(!repo.task_path(&parent).exists());
    assert!(!repo.task_path(&child).exists());
    assert!(!repo.task_path(&dependent).exists());
    assert!(repo.task_path(&new_parent).exists());
    assert!(repo.task_path(&new_child).exists());
    assert!(repo.task_path(&new_dependent).exists());

    let parent_body = repo.read_task(&new_parent);
    assert!(parent_body.contains("The body keeps the old ID in prose."));
    let child_body = repo.read_task(&new_child);
    assert!(child_body.contains(&format!("parent: {new_parent}")));
    let dependent_body = repo.read_task(&new_dependent);
    assert!(dependent_body.contains(&format!("depends_on:\n- {new_parent}")));

    assert!(!board.join(format!("comments/{parent}.md")).exists());
    let comments_path = board.join(format!("comments/{new_parent}.md"));
    assert!(comments_path.exists());
    let comments = std::fs::read_to_string(&comments_path).unwrap();
    assert!(comments.starts_with(&format!("# Comments: {new_parent}\n")));
    assert!(comments.contains(&format!("Comment prose keeps {parent} too.")));

    let export = repo.ok_json(&["export", "--json"]);
    assert_eq!(export["data"]["board"]["prefix"], "atlas-");
    let tasks = export["data"]["tasks"].as_array().unwrap();
    let parent_task = tasks.iter().find(|task| task["id"] == new_parent).unwrap();
    assert_eq!(parent_task["subtask_count"], 1);
    let child_task = tasks.iter().find(|task| task["id"] == new_child).unwrap();
    assert_eq!(child_task["parent"], new_parent);
    let dependent_task = tasks
        .iter()
        .find(|task| task["id"] == new_dependent)
        .unwrap();
    assert_eq!(dependent_task["depends_on"][0], new_parent);

    assert_eq!(commit_count(&repo, &board), baseline + 1);
    let log = git_stdout(&repo, &board, &["log", "--oneline", "-2"]);
    assert!(
        log.contains("orbit: migrate task prefix from orb- to atlas-"),
        "{log}"
    );
    let worktrees = git_stdout(&repo, &board, &["worktree", "list"]);
    assert!(!worktrees.contains("orbit-slug-migrate"), "{worktrees}");
    assert_eq!(
        git_stdout(&repo, &board, &["status", "--porcelain"]).trim(),
        ""
    );

    let doctor = repo.ok_json(&["doctor", "--json"]);
    assert_eq!(doctor["data"]["healthy"], true, "{doctor}");
    let next = repo.add("Next", &[]);
    assert!(next.starts_with("atlas-"), "{next}");
}

#[test]
fn live_claims_require_force_and_force_lists_the_holders() {
    let repo = TestRepo::new();
    repo.init();
    let board = repo.board_path();
    let id = repo.add("Claimed", &[]);
    repo.ok_json(&["claim", &id, "--by", "alice", "--json"]);
    let claimed = commit_count(&repo, &board);

    let (status, value) = repo.err_json(&["slug", "migrate", "atlas", "--json"]);
    assert_eq!(status, 1);
    assert_eq!(value["error"]["code"], "MIGRATION_BLOCKED");
    assert_eq!(value["error"]["details"]["reason"], "live_claims");
    assert_eq!(value["error"]["details"]["claims"][0]["holder"], "alice");
    assert_eq!(value["error"]["details"]["claims"][0]["from"], id);
    assert!(
        value["error"]["details"]["claims"][0]["to"]
            .as_str()
            .unwrap()
            .starts_with("atlas-")
    );
    assert!(repo.task_path(&id).exists());

    let value = repo.ok_json(&["slug", "migrate", "atlas", "--force", "--json"]);
    let new_id = mapped_to(&value, &id);
    assert_eq!(value["data"]["claims"][0]["holder"], "alice");
    assert_eq!(value["data"]["claims"][0]["from"], id);
    assert_eq!(value["data"]["claims"][0]["to"], new_id);
    assert!(
        value["meta"]["warnings"][0]
            .as_str()
            .unwrap()
            .contains("holders must re-read"),
        "{value}"
    );

    let export = repo.ok_json(&["export", "--json"]);
    let task = export["data"]["tasks"]
        .as_array()
        .unwrap()
        .iter()
        .find(|task| task["id"] == new_id)
        .unwrap();
    assert_eq!(task["claimed_by"], "alice");
    assert_eq!(task["status"], "in_progress");
    assert_eq!(commit_count(&repo, &board), claimed + 1);
}

#[test]
fn migrate_refuses_an_unhealthy_board_without_changing_it() {
    let repo = TestRepo::new();
    repo.init();
    let board = repo.board_path();
    let id = repo.add("One", &[]);
    std::fs::write(
        board.join("comments/orb-missing.md"),
        "# Comments: orb-missing\n\nbody\n",
    )
    .unwrap();

    let (status, value) = repo.err_json(&["slug", "migrate", "atlas", "--json"]);
    assert_eq!(status, 1);
    assert_eq!(value["error"]["code"], "MIGRATION_BLOCKED");
    assert_eq!(value["error"]["details"]["reason"], "unhealthy");
    assert_eq!(
        value["error"]["details"]["issues"][0]["code"],
        "ORPHAN_COMMENT"
    );
    assert!(repo.task_path(&id).exists());
    assert!(
        std::fs::read_to_string(board.join("config.toml"))
            .unwrap()
            .contains("prefix = \"orb-\"")
    );
    assert!(board.join("comments/orb-missing.md").exists());

    let show = repo.ok_json(&["slug", "show", "--json"]);
    assert_eq!(show["data"]["prefix"], "orb-");
    assert_eq!(show["data"]["comment_files"], 1);
}

#[test]
fn migrate_refuses_a_parent_id_mismatch() {
    let repo = TestRepo::new();
    repo.init();
    let parent = repo.add("Parent", &[]);
    let other = repo.add("Other parent", &[]);
    let child = repo.add("Child", &["--parent", &parent]);
    let content = repo
        .read_task(&child)
        .replace(&format!("parent: {parent}"), &format!("parent: {other}"));
    repo.write_task(&child, &content);

    let (status, value) = repo.err_json(&["slug", "migrate", "atlas", "--json"]);

    assert_eq!(status, 1);
    assert_eq!(value["error"]["code"], "MIGRATION_BLOCKED");
    assert_eq!(value["error"]["details"]["reason"], "unhealthy");
    assert_eq!(
        value["error"]["details"]["issues"][0]["code"],
        "PARENT_MISMATCH"
    );
    assert!(repo.task_path(&child).exists());
}

#[test]
fn invalid_migration_slug_uses_the_shared_validator() {
    let repo = TestRepo::new();
    repo.init();
    let id = repo.add("One", &[]);

    let (status, value) = repo.err_json(&["slug", "migrate", "Atlas_", "--json"]);

    assert_eq!(status, 1);
    assert_eq!(value["error"]["code"], "INVALID_SLUG");
    assert!(
        value["error"]["message"]
            .as_str()
            .unwrap()
            .contains("use `atlas`"),
        "{value}"
    );
    assert!(repo.task_path(&id).exists());
}

#[test]
fn migrating_to_the_current_slug_is_a_no_op() {
    let repo = TestRepo::new();
    repo.init();
    let board = repo.board_path();
    repo.add("One", &[]);
    let baseline = commit_count(&repo, &board);

    let value = repo.ok_json(&["slug", "migrate", "orb", "--json"]);

    assert_eq!(value["data"]["changed"], false);
    assert_eq!(value["data"]["applied"], false);
    assert!(value["data"]["commit"].is_null());
    assert_eq!(commit_count(&repo, &board), baseline);
}

#[test]
fn migrate_works_on_an_empty_board() {
    let repo = TestRepo::new();
    repo.init();
    let board = repo.board_path();
    let baseline = commit_count(&repo, &board);

    let value = repo.ok_json(&["slug", "migrate", "acme", "--json"]);

    assert_eq!(value["data"]["applied"], true);
    assert_eq!(value["data"]["tasks"], 0);
    assert_eq!(value["data"]["mappings"].as_array().unwrap().len(), 0);
    assert_eq!(commit_count(&repo, &board), baseline + 1);
    assert!(
        std::fs::read_to_string(board.join("config.toml"))
            .unwrap()
            .contains("prefix = \"acme-\"")
    );
    let id = repo.add("First", &[]);
    assert!(id.starts_with("acme-"), "{id}");
}

#[test]
fn migrate_rewrites_a_legacy_prefix_without_a_dash() {
    let repo = TestRepo::new();
    repo.init();
    let board = repo.board_path();
    set_config_prefix(&repo, &board, "orb");
    let legacy = repo.add("Legacy", &[]);
    assert!(legacy.starts_with("orb") && !legacy.starts_with("orb-"));

    let value = repo.ok_json(&["slug", "migrate", "acme", "--json"]);
    let new_id = mapped_to(&value, &legacy);

    assert_eq!(new_id, format!("acme-{}", &legacy["orb".len()..]));
    assert!(repo.task_path(&new_id).exists());
    assert!(!repo.task_path(&legacy).exists());
    let export = repo.ok_json(&["export", "--json"]);
    assert_eq!(export["data"]["board"]["prefix"], "acme-");
}

#[test]
fn migrate_flushes_pending_heartbeat_writes_before_the_single_commit() {
    let repo = TestRepo::new();
    repo.init();
    let board = repo.board_path();
    let id = repo.add("Claimed", &[]);
    repo.ok_json(&["claim", &id, "--by", "alice", "--json"]);
    let claimed = commit_count(&repo, &board);
    repo.ok_json(&["heartbeat", &id, "--by", "alice", "--json"]);
    assert!(
        !git_stdout(&repo, &board, &["status", "--porcelain"])
            .trim()
            .is_empty()
    );

    let dry_run = repo.ok_json(&["slug", "migrate", "atlas", "--dry-run", "--json"]);
    assert_eq!(dry_run["data"]["claims"][0]["holder"], "alice");
    assert_eq!(commit_count(&repo, &board), claimed);
    assert!(
        !git_stdout(&repo, &board, &["status", "--porcelain"])
            .trim()
            .is_empty()
    );

    let value = repo.ok_json(&["slug", "migrate", "atlas", "--force", "--json"]);
    let new_id = mapped_to(&value, &id);
    assert_eq!(commit_count(&repo, &board), claimed + 2);
    let log = git_stdout(&repo, &board, &["log", "--oneline", "-3"]);
    assert!(log.contains("orbit: sync"), "{log}");
    assert!(
        log.contains("orbit: migrate task prefix from orb- to atlas-"),
        "{log}"
    );
    assert_eq!(
        git_stdout(&repo, &board, &["status", "--porcelain"]).trim(),
        ""
    );
    let export = repo.ok_json(&["export", "--json"]);
    let task = export["data"]["tasks"]
        .as_array()
        .unwrap()
        .iter()
        .find(|task| task["id"] == new_id)
        .unwrap();
    assert_eq!(task["claimed_by"], "alice");
}

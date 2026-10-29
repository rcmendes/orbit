mod common;

use std::path::Path;

use common::TestRepo;
use serde_json::json;

const TASK: &str = "---\ntitle: Preserved work\nstatus: todo\ntype: task\npriority: 1\nlabels:\n- ready-for-agent\ndepends_on: []\nclaimed_by: migration-agent\nclaimed_at: 2026-10-28T12:00:00Z\nlease_until: 2026-10-28T12:30:00Z\ncheckpoint:\n  by: migration-agent\n  at: 2026-10-28T12:01:00Z\n  progress: Existing work checkpoint\n  next: Continue the task\ncreated_at: 2026-10-28T12:00:00Z\nupdated_at: 2026-10-28T12:00:00Z\n---\n\nHistory stays with the task.\n";

fn write_board(root: &Path, task_id: &str, task: &str) {
    std::fs::create_dir_all(root.join("tasks")).unwrap();
    std::fs::create_dir_all(root.join("comments")).unwrap();
    std::fs::write(
        root.join("config.toml"),
        "[board]\nbranch = \"orbit\"\nstorage = \"repository\"\nlease_minutes = 30\ndefault_priority = 2\nnon_ready_labels = [\"needs-triage\", \"needs-info\", \"ready-for-human\"]\nauto_commit = true\nlock_timeout_ms = 2000\n\n[id]\nlength = 6\nprefix = \"teramigo-\"\n",
    )
    .unwrap();
    std::fs::write(root.join("tasks").join(format!("{task_id}.md")), task).unwrap();
}

fn write_task(root: &Path, task_id: &str, task: &str) {
    std::fs::create_dir_all(root.join("tasks")).unwrap();
    std::fs::write(root.join("tasks").join(format!("{task_id}.md")), task).unwrap();
}

fn task_with(id_title: &str, extra: &str) -> String {
    let dependencies = if extra.starts_with("depends_on:") {
        ""
    } else {
        "depends_on: []\n"
    };
    format!(
        "---\ntitle: {id_title}\nstatus: todo\ntype: task\npriority: 1\nlabels: []\n{dependencies}{extra}created_at: 2026-10-28T12:00:00Z\nupdated_at: 2026-10-28T12:00:00Z\n---\n\nTask body.\n"
    )
}

fn workspace(repo: &TestRepo) -> tempfile::TempDir {
    workspace_with_slug(repo, "teramigo")
}

fn workspace_with_slug(repo: &TestRepo, slug: &str) -> tempfile::TempDir {
    let temp = tempfile::tempdir().unwrap();
    let workspace = temp.path().join("Teramigo");
    std::fs::create_dir(&workspace).unwrap();
    let slug_arg = format!("--slug={slug}");
    repo.ok_json_in(
        &workspace,
        &["init", "--agents", "none", &slug_arg, "--json"],
    );
    temp
}

fn empty_workspace() -> tempfile::TempDir {
    let temp = tempfile::tempdir().unwrap();
    std::fs::create_dir(temp.path().join("Acme")).unwrap();
    temp
}

#[test]
fn migration_preview_is_read_only_and_apply_preserves_the_source_and_task_data() {
    let repo = TestRepo::new();
    let temp = workspace(&repo);
    let workspace = temp.path().join("Teramigo");
    let source = workspace.join(".board");
    write_board(&source, "teramigo-abcdef", TASK);
    let source_config = std::fs::read(source.join("config.toml")).unwrap();
    let source_task = std::fs::read(source.join("tasks/teramigo-abcdef.md")).unwrap();
    let comments = "# Comments: teramigo-abcdef\n\n<!-- orbit:comment {\"author\":\"agent\",\"created_at\":\"2026-10-28T12:01:00Z\"} -->\nA historical comment.\n<!-- /orbit:comment -->\n";
    std::fs::write(source.join("comments/teramigo-abcdef.md"), comments).unwrap();

    let dry_run = repo.ok_json_in(&workspace, &["workspace", "migrate", "--dry-run", "--json"]);
    assert_eq!(dry_run["data"]["tasks"], 1);
    assert_eq!(dry_run["data"]["applied"], false);
    assert_eq!(dry_run["data"]["source_boards"], 1);
    assert!(
        !workspace
            .join(".orbit/workspace-migration-state.json")
            .exists()
    );
    assert!(
        !workspace
            .join(".orbit/board/tasks/teramigo-abcdef.md")
            .exists()
    );

    let applied = repo.ok_json_in(&workspace, &["workspace", "migrate", "--json"]);
    assert_eq!(applied["data"]["applied"], true);
    assert_eq!(applied["data"]["commit_created"], true);
    assert!(applied["data"]["commit"].as_str().is_some());
    assert!(applied["data"]["rollback_guidance"].as_str().is_some());
    assert!(applied["data"]["doctor_check"].as_str().is_some());
    assert_eq!(
        std::fs::read(workspace.join(".orbit/board/tasks/teramigo-abcdef.md")).unwrap(),
        source_task
    );
    assert!(
        std::fs::read_to_string(workspace.join(".orbit/board/comments/teramigo-abcdef.md"))
            .unwrap()
            .contains("A historical comment.")
    );
    assert_eq!(
        std::fs::read(source.join("config.toml")).unwrap(),
        source_config
    );
    assert_eq!(
        std::fs::read(source.join("tasks/teramigo-abcdef.md")).unwrap(),
        source_task
    );
    assert_eq!(
        std::fs::read(source.join("comments/teramigo-abcdef.md")).unwrap(),
        comments.as_bytes()
    );
    let doctor = repo.ok_json_in(&workspace, &["doctor", "--json"]);
    assert_eq!(doctor["ok"], true);
}

#[test]
fn migration_can_build_the_workspace_board_without_shadowing_child_boards_first() {
    let repo = TestRepo::new();
    let temp = empty_workspace();
    let workspace = temp.path().join("Acme");
    let workspace_source = workspace.join(".board");
    write_board(&workspace_source, "teramigo-abcdef", TASK);
    let child_repo = workspace.join("api");
    std::fs::create_dir_all(&child_repo).unwrap();
    assert!(
        repo.git_in(&workspace, &["init", "-q", child_repo.to_str().unwrap()])
            .status
            .success()
    );
    let child_board = child_repo.join(".orbit");
    write_board(
        &child_board,
        "teramigo-ghjkmp",
        &task_with("Child repo task", ""),
    );
    let source_task = std::fs::read(workspace_source.join("tasks/teramigo-abcdef.md")).unwrap();
    let child_task = std::fs::read(child_board.join("tasks/teramigo-ghjkmp.md")).unwrap();

    let preview = repo.ok_json_in(&workspace, &["workspace", "migrate", "--dry-run", "--json"]);
    assert_eq!(preview["data"]["source_boards"], 2);
    assert_eq!(preview["data"]["will_create_destination"], true);
    assert!(!workspace.join(".orbit/board").exists());

    let applied = repo.ok_json_in(&workspace, &["workspace", "migrate", "--json"]);
    assert_eq!(applied["data"]["applied"], true);
    assert_eq!(applied["data"]["destination_created"], true);
    assert!(workspace.join(".orbit/board/config.toml").is_file());
    assert_eq!(
        std::fs::read(workspace_source.join("tasks/teramigo-abcdef.md")).unwrap(),
        source_task
    );
    assert_eq!(
        std::fs::read(child_board.join("tasks/teramigo-ghjkmp.md")).unwrap(),
        child_task
    );
    let tasks = repo.ok_json_in(&child_repo, &["list", "--all", "--json"]);
    let ids: Vec<&str> = tasks["data"]["tasks"]
        .as_array()
        .unwrap()
        .iter()
        .map(|task| task["id"].as_str().unwrap())
        .collect();
    assert!(ids.contains(&"teramigo-abcdef"));
    assert!(ids.contains(&"teramigo-ghjkmp"));
    let initialized = repo.ok_json_in(&workspace, &["init", "--agents", "none", "--json"]);
    assert_eq!(initialized["ok"], true);
}

#[test]
fn migration_remaps_collisions_and_rewrites_parent_dependency_and_comment_references() {
    let repo = TestRepo::new();
    let temp = workspace(&repo);
    let workspace = temp.path().join("Teramigo");
    let destination = workspace.join(".orbit/board");
    let source = workspace.join(".board");
    write_board(&source, "teramigo-abcdef", TASK);
    write_task(
        &source,
        "teramigo-abcdef.ghjkmp",
        &task_with("Child", "parent: teramigo-abcdef\n"),
    );
    write_task(
        &source,
        "teramigo-012345",
        &task_with("Dependent", "depends_on:\n- teramigo-abcdef\n"),
    );
    std::fs::create_dir_all(source.join("comments")).unwrap();
    std::fs::write(
        source.join("comments/teramigo-abcdef.md"),
        "# Comments: teramigo-abcdef\n\n<!-- orbit:comment {\"author\":\"agent\",\"created_at\":\"2026-10-28T12:01:00Z\"} -->\nParent history.\n<!-- /orbit:comment -->\n",
    )
    .unwrap();
    let source_comment = std::fs::read(source.join("comments/teramigo-abcdef.md")).unwrap();
    let source_root_task = std::fs::read(source.join("tasks/teramigo-abcdef.md")).unwrap();
    let source_child_task = std::fs::read(source.join("tasks/teramigo-abcdef.ghjkmp.md")).unwrap();
    let child_repo = workspace.join("api");
    std::fs::create_dir_all(&child_repo).unwrap();
    assert!(
        repo.git_in(&workspace, &["init", "-q", child_repo.to_str().unwrap()])
            .status
            .success()
    );
    let child_board = child_repo.join(".orbit");
    let child_task = task_with("Child repository work", "");
    write_board(&child_board, "teramigo-abcdef", &child_task);
    let child_source_bytes = std::fs::read(child_board.join("tasks/teramigo-abcdef.md")).unwrap();
    write_task(&destination, "teramigo-abcdef", TASK);
    assert!(
        repo.git_in(&destination, &["add", "tasks/teramigo-abcdef.md"])
            .status
            .success()
    );
    assert!(
        repo.git_in(
            &destination,
            &["commit", "-q", "-m", "pre-existing Workspace task"]
        )
        .status
        .success()
    );

    let applied = repo.ok_json_in(&workspace, &["workspace", "migrate", "--json"]);
    let mapping = applied["data"]["id_mappings"]
        .as_array()
        .unwrap()
        .iter()
        .find(|mapping| mapping["from"] == "teramigo-abcdef")
        .unwrap();
    let remapped_parent = mapping["to"].as_str().unwrap();
    let child_mapping = applied["data"]["id_mappings"]
        .as_array()
        .unwrap()
        .iter()
        .find(|mapping| {
            mapping["source"]
                == dunce::canonicalize(&child_board)
                    .unwrap()
                    .to_string_lossy()
                    .as_ref()
                && mapping["from"] == "teramigo-abcdef"
        })
        .unwrap();
    let remapped_child_repo_task = child_mapping["to"].as_str().unwrap();
    let remapped_child = format!("{remapped_parent}.ghjkmp");
    let child = std::fs::read_to_string(
        destination
            .join("tasks")
            .join(format!("{remapped_child}.md")),
    )
    .unwrap();
    assert!(child.contains(&format!("parent: {remapped_parent}")));
    let migrated_parent = std::fs::read_to_string(
        destination
            .join("tasks")
            .join(format!("{remapped_parent}.md")),
    )
    .unwrap();
    assert!(migrated_parent.contains("claimed_by: migration-agent"));
    assert!(migrated_parent.contains("progress: Existing work checkpoint"));
    let dependent = std::fs::read_to_string(destination.join("tasks/teramigo-012345.md")).unwrap();
    assert!(dependent.contains(&format!("- {remapped_parent}")));
    assert!(
        destination
            .join("comments")
            .join(format!("{remapped_parent}.md"))
            .is_file()
    );
    assert!(
        std::fs::read_to_string(
            destination
                .join("comments")
                .join(format!("{remapped_parent}.md"))
        )
        .unwrap()
        .contains("Parent history.")
    );
    assert_eq!(
        std::fs::read(source.join("tasks/teramigo-abcdef.md")).unwrap(),
        source_root_task
    );
    assert_eq!(
        std::fs::read(source.join("tasks/teramigo-abcdef.ghjkmp.md")).unwrap(),
        source_child_task
    );
    assert_eq!(
        std::fs::read(source.join("comments/teramigo-abcdef.md")).unwrap(),
        source_comment
    );
    assert_eq!(
        std::fs::read(child_board.join("tasks/teramigo-abcdef.md")).unwrap(),
        child_source_bytes
    );
    assert!(
        destination
            .join("tasks")
            .join(format!("{remapped_child_repo_task}.md"))
            .is_file()
    );
    let from_child = repo.ok_json_in(&child_repo, &["list", "--all", "--json"]);
    let listed_ids: Vec<&str> = from_child["data"]["tasks"]
        .as_array()
        .unwrap()
        .iter()
        .map(|task| task["id"].as_str().unwrap())
        .collect();
    assert!(listed_ids.contains(&remapped_parent));
    assert!(listed_ids.contains(&remapped_child_repo_task));
}

#[test]
fn duplicate_github_issue_links_stop_before_any_source_task_is_written() {
    let repo = TestRepo::new();
    let temp = workspace(&repo);
    let workspace = temp.path().join("Teramigo");
    let source = workspace.join(".board");
    let linked = task_with("Linked issue", "github:\n  repo: acme/api\n  number: 17\n");
    write_board(&source, "teramigo-abcdef", &linked);
    let child_repo = workspace.join("services/api");
    std::fs::create_dir_all(&child_repo).unwrap();
    assert!(
        repo.git_in(&workspace, &["init", "-q", child_repo.to_str().unwrap()])
            .status
            .success()
    );
    let duplicate = child_repo.join(".orbit");
    write_board(&duplicate, "teramigo-ghjkmp", &linked);
    let status = repo.json_in(&workspace, &["workspace", "migrate", "--json"]);
    assert_ne!(status.0, 0);
    assert!(
        status.1["error"]["message"]
            .as_str()
            .unwrap()
            .contains("duplicate GitHub Issue identity")
    );
    assert!(
        !workspace
            .join(".orbit/board/tasks/teramigo-abcdef.md")
            .exists()
    );
}

#[test]
fn workspace_configuration_wins_and_every_difference_is_reported() {
    let repo = TestRepo::new();
    let temp = workspace(&repo);
    let workspace = temp.path().join("Teramigo");
    let source = workspace.join(".board");
    write_board(&source, "teramigo-abcdef", TASK);
    let config_path = source.join("config.toml");
    let source_config = std::fs::read_to_string(&config_path)
        .unwrap()
        .replace("default_priority = 2", "default_priority = 0");
    std::fs::write(config_path, source_config).unwrap();
    let workspace_config_path = workspace.join(".orbit/board/config.toml");
    let workspace_config = std::fs::read(&workspace_config_path).unwrap();

    let applied = repo.ok_json_in(&workspace, &["workspace", "migrate", "--json"]);
    assert!(
        applied["data"]["configuration_conflicts"]
            .as_array()
            .unwrap()
            .iter()
            .any(|conflict| conflict["setting"] == "board.default_priority")
    );
    assert_eq!(
        std::fs::read(workspace_config_path).unwrap(),
        workspace_config
    );
}

#[test]
fn staging_a_workspace_board_inherits_the_legacy_configuration() {
    let repo = TestRepo::new();
    let temp = empty_workspace();
    let workspace = temp.path().join("Acme");
    let source = workspace.join(".board");
    write_board(&source, "teramigo-abcdef", TASK);
    let mut config = std::fs::read_to_string(source.join("config.toml"))
        .unwrap()
        .replace("default_priority = 2", "default_priority = 0");
    config.push_str(
        "\n[github]\norganization = \"acme\"\nrepositories = [\"acme/api\", \"acme/web\"]\n\n[github.issue_fields]\narea_name = \"Area\"\nverification_name = \"Verification\"\narea_id = \"AREA123\"\nverification_id = \"VERIFY123\"\n",
    );
    std::fs::write(source.join("config.toml"), &config).unwrap();

    let preview = repo.ok_json_in(&workspace, &["workspace", "migrate", "--dry-run", "--json"]);
    assert_eq!(
        preview["data"]["configuration_source"],
        json!(dunce::canonicalize(&source).unwrap())
    );
    assert!(!workspace.join(".orbit/board").exists());

    let applied = repo.ok_json_in(&workspace, &["workspace", "migrate", "--json"]);
    assert_eq!(applied["data"]["destination_created"], true);
    let written = std::fs::read_to_string(workspace.join(".orbit/board/config.toml")).unwrap();
    assert!(written.contains("default_priority = 0"), "{written}");
    assert!(written.contains("storage = \"repository\""), "{written}");
    assert!(written.contains("organization = \"acme\""), "{written}");
    assert!(written.contains("acme/api"), "{written}");
    assert!(written.contains("area_id = \"AREA123\""), "{written}");
    assert!(written.contains("prefix = \"acme-\""), "{written}");
    assert_eq!(
        std::fs::read_to_string(source.join("config.toml")).unwrap(),
        config
    );
}

#[test]
fn migration_preserves_ids_with_a_different_source_prefix() {
    let repo = TestRepo::new();
    let temp = workspace_with_slug(&repo, "acme");
    let workspace = temp.path().join("Teramigo");
    let source = workspace.join(".board");
    write_board(&source, "teramigo-abcdef", TASK);
    write_task(
        &source,
        "teramigo-abcdef.ghjkmp",
        &task_with("Legacy subtask", "parent: teramigo-abcdef\n"),
    );

    let planned = repo.ok_json_in(&workspace, &["workspace", "migrate", "--dry-run", "--json"]);
    assert_eq!(planned["data"]["id_mappings"].as_array().unwrap().len(), 0);
    assert_eq!(planned["data"]["accepted_prefixes_added"][0], "teramigo-");
    let applied = repo.ok_json_in(&workspace, &["workspace", "migrate", "--json"]);
    assert_eq!(applied["data"]["id_mappings"].as_array().unwrap().len(), 0);
    assert!(
        workspace
            .join(".orbit/board/tasks/teramigo-abcdef.md")
            .is_file()
    );
    assert!(
        workspace
            .join(".orbit/board/tasks/teramigo-abcdef.ghjkmp.md")
            .is_file()
    );
    let config = std::fs::read_to_string(workspace.join(".orbit/board/config.toml")).unwrap();
    assert!(config.contains("prefix = \"acme-\""));
    assert!(config.contains("accepted_prefixes = [\"teramigo-\"]"));
    assert_eq!(
        repo.ok_json_in(&workspace, &["doctor", "--json"])["ok"],
        true
    );
    let slug_migration = repo.ok_json_in(
        &workspace,
        &["slug", "migrate", "atlas", "--force", "--json"],
    );
    assert_eq!(slug_migration["data"]["applied"], true);
    assert!(
        workspace
            .join(".orbit/board/tasks/atlas-abcdef.md")
            .is_file()
    );
    assert!(
        workspace
            .join(".orbit/board/tasks/atlas-abcdef.ghjkmp.md")
            .is_file()
    );
}

#[test]
fn migration_resumes_after_some_planned_files_have_been_written() {
    let repo = TestRepo::new();
    let temp = workspace(&repo);
    let workspace = temp.path().join("Teramigo");
    let source = workspace.join(".board");
    write_board(&source, "teramigo-abcdef", TASK);
    let state = workspace.join(".orbit/workspace-migration-state.json");
    let config_before = std::fs::read(workspace.join(".orbit/board/config.toml")).unwrap();
    let partial = workspace.join(".orbit/board/tasks/teramigo-abcdef.md");
    std::fs::write(&partial, TASK).unwrap();
    let planned = serde_json::json!({
        "workspace": dunce::canonicalize(&workspace).unwrap(),
        "destination": dunce::canonicalize(workspace.join(".orbit/board")).unwrap(),
        "initialize_destination": false,
        "destination_config_before": config_before,
        "destination_config_after": null,
        "accepted_prefixes_added": [],
        "sources": [source],
        "files": [{
            "path": "tasks/teramigo-abcdef.md",
            "content": TASK.as_bytes(),
        }],
        "task_count": 1,
        "mappings": [],
        "configuration_conflicts": [],
        "commit": null,
    });
    std::fs::write(&state, serde_json::to_vec(&planned).unwrap()).unwrap();

    let resumed = repo.ok_json_in(&workspace, &["workspace", "migrate", "--resume", "--json"]);
    assert_eq!(resumed["data"]["applied"], true);
    assert_eq!(resumed["data"]["resumed"], true);
    assert!(!state.exists());
}

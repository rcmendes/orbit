mod common;

use common::TestRepo;
use serde_json::{Value, json};
use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

fn init_workspace(repo: &TestRepo) -> (tempfile::TempDir, PathBuf) {
    let dir = tempfile::tempdir().unwrap();
    let workspace = dir.path().to_path_buf();
    repo.init_at(&workspace);
    let board_config = workspace.join(".orbit/board/config.toml");
    let mut config = std::fs::read_to_string(&board_config).unwrap();
    config.push_str("\n[github]\norganization = \"acme\"\n");
    std::fs::write(board_config, config).unwrap();
    (dir, workspace)
}

fn child_repo(repo: &TestRepo, workspace: &Path, folder: &str, remote: &str) -> PathBuf {
    let path = workspace.join(folder);
    std::fs::create_dir_all(&path).unwrap();
    for args in [
        &(["init", "-q"][..]),
        &(["config", "user.name", "Test"][..]),
        &(["config", "user.email", "test@example.com"][..]),
        &(["commit", "--allow-empty", "-q", "-m", "init"][..]),
        &(["remote", "add", "origin", remote][..]),
    ] {
        let output = repo.git_in(&path, args);
        assert!(
            output.status.success(),
            "git {args:?} failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
    }
    path
}

fn child_repo_without_origin(repo: &TestRepo, workspace: &Path, folder: &str) -> PathBuf {
    let path = workspace.join(folder);
    std::fs::create_dir_all(&path).unwrap();
    for args in [
        &(["init", "-q"][..]),
        &(["config", "user.name", "Test"][..]),
        &(["config", "user.email", "test@example.com"][..]),
        &(["commit", "--allow-empty", "-q", "-m", "init"][..]),
    ] {
        let output = repo.git_in(&path, args);
        assert!(output.status.success(), "git {args:?} failed");
    }
    path
}

fn fixture(repositories: &[(&str, &str, bool)]) -> String {
    let repositories = repositories
        .iter()
        .map(|(name, owner_type, accessible)| {
            (
                name.to_string(),
                json!({"owner_type": owner_type, "accessible": accessible}),
            )
        })
        .collect::<serde_json::Map<_, _>>();
    json!({"repositories": repositories}).to_string()
}

fn run_workspace(
    repo: &TestRepo,
    workspace: &Path,
    args: &[&str],
    github_fixture: &Path,
) -> (i32, Value) {
    let mut command = vec!["--dir", workspace.to_str().unwrap()];
    command.extend_from_slice(args);
    command.push("--json");
    repo.json_with_env(
        &command,
        &[("ORBIT_GITHUB_FIXTURE", github_fixture.to_str().unwrap())],
    )
}

#[test]
fn refresh_discovers_direct_github_children_and_reports_repository_states() {
    let repo = TestRepo::new();
    let (_workspace_dir, workspace) = init_workspace(&repo);
    let repositories = [
        ("acme/api", "Organization", true),
        ("acme/cli", "Organization", true),
        ("acme/docs", "Organization", false),
        ("acme/infra", "Organization", true),
        ("acme/web", "Organization", true),
        ("someone/other", "User", true),
    ];
    for (repo_name, _, _) in repositories {
        let folder = repo_name.split('/').nth(1).unwrap();
        child_repo(
            &repo,
            &workspace,
            folder,
            &format!("git@github.com:{repo_name}.git"),
        );
    }
    std::fs::create_dir(workspace.join("notes")).unwrap();
    child_repo_without_origin(&repo, &workspace, "no-origin");
    child_repo(
        &repo,
        &workspace,
        "gitlab-project",
        "git@gitlab.com:acme/project.git",
    );
    let github_fixture = repo.home().join("github.json");
    std::fs::write(&github_fixture, fixture(&repositories)).unwrap();

    let (status, excluded) = run_workspace(
        &repo,
        &workspace,
        &["workspace", "exclude", "acme/infra"],
        &github_fixture,
    );
    assert_eq!(status, 0, "{excluded}");
    let (status, included) = run_workspace(
        &repo,
        &workspace,
        &["workspace", "include", "acme/docs"],
        &github_fixture,
    );
    assert_eq!(status, 0, "{included}");
    let (status, included) = run_workspace(
        &repo,
        &workspace,
        &["workspace", "include", "acme/api"],
        &github_fixture,
    );
    assert_eq!(status, 0, "{included}");
    let (status, refreshed) = run_workspace(
        &repo,
        &workspace,
        &["workspace", "refresh"],
        &github_fixture,
    );
    assert_eq!(status, 0, "{refreshed}");

    let (_, listed) = run_workspace(&repo, &workspace, &["workspace", "status"], &github_fixture);
    assert_eq!(listed["ok"], true, "{listed}");
    let repos = listed["data"]["repositories"].as_array().unwrap();
    let find = |name: &str| {
        repos
            .iter()
            .find(|repo| repo["repository"] == name)
            .unwrap_or_else(|| panic!("missing {name}: {listed}"))
    };
    assert_eq!(find("acme/api")["status"], "included");
    assert_eq!(find("acme/infra")["status"], "excluded");
    assert_eq!(find("acme/docs")["status"], "inaccessible");
    assert_eq!(find("acme/docs")["included"], true);
    assert_eq!(find("acme/docs")["cached"], false);
    assert_eq!(find("acme/cli")["status"], "excluded");
    assert_eq!(find("someone/other")["status"], "unsupported_owner");
    assert_eq!(listed["data"]["non_git_directories"][0], "notes");
    assert_eq!(listed["data"]["missing_origins"][0], "no-origin");
    assert_eq!(
        listed["data"]["unsupported_origins"][0]["path"],
        "gitlab-project"
    );
    let tasks = repo.ok_json_in(
        &workspace,
        &[
            "--dir",
            workspace.to_str().unwrap(),
            "list",
            "--all",
            "--json",
        ],
    );
    assert_eq!(tasks["data"]["tasks"].as_array().unwrap().len(), 0);

    let root_status = repo.ok_json_in(&workspace, &["workspace", "status", "--json"]);
    assert_eq!(root_status["data"]["organization"], "acme");
    let child_status = repo.ok_json_in(&workspace.join("api"), &["workspace", "status", "--json"]);
    assert_eq!(child_status["data"]["organization"], "acme");
}

#[test]
fn github_create_infers_the_single_included_workspace_repository() {
    let repo = TestRepo::new();
    let (_workspace_dir, workspace) = init_workspace(&repo);
    child_repo(&repo, &workspace, "api", "https://github.com/acme/api.git");
    let github_fixture = repo.home().join("github.json");
    std::fs::write(
        &github_fixture,
        json!({
            "repositories": { "acme/api": { "owner_type": "Organization" } },
            "issues": {}
        })
        .to_string(),
    )
    .unwrap();
    let (status, refreshed) = run_workspace(
        &repo,
        &workspace,
        &["workspace", "refresh"],
        &github_fixture,
    );
    assert_eq!(status, 0, "{refreshed}");
    let task = repo.ok_json_in(&workspace, &["add", "Workspace export", "--json"]);
    let id = task["data"]["id"].as_str().unwrap();

    let (status, result) = run_workspace(
        &repo,
        &workspace,
        &["github", "create", id, "--apply"],
        &github_fixture,
    );

    assert_eq!(status, 0, "{result}");
    assert_eq!(result["data"]["tasks"][0]["repository"], "acme/api");
    assert_eq!(result["data"]["tasks"][0]["success"], true);
    let task = repo.ok_json_in(&workspace, &["show", id, "--json"]);
    assert_eq!(task["data"]["github"]["repo"], "acme/api");
}

#[test]
fn refresh_marks_missing_repositories_removed_and_finds_new_children_explicitly() {
    let repo = TestRepo::new();
    let (_workspace_dir, workspace) = init_workspace(&repo);
    let initial = [("acme/api", "Organization", true)];
    let api = child_repo(&repo, &workspace, "api", "https://github.com/acme/api.git");
    let github_fixture = repo.home().join("github.json");
    std::fs::write(&github_fixture, fixture(&initial)).unwrap();
    let (status, _) = run_workspace(
        &repo,
        &workspace,
        &["workspace", "refresh"],
        &github_fixture,
    );
    assert_eq!(status, 0);

    std::fs::write(
        &github_fixture,
        fixture(&[("acme/api", "Organization", false)]),
    )
    .unwrap();
    let (status, refreshed_stale) = run_workspace(
        &repo,
        &workspace,
        &["workspace", "refresh"],
        &github_fixture,
    );
    assert_eq!(status, 0, "{refreshed_stale}");
    let (_, stale) = run_workspace(&repo, &workspace, &["workspace", "status"], &github_fixture);
    assert_eq!(stale["data"]["repositories"][0]["status"], "stale");
    assert_eq!(stale["data"]["repositories"][0]["cached"], true);

    std::fs::remove_dir_all(api).unwrap();
    let before_refresh =
        run_workspace(&repo, &workspace, &["workspace", "status"], &github_fixture);
    assert_eq!(
        before_refresh.1["data"]["repositories"][0]["status"],
        "stale"
    );

    child_repo(
        &repo,
        &workspace,
        "new-api",
        "https://github.com/acme/new-api.git",
    );
    let before_new_repo_refresh =
        run_workspace(&repo, &workspace, &["workspace", "status"], &github_fixture);
    assert!(
        !before_new_repo_refresh.1["data"]["repositories"]
            .as_array()
            .unwrap()
            .iter()
            .any(|repo| repo["repository"] == "acme/new-api")
    );
    let expanded = [
        ("acme/api", "Organization", true),
        ("acme/new-api", "Organization", true),
    ];
    std::fs::write(&github_fixture, fixture(&expanded)).unwrap();
    let (status, refreshed) = run_workspace(
        &repo,
        &workspace,
        &["workspace", "refresh"],
        &github_fixture,
    );
    assert_eq!(status, 0, "{refreshed}");

    let (_, listed) = run_workspace(&repo, &workspace, &["workspace", "status"], &github_fixture);
    let repos = listed["data"]["repositories"].as_array().unwrap();
    assert!(
        repos
            .iter()
            .any(|repo| { repo["repository"] == "acme/api" && repo["status"] == "removed" })
    );
    assert!(
        repos
            .iter()
            .any(|repo| { repo["repository"] == "acme/api" && repo["cached"] == true })
    );
    assert!(
        repos
            .iter()
            .any(|repo| { repo["repository"] == "acme/new-api" && repo["status"] == "included" })
    );
}

#[test]
fn issue_catalog_stays_outside_tasks_until_a_task_is_explicitly_linked() {
    let repo = TestRepo::new();
    let (_workspace_dir, workspace) = init_workspace(&repo);
    child_repo(&repo, &workspace, "api", "https://github.com/acme/api.git");
    let github_fixture = repo.home().join("github.json");
    let issue = |number, title: &str, state: &str, labels: &[&str]| {
        json!({
            "number": number,
            "title": title,
            "state": state,
            "labels": labels,
            "html_url": format!("https://github.com/acme/api/issues/{number}"),
            "created_at": "2026-01-01T00:00:00Z",
            "updated_at": "2026-02-01T00:00:00Z",
            "body": "must not be in the catalog cache",
            "area": "cli",
            "verification": "cargo test",
            "issue_fields": { "Area": "cli", "Verification": "cargo test", "Risk": 2 }
        })
    };
    std::fs::write(
        &github_fixture,
        json!({
            "repositories": { "acme/api": { "owner_type": "Organization" } },
            "catalog_page_size": 1,
            "catalog_issues": {
                "acme/api": [
                    issue(1, "Open catalog item", "open", &["bug"]),
                    issue(2, "Closed catalog item", "closed", &["docs"]),
                    { "number": 3, "title": "PR, not an issue", "state": "open", "labels": [], "pull_request": true }
                ]
            },
            "issue_details": {
                "acme/api#1": { "body": "Issue body loaded on inspect" }
            },
            "issue_comments": {
                "acme/api#1": [{ "id": 7, "body": "A comment", "author": "alice", "created_at": "2026-02-01T00:00:00Z" }]
            }
        })
        .to_string(),
    )
    .unwrap();

    let (status, refreshed) = run_workspace(
        &repo,
        &workspace,
        &["workspace", "issues", "refresh"],
        &github_fixture,
    );
    assert_eq!(status, 0, "{refreshed}");
    assert_eq!(refreshed["data"]["refreshed_count"], 1);

    let (status, listed) = run_workspace(
        &repo,
        &workspace,
        &[
            "workspace",
            "issues",
            "list",
            "--state",
            "closed",
            "--label",
            "docs",
        ],
        &github_fixture,
    );
    assert_eq!(status, 0, "{listed}");
    assert_eq!(listed["data"]["count"], 1);
    assert_eq!(listed["data"]["issues"][0]["identity"], "acme/api#2");
    assert_eq!(listed["data"]["issues"][0]["issue_fields"]["Risk"], 2);
    assert!(listed["data"]["issues"][0].get("body").is_none());

    let cache = std::fs::read_to_string(workspace.join(".orbit/cache/issues.json")).unwrap();
    assert!(!cache.contains("must not be in the catalog cache"));
    assert!(!cache.contains("PR, not an issue"));

    let (status, inspected) = run_workspace(
        &repo,
        &workspace,
        &["workspace", "issues", "inspect", "acme/api#1"],
        &github_fixture,
    );
    assert_eq!(status, 0, "{inspected}");
    assert_eq!(inspected["data"]["body"], "Issue body loaded on inspect");
    assert_eq!(inspected["data"]["comments"][0]["body"], "A comment");

    let ready = repo.ok_json_in(
        &workspace,
        &["--dir", workspace.to_str().unwrap(), "ready", "--json"],
    );
    assert_eq!(ready["data"]["count"], 0);
    let tasks = repo.ok_json_in(
        &workspace,
        &[
            "--dir",
            workspace.to_str().unwrap(),
            "list",
            "--all",
            "--json",
        ],
    );
    assert_eq!(tasks["data"]["tasks"].as_array().unwrap().len(), 0);

    let (_, linked_repo) = run_workspace(
        &repo,
        &workspace,
        &["github", "repo", "link", "acme/api"],
        &github_fixture,
    );
    assert_eq!(linked_repo["ok"], true, "{linked_repo}");
    let task = repo.ok_json_in(
        &workspace,
        &[
            "--dir",
            workspace.to_str().unwrap(),
            "add",
            "Linked catalog task",
            "--json",
        ],
    );
    let task_id = task["data"]["id"].as_str().unwrap();
    let task_id_arg = task_id.to_string();
    let (_, adopted) = run_workspace(
        &repo,
        &workspace,
        &[
            "github",
            "adopt",
            &task_id_arg,
            "--repo",
            "acme/api",
            "--number",
            "1",
        ],
        &github_fixture,
    );
    assert_eq!(adopted["ok"], true, "{adopted}");

    let (_, linked) = run_workspace(
        &repo,
        &workspace,
        &["workspace", "issues", "list", "--linked"],
        &github_fixture,
    );
    assert_eq!(linked["data"]["count"], 1);
    assert_eq!(linked["data"]["issues"][0]["identity"], "acme/api#1");
    let (_, unlinked) = run_workspace(
        &repo,
        &workspace,
        &[
            "workspace",
            "issues",
            "list",
            "--text",
            "closed catalog",
            "--unlinked",
        ],
        &github_fixture,
    );
    assert_eq!(unlinked["data"]["count"], 1);
    assert_eq!(unlinked["data"]["issues"][0]["identity"], "acme/api#2");
}

#[test]
fn workspace_issue_import_previews_then_creates_a_linked_task_without_writing_github() {
    let repo = TestRepo::new();
    let (_workspace_dir, workspace) = init_workspace(&repo);
    child_repo(&repo, &workspace, "api", "https://github.com/acme/api.git");
    let github_fixture = repo.home().join("github.json");
    let issue_fixture = json!({
        "repositories": { "acme/api": { "owner_type": "Organization" } },
        "catalog_issues": {
            "acme/api": [{
                "number": 17,
                "title": "Repair import",
                "state": "open",
                "labels": ["bug", "needs-triage"],
                "html_url": "https://github.com/acme/api/issues/17",
                "created_at": "2026-01-01T00:00:00Z",
                "updated_at": "2026-02-01T00:00:00Z",
                "area": "cli",
                "verification": "cargo test"
            }]
        },
        "issue_details": { "acme/api#17": { "body": "Issue description" } }
    });
    std::fs::write(&github_fixture, issue_fixture.to_string()).unwrap();
    let (status, refreshed) = run_workspace(
        &repo,
        &workspace,
        &["workspace", "issues", "refresh"],
        &github_fixture,
    );
    assert_eq!(status, 0, "{refreshed}");

    let (status, preview) = run_workspace(
        &repo,
        &workspace,
        &["workspace", "issues", "import", "acme/api#17"],
        &github_fixture,
    );
    assert_eq!(status, 0, "{preview}");
    assert_eq!(preview["data"]["apply"], false);
    assert_eq!(preview["data"]["creates"][0]["identity"], "acme/api#17");
    assert_eq!(preview["data"]["creates"][0]["status"], "todo");
    assert_eq!(preview["data"]["links"].as_array().unwrap().len(), 0);
    assert_eq!(preview["data"]["errors"].as_array().unwrap().len(), 0);
    assert_eq!(
        repo.ok_json_in(
            &workspace,
            &[
                "--dir",
                workspace.to_str().unwrap(),
                "list",
                "--all",
                "--json"
            ]
        )["data"]["tasks"]
            .as_array()
            .unwrap()
            .len(),
        0
    );

    let before_apply = std::fs::read(&github_fixture).unwrap();
    let (status, applied) = run_workspace(
        &repo,
        &workspace,
        &["workspace", "issues", "import", "acme/api#17", "--apply"],
        &github_fixture,
    );
    assert_eq!(status, 0, "{applied}");
    assert_eq!(applied["data"]["imported"][0]["identity"], "acme/api#17");
    let task_id = applied["data"]["imported"][0]["task"].as_str().unwrap();
    let task = repo.ok_json_in(
        &workspace,
        &[
            "--dir",
            workspace.to_str().unwrap(),
            "show",
            task_id,
            "--json",
        ],
    );
    assert_eq!(task["data"]["title"], "Repair import");
    assert_eq!(task["data"]["body"], "Issue description");
    assert_eq!(task["data"]["labels"], json!(["bug", "needs-triage"]));
    assert_eq!(task["data"]["area"], "cli");
    assert_eq!(task["data"]["verification"], "cargo test");
    assert_eq!(task["data"]["github"]["repo"], "acme/api");
    assert_eq!(task["data"]["github"]["number"], 17);
    assert!(task["data"]["github"]["last_sync"].is_object());
    let held_ready = repo.ok_json_in(
        &workspace,
        &["--dir", workspace.to_str().unwrap(), "ready", "--json"],
    );
    assert_eq!(held_ready["data"]["count"], 0);
    assert_eq!(std::fs::read(&github_fixture).unwrap(), before_apply);
}

#[test]
fn workspace_issue_import_links_local_tasks_without_overwriting_and_skips_both_duplicate_directions()
 {
    let repo = TestRepo::new();
    let (_workspace_dir, workspace) = init_workspace(&repo);
    child_repo(&repo, &workspace, "api", "https://github.com/acme/api.git");
    let github_fixture = repo.home().join("github.json");
    let issue_fixture = json!({
        "repositories": { "acme/api": { "owner_type": "Organization" } },
        "catalog_issues": {
            "acme/api": [
                { "number": 10, "title": "Remote title", "state": "open", "labels": ["remote-label"], "updated_at": "2026-02-01T00:00:00Z" },
                { "number": 11, "title": "Other issue", "state": "open", "labels": [], "updated_at": "2026-02-02T00:00:00Z" }
            ]
        },
        "issue_details": {
            "acme/api#10": { "body": "Remote body" },
            "acme/api#11": { "body": "Other body" }
        }
    });
    std::fs::write(&github_fixture, issue_fixture.to_string()).unwrap();
    let (status, refreshed) = run_workspace(
        &repo,
        &workspace,
        &["workspace", "issues", "refresh"],
        &github_fixture,
    );
    assert_eq!(status, 0, "{refreshed}");
    let local = repo.ok_json_in(
        &workspace,
        &[
            "--dir",
            workspace.to_str().unwrap(),
            "add",
            "Local title",
            "--body",
            "Local body",
            "--label",
            "local-label",
            "--json",
        ],
    );
    let task_id = local["data"]["id"].as_str().unwrap();
    let mapping = format!("acme/api#10={task_id}");
    let preview_args = [
        "workspace",
        "issues",
        "import",
        "acme/api#10",
        "--link",
        &mapping,
    ];
    let (status, preview) = run_workspace(&repo, &workspace, &preview_args, &github_fixture);
    assert_eq!(status, 0, "{preview}");
    assert_eq!(preview["data"]["links"][0]["task"], task_id);
    let difference_fields = preview["data"]["links"][0]["differences"]
        .as_array()
        .unwrap()
        .iter()
        .map(|difference| difference["field"].as_str().unwrap())
        .collect::<Vec<_>>();
    assert!(difference_fields.contains(&"title"));
    assert!(difference_fields.contains(&"body"));
    assert!(difference_fields.contains(&"labels"));

    let link_args = [
        "workspace",
        "issues",
        "import",
        "acme/api#10",
        "--link",
        &mapping,
        "--apply",
    ];
    let (status, linked) = run_workspace(&repo, &workspace, &link_args, &github_fixture);
    assert_eq!(status, 0, "{linked}");
    assert_eq!(linked["data"]["imported"][0]["action"], "linked");
    let still_local = repo.ok_json_in(
        &workspace,
        &[
            "--dir",
            workspace.to_str().unwrap(),
            "show",
            task_id,
            "--json",
        ],
    );
    assert_eq!(still_local["data"]["title"], "Local title");
    assert_eq!(still_local["data"]["body"], "Local body");
    assert_eq!(still_local["data"]["labels"], json!(["local-label"]));
    assert_eq!(still_local["data"]["github"]["number"], 10);

    let (status, task_duplicate) = run_workspace(
        &repo,
        &workspace,
        &[
            "workspace",
            "issues",
            "import",
            "acme/api#11",
            "--link",
            &format!("acme/api#11={task_id}"),
        ],
        &github_fixture,
    );
    assert_eq!(status, 0, "{task_duplicate}");
    assert_eq!(
        task_duplicate["data"]["skipped_duplicates"][0]["reason"],
        "task_already_linked"
    );

    let (status, issue_duplicate) = run_workspace(
        &repo,
        &workspace,
        &["workspace", "issues", "import", "acme/api#10"],
        &github_fixture,
    );
    assert_eq!(status, 0, "{issue_duplicate}");
    assert_eq!(
        issue_duplicate["data"]["skipped_duplicates"][0]["reason"],
        "issue_already_linked"
    );
}

#[test]
fn workspace_issue_import_reviews_and_applies_explicit_batches_with_terminal_mappings() {
    let repo = TestRepo::new();
    let (_workspace_dir, workspace) = init_workspace(&repo);
    child_repo(&repo, &workspace, "api", "https://github.com/acme/api.git");
    child_repo(&repo, &workspace, "web", "https://github.com/acme/web.git");
    let github_fixture = repo.home().join("github.json");
    let issue_fixture = json!({
        "repositories": {
            "acme/api": { "owner_type": "Organization" },
            "acme/web": { "owner_type": "Organization" }
        },
        "catalog_issues": {
            "acme/api": [
                { "number": 1, "title": "Open issue", "state": "open", "labels": [], "updated_at": "2026-02-01T00:00:00Z" },
                { "number": 2, "title": "Open issue", "state": "closed", "labels": [], "updated_at": "2026-02-02T00:00:00Z" },
                { "number": 4, "title": "Claimable issue", "state": "open", "labels": [], "updated_at": "2026-02-04T00:00:00Z" }
            ],
            "acme/web": [
                { "number": 3, "title": "Won't fix", "state": "closed", "labels": ["wontfix"], "updated_at": "2026-02-03T00:00:00Z" }
            ]
        }
    });
    std::fs::write(&github_fixture, issue_fixture.to_string()).unwrap();
    let (status, refreshed) = run_workspace(
        &repo,
        &workspace,
        &["workspace", "issues", "refresh"],
        &github_fixture,
    );
    assert_eq!(status, 0, "{refreshed}");

    let local = repo.ok_json_in(
        &workspace,
        &[
            "--dir",
            workspace.to_str().unwrap(),
            "add",
            "Keep local task",
            "--body",
            "Local values stay authoritative",
            "--label",
            "needs-triage",
            "--json",
        ],
    );
    let local_task = local["data"]["id"].as_str().unwrap();
    let link_mapping = format!("acme/api#1={local_task}");

    let selection = [
        "acme/api#1",
        "acme/api#2",
        "acme/web#3",
        "acme/api#1",
        "acme/api#99",
        "acme/api#4",
    ];
    let mut preview_args = vec!["workspace", "issues", "import"];
    preview_args.extend(selection);
    preview_args.extend(["--link", &link_mapping]);
    let (status, preview) = run_workspace(&repo, &workspace, &preview_args, &github_fixture);
    assert_eq!(status, 0, "{preview}");
    assert_eq!(preview["data"]["creates"].as_array().unwrap().len(), 3);
    assert_eq!(preview["data"]["links"].as_array().unwrap().len(), 1);
    assert_eq!(
        preview["data"]["terminal_tasks"].as_array().unwrap().len(),
        2
    );
    assert_eq!(
        preview["data"]["skipped_duplicates"][0]["reason"],
        "issue_selected_more_than_once"
    );
    assert_eq!(preview["data"]["errors"][0]["identity"], "acme/api#99");
    assert_eq!(preview["data"]["creates"][0]["status"], "done");
    assert_eq!(preview["data"]["creates"][1]["status"], "cancelled");
    assert_eq!(
        repo.ok_json_in(
            &workspace,
            &[
                "--dir",
                workspace.to_str().unwrap(),
                "list",
                "--all",
                "--json"
            ]
        )["data"]["tasks"]
            .as_array()
            .unwrap()
            .len(),
        1
    );

    let mut apply_args = vec!["workspace", "issues", "import"];
    apply_args.extend(selection);
    apply_args.extend(["--link", &link_mapping]);
    apply_args.push("--apply");
    let (status, applied) = run_workspace(&repo, &workspace, &apply_args, &github_fixture);
    assert_eq!(status, 0, "{applied}");
    assert_eq!(applied["data"]["imported"].as_array().unwrap().len(), 4);
    let tasks = repo.ok_json_in(
        &workspace,
        &[
            "--dir",
            workspace.to_str().unwrap(),
            "list",
            "--all",
            "--json",
        ],
    );
    let statuses = tasks["data"]["tasks"]
        .as_array()
        .unwrap()
        .iter()
        .map(|task| task["status"].as_str().unwrap())
        .collect::<BTreeSet<_>>();
    assert_eq!(statuses, BTreeSet::from(["todo", "done", "cancelled"]));
    let local_after = repo.ok_json_in(
        &workspace,
        &[
            "--dir",
            workspace.to_str().unwrap(),
            "show",
            local_task,
            "--json",
        ],
    );
    assert_eq!(local_after["data"]["title"], "Keep local task");
    assert_eq!(
        local_after["data"]["body"],
        "Local values stay authoritative"
    );
    assert_eq!(local_after["data"]["github"]["number"], 1);
    let ready = repo.ok_json_in(
        &workspace,
        &["--dir", workspace.to_str().unwrap(), "ready", "--json"],
    );
    assert_eq!(ready["data"]["count"], 1);
    assert_eq!(ready["data"]["tasks"][0]["title"], "Claimable issue");
    let claim = repo.ok_json_in(
        &workspace,
        &[
            "--dir",
            workspace.to_str().unwrap(),
            "ready",
            "--claim",
            "--by",
            "test-agent",
            "--json",
        ],
    );
    assert_eq!(claim["data"]["title"], "Claimable issue");
    assert_eq!(claim["data"]["status"], "in_progress");
}

#[test]
fn issue_catalog_keeps_previous_snapshot_and_reports_partial_repository_failures() {
    let repo = TestRepo::new();
    let (_workspace_dir, workspace) = init_workspace(&repo);
    child_repo(&repo, &workspace, "api", "https://github.com/acme/api.git");
    child_repo(&repo, &workspace, "web", "https://github.com/acme/web.git");
    let github_fixture = repo.home().join("github.json");
    let initial = json!({
        "repositories": {
            "acme/api": { "owner_type": "Organization" },
            "acme/web": { "owner_type": "Organization" }
        },
        "catalog_issues": {
            "acme/api": [{ "number": 1, "title": "Keep me", "state": "open", "labels": [] }],
            "acme/web": [{ "number": 2, "title": "Web issue", "state": "open", "labels": [] }]
        }
    });
    std::fs::write(&github_fixture, initial.to_string()).unwrap();
    let (status, refreshed) = run_workspace(
        &repo,
        &workspace,
        &["workspace", "issues", "refresh"],
        &github_fixture,
    );
    assert_eq!(status, 0, "{refreshed}");

    let mut failed_refresh = initial;
    failed_refresh["catalog_errors"] = json!({ "acme/api": "fixture API unavailable" });
    failed_refresh["catalog_issues"]["acme/web"] = json!([
        { "number": 3, "title": "New web issue", "state": "closed", "labels": [] }
    ]);
    std::fs::write(&github_fixture, failed_refresh.to_string()).unwrap();
    let (status, refreshed) = run_workspace(
        &repo,
        &workspace,
        &["workspace", "issues", "refresh"],
        &github_fixture,
    );
    assert_eq!(
        status, 0,
        "a per-repository failure is partial success: {refreshed}"
    );
    assert_eq!(refreshed["data"]["errors"][0]["repository"], "acme/api");

    let (_, listed) = run_workspace(
        &repo,
        &workspace,
        &["workspace", "issues", "list", "--repo", "acme/api"],
        &github_fixture,
    );
    assert_eq!(listed["data"]["issues"][0]["title"], "Keep me");
    assert_eq!(listed["data"]["repositories"][0]["stale"], true);

    let (_, web) = run_workspace(
        &repo,
        &workspace,
        &["workspace", "issues", "list", "--repo", "acme/web"],
        &github_fixture,
    );
    assert_eq!(web["data"]["issues"][0]["title"], "New web issue");
}

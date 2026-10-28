mod common;

use common::TestRepo;
use serde_json::{Value, json};
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

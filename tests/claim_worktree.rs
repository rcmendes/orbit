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

fn worktree_branch(repo: &TestRepo, worktree: &Path) -> String {
    let output = repo.git_in(worktree, &["rev-parse", "--abbrev-ref", "HEAD"]);
    assert!(output.status.success());
    String::from_utf8_lossy(&output.stdout).trim().to_string()
}

#[test]
fn claim_worktree_creates_the_worktree_in_the_tasks_repository() {
    let repo = TestRepo::new();
    repo.init();
    let api = git_repo(&repo, &repo.path().join("api"));
    repo.ok_json(&[
        "repo",
        "add",
        api.to_str().unwrap(),
        "--name",
        "api",
        "--json",
    ]);
    let added = repo.ok_json_in(&api, &["add", "API work", "--json"]);
    let id = added["data"]["id"].as_str().unwrap().to_string();

    // The claim runs from the Board root: the Task's own `repo` field picks
    // the member Repository, not the caller's directory.
    let value = repo.ok_json(&["claim", &id, "--by", "devin", "--worktree", "--json"]);

    let worktree = api.join(".worktrees").join(&id);
    assert!(worktree.join(".git").is_file(), "{value}");
    assert_eq!(worktree_branch(&repo, &worktree), format!("ticket/{id}"));
    let exclude = std::fs::read_to_string(api.join(".git/info/exclude")).unwrap();
    assert!(exclude.contains("/.worktrees/"), "{exclude}");

    let record = &value["data"]["worktree"];
    assert_eq!(record["repository"], "api", "{value}");
    assert_eq!(record["branch"], format!("ticket/{id}"), "{value}");
    assert_eq!(record["created"], true, "{value}");
    assert_eq!(
        record["path"].as_str().unwrap(),
        dunce::canonicalize(&worktree).unwrap().to_str().unwrap()
    );
    assert_eq!(value["data"]["status"], "in_progress", "{value}");
}

#[test]
fn claim_worktree_is_idempotent_for_the_same_holder() {
    let repo = TestRepo::new();
    repo.init();
    let api = git_repo(&repo, &repo.path().join("api"));
    repo.ok_json(&["repo", "add", api.to_str().unwrap(), "--json"]);
    let added = repo.ok_json_in(&api, &["add", "API work", "--json"]);
    let id = added["data"]["id"].as_str().unwrap().to_string();

    repo.ok_json(&["claim", &id, "--by", "devin", "--worktree", "--json"]);
    let second = repo.ok_json(&["claim", &id, "--by", "devin", "--worktree", "--json"]);

    assert_eq!(second["data"]["worktree"]["created"], false, "{second}");
    assert_eq!(
        worktree_branch(&repo, &api.join(".worktrees").join(&id)),
        format!("ticket/{id}")
    );
}

#[test]
fn claim_worktree_reuses_an_existing_ticket_branch() {
    let repo = TestRepo::new();
    repo.init();
    let api = git_repo(&repo, &repo.path().join("api"));
    repo.ok_json(&["repo", "add", api.to_str().unwrap(), "--json"]);
    let added = repo.ok_json_in(&api, &["add", "API work", "--json"]);
    let id = added["data"]["id"].as_str().unwrap().to_string();

    // A leftover branch from an earlier attempt is checked out, not
    // recreated.
    let output = repo.git_in(&api, &["branch", &format!("ticket/{id}")]);
    assert!(output.status.success());

    repo.ok_json(&["claim", &id, "--by", "devin", "--worktree", "--json"]);
    assert_eq!(
        worktree_branch(&repo, &api.join(".worktrees").join(&id)),
        format!("ticket/{id}")
    );
}

#[test]
fn claim_worktree_falls_back_to_the_current_member_repository() {
    let repo = TestRepo::new();
    repo.init();
    let api = git_repo(&repo, &repo.path().join("api"));
    repo.ok_json(&["repo", "add", api.to_str().unwrap(), "--json"]);
    // A Board-wide Task records no `repo`; claiming it from inside a member
    // Repository uses that member.
    let id = repo.add("Board work", &[]);

    let value = repo.ok_json_in(
        &api,
        &["claim", &id, "--by", "devin", "--worktree", "--json"],
    );

    assert_eq!(value["data"]["worktree"]["repository"], "api", "{value}");
    assert!(api.join(".worktrees").join(&id).join(".git").is_file());
}

#[test]
fn claim_worktree_without_a_repository_fails() {
    let repo = TestRepo::new();
    repo.init();
    let id = repo.add("Board work", &[]);

    // The Board root is not a member Repository and the Task names none.
    let (status, value) = repo.json(&["claim", &id, "--by", "devin", "--worktree", "--json"]);

    assert_eq!(status, 1, "{value}");
    assert_eq!(value["error"]["code"], "NOT_A_REPOSITORY");
}

#[test]
fn claim_worktree_rejects_a_blocked_target_path() {
    let repo = TestRepo::new();
    repo.init();
    let api = git_repo(&repo, &repo.path().join("api"));
    repo.ok_json(&["repo", "add", api.to_str().unwrap(), "--json"]);
    let added = repo.ok_json_in(&api, &["add", "API work", "--json"]);
    let id = added["data"]["id"].as_str().unwrap().to_string();

    // A non-worktree directory already occupies the target.
    std::fs::create_dir_all(api.join(".worktrees").join(&id)).unwrap();

    let (status, value) = repo.json(&["claim", &id, "--by", "devin", "--worktree", "--json"]);

    assert_eq!(status, 1, "{value}");
    assert_eq!(value["error"]["code"], "GIT_ERROR");
}

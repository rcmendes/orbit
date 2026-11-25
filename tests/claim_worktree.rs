mod common;

use common::TestRepo;
use std::path::Path;

fn worktree_branch(repo: &TestRepo, worktree: &Path) -> String {
    let output = repo.git_in(worktree, &["rev-parse", "--abbrev-ref", "HEAD"]);
    assert!(output.status.success());
    String::from_utf8_lossy(&output.stdout).trim().to_string()
}

#[test]
fn claim_worktree_creates_the_worktree_in_the_tasks_repository() {
    let repo = TestRepo::new();
    repo.init();
    let api = repo.named_repo("api");
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
    let api = repo.named_repo("api");
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

/// Creates `branch` in `cwd` with one empty commit and returns its tip,
/// restoring the previously checked-out branch.
fn commit_on_branch(repo: &TestRepo, cwd: &Path, branch: &str) -> String {
    let original = worktree_branch(repo, cwd);
    assert!(
        repo.git_in(cwd, &["checkout", "-q", "-b", branch])
            .status
            .success()
    );
    assert!(
        repo.git_in(cwd, &["commit", "--allow-empty", "-q", "-m", branch])
            .status
            .success()
    );
    let tip = String::from_utf8_lossy(&repo.git_in(cwd, &["rev-parse", "HEAD"]).stdout)
        .trim()
        .to_string();
    assert!(
        repo.git_in(cwd, &["checkout", "-q", &original])
            .status
            .success()
    );
    tip
}

fn head_commit(repo: &TestRepo, cwd: &Path) -> String {
    String::from_utf8_lossy(&repo.git_in(cwd, &["rev-parse", "HEAD"]).stdout)
        .trim()
        .to_string()
}

#[test]
fn claim_worktree_base_starts_the_ticket_branch_at_the_given_ref() {
    let repo = TestRepo::new();
    repo.init();
    let api = repo.named_repo("api");
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
    let tip = commit_on_branch(&repo, &api, "feature/x");

    let value = repo.ok_json(&[
        "claim",
        &id,
        "--by",
        "devin",
        "--worktree",
        "--base",
        "feature/x",
        "--json",
    ]);

    let worktree = api.join(".worktrees").join(&id);
    assert_eq!(head_commit(&repo, &worktree), tip, "{value}");
    assert_eq!(
        worktree_branch(&repo, &worktree),
        format!("ticket/{id}"),
        "{value}"
    );
    assert_eq!(value["data"]["worktree"]["created"], true, "{value}");
    assert!(value["meta"]["warnings"].is_null(), "{value}");
}

#[test]
fn claim_base_without_worktree_is_a_usage_error() {
    let repo = TestRepo::new();
    repo.init();
    let id = repo.add("Board work", &[]);

    let output = repo.orbit(&["claim", &id, "--by", "devin", "--base", "main"]);

    assert!(!output.status.success());
    let shown = repo.ok_json(&["show", &id, "--json"]);
    assert_eq!(shown["data"]["status"], "ready", "{shown}");
}

#[test]
fn claim_worktree_base_with_an_unknown_ref_fails_before_claiming() {
    let repo = TestRepo::new();
    repo.init();
    let api = repo.named_repo("api");
    repo.ok_json(&["repo", "add", api.to_str().unwrap(), "--json"]);
    let added = repo.ok_json_in(&api, &["add", "API work", "--json"]);
    let id = added["data"]["id"].as_str().unwrap().to_string();

    let (status, value) = repo.json(&[
        "claim",
        &id,
        "--by",
        "devin",
        "--worktree",
        "--base",
        "does/not-exist",
        "--json",
    ]);

    assert_eq!(status, 1, "{value}");
    assert_eq!(value["error"]["code"], "INVALID_ARGUMENT", "{value}");
    let shown = repo.ok_json(&["show", &id, "--json"]);
    assert_eq!(shown["data"]["status"], "ready", "{shown}");
    assert!(!api.join(".worktrees").join(&id).exists());
}

#[test]
fn claim_worktree_base_warns_when_reusing_a_branch_on_another_base() {
    let repo = TestRepo::new();
    repo.init();
    let api = repo.named_repo("api");
    repo.ok_json(&["repo", "add", api.to_str().unwrap(), "--json"]);
    let added = repo.ok_json_in(&api, &["add", "API work", "--json"]);
    let id = added["data"]["id"].as_str().unwrap().to_string();

    // An existing ticket branch rooted elsewhere is kept, and the claim
    // reports that it is not based on the requested ref.
    let ticket_tip = commit_on_branch(&repo, &api, &format!("ticket/{id}"));
    commit_on_branch(&repo, &api, "feature/x");

    let value = repo.ok_json(&[
        "claim",
        &id,
        "--by",
        "devin",
        "--worktree",
        "--base",
        "feature/x",
        "--json",
    ]);

    let worktree = api.join(".worktrees").join(&id);
    assert_eq!(head_commit(&repo, &worktree), ticket_tip, "{value}");
    let warnings = value["meta"]["warnings"].as_array().unwrap();
    assert_eq!(warnings.len(), 1, "{value}");
    assert!(
        warnings[0]
            .as_str()
            .unwrap()
            .contains(&format!("ticket/{id}")),
        "{value}"
    );
}

#[test]
fn claim_worktree_reuses_an_existing_ticket_branch() {
    let repo = TestRepo::new();
    repo.init();
    let api = repo.named_repo("api");
    repo.ok_json(&["repo", "add", api.to_str().unwrap(), "--json"]);
    let added = repo.ok_json_in(&api, &["add", "API work", "--json"]);
    let id = added["data"]["id"].as_str().unwrap().to_string();

    // A leftover branch from an earlier attempt is checked out, not
    // recreated.
    let original = worktree_branch(&repo, &api);
    assert!(
        repo.git_in(&api, &["checkout", "-b", &format!("ticket/{id}")])
            .status
            .success()
    );
    assert!(
        repo.git_in(&api, &["commit", "--allow-empty", "-m", "existing work"])
            .status
            .success()
    );
    let tip = repo.git_in(&api, &["rev-parse", "HEAD"]).stdout;
    assert!(repo.git_in(&api, &["checkout", &original]).status.success());

    repo.ok_json(&["claim", &id, "--by", "devin", "--worktree", "--json"]);
    assert_eq!(
        repo.git_in(&api.join(".worktrees").join(&id), &["rev-parse", "HEAD"])
            .stdout,
        tip
    );
    assert_eq!(
        worktree_branch(&repo, &api.join(".worktrees").join(&id)),
        format!("ticket/{id}")
    );
}

#[test]
fn claim_worktree_falls_back_to_the_current_member_repository() {
    let repo = TestRepo::new();
    repo.init();
    let api = repo.named_repo("api");
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
fn claim_worktree_rejects_a_branch_checked_out_elsewhere_before_claiming() {
    let repo = TestRepo::new();
    repo.init();
    let api = repo.named_repo("api");
    repo.ok_json(&["repo", "add", api.to_str().unwrap(), "--json"]);
    let added = repo.ok_json_in(&api, &["add", "API work", "--json"]);
    let id = added["data"]["id"].as_str().unwrap();
    let other = api.join("other");
    assert!(
        repo.git_in(
            &api,
            &[
                "worktree",
                "add",
                "-b",
                &format!("ticket/{id}"),
                other.to_str().unwrap()
            ]
        )
        .status
        .success()
    );

    let (status, value) = repo.json(&["claim", id, "--by", "devin", "--worktree", "--json"]);
    assert_eq!(status, 1, "{value}");
    assert_eq!(value["error"]["code"], "GIT_ERROR", "{value}");
    let shown = repo.ok_json(&["show", id, "--json"]);
    assert_eq!(shown["data"]["status"], "ready", "{shown}");
}

#[test]
fn worktree_runtime_failure_reports_and_preserves_successful_claim() {
    for already_claimed in [false, true] {
        let repo = TestRepo::new();
        repo.init();
        let api = repo.named_repo("api");
        repo.ok_json(&["repo", "add", api.to_str().unwrap(), "--json"]);
        let added = repo.ok_json_in(&api, &["add", "API work", "--json"]);
        let id = added["data"]["id"].as_str().unwrap();
        if already_claimed {
            repo.ok_json(&["claim", id, "--by", "devin", "--json"]);
        }
        // Portable deterministic I/O failure after claim commit, without chmod or races.
        let exclude = api.join(".git/info/exclude");
        std::fs::remove_file(&exclude).unwrap();
        std::fs::create_dir(&exclude).unwrap();
        let (status, value) = repo.json(&["claim", id, "--by", "devin", "--worktree", "--json"]);
        assert_eq!(status, 1, "{value}");
        assert_eq!(value["error"]["code"], "CLAIM_WORKTREE_FAILED", "{value}");
        assert!(
            value["error"]["message"]
                .as_str()
                .unwrap()
                .contains("claim remains active"),
            "{value}"
        );
        assert_eq!(
            value["error"]["details"]["claim"]["status"], "in_progress",
            "{value}"
        );
        assert_eq!(
            value["error"]["details"]["claim"]["assignee"], "devin",
            "{value}"
        );
        assert_eq!(
            value["error"]["details"]["cause"]["code"], "IO_ERROR",
            "{value}"
        );
        let shown = repo.ok_json(&["show", id, "--json"]);
        assert_eq!(shown["data"]["status"], "in_progress", "{shown}");
        assert_eq!(shown["data"]["assignee"], "devin", "{shown}");
    }
}

#[test]
fn claim_worktree_rejects_a_blocked_target_path() {
    let repo = TestRepo::new();
    repo.init();
    let api = repo.named_repo("api");
    repo.ok_json(&["repo", "add", api.to_str().unwrap(), "--json"]);
    let added = repo.ok_json_in(&api, &["add", "API work", "--json"]);
    let id = added["data"]["id"].as_str().unwrap().to_string();

    // A non-worktree directory already occupies the target.
    std::fs::create_dir_all(api.join(".worktrees").join(&id)).unwrap();

    let (status, value) = repo.json(&["claim", &id, "--by", "devin", "--worktree", "--json"]);

    assert_eq!(status, 1, "{value}");
    assert_eq!(value["error"]["code"], "GIT_ERROR");
}

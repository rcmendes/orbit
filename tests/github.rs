mod common;

use common::TestRepo;
use std::fs;
use tempfile::TempDir;

fn write_github_config(repo: &TestRepo) {
    write_github_config_for_repositories(repo, &["acme/widget"]);
}

fn write_github_config_for_repositories(repo: &TestRepo, repositories: &[&str]) {
    let path = repo.board_path().join("config.toml");
    let mut config = fs::read_to_string(&path).unwrap();
    let repositories = repositories
        .iter()
        .map(|repository| format!("\"{repository}\""))
        .collect::<Vec<_>>()
        .join(", ");
    config.push_str(&format!(
        "\n[github]\norganization = \"acme\"\nrepositories = [{repositories}]\n"
    ));
    fs::write(path, config).unwrap();
}

fn write_basic_github_config(repo: &TestRepo) {
    let path = repo.board_path().join("config.toml");
    let mut config = fs::read_to_string(&path).unwrap();
    config.push_str("\n[github]\nmode = \"basic\"\nrepositories = [\"personal/widget\"]\n");
    fs::write(path, config).unwrap();
}

fn write_fixture(dir: &TempDir) -> std::path::PathBuf {
    let path = dir.path().join("fixture.json");
    fs::write(
        &path,
        r#"{
  "org_fields": {
    "acme/Area": "AREA_ID",
    "acme/Verification": "VERIFICATION_ID"
  },
  "issues": {
    "acme/widget#42": {
      "repo": "acme/widget",
      "number": 42,
      "title": "Imported issue",
      "body": "Body from GitHub",
      "state": "open",
      "labels": ["bug"],
      "assignees": ["human"],
      "updated_at": "2026-10-23T13:00:00Z",
      "area": "cli",
      "verification": "cargo test"
    }
  }
}"#,
    )
    .unwrap();
    path
}

fn adopt_fixture_task(repo: &TestRepo, fixture: &std::path::Path, title: &str) -> String {
    repo.init();
    write_github_config(repo);
    let id = repo.add(title, &[]);
    repo.ok_json_with_env(
        &[
            "github",
            "adopt",
            id.as_str(),
            "--repo",
            "acme/widget",
            "--number",
            "42",
            "--json",
        ],
        &[("ORBIT_GITHUB_FIXTURE", fixture.to_str().unwrap())],
    );
    id
}

fn set_fixture_issue_field(fixture: &std::path::Path, field: &str, value: serde_json::Value) {
    let mut data: serde_json::Value = serde_json::from_slice(&fs::read(fixture).unwrap()).unwrap();
    data["issues"]["acme/widget#42"][field] = value;
    fs::write(fixture, serde_json::to_vec_pretty(&data).unwrap()).unwrap();
}

fn set_task_frontmatter(repo: &TestRepo, id: &str, field: &str, value: &str) {
    let content = repo.read_task(id);
    let updated = content
        .lines()
        .map(|line| {
            if line.starts_with(&format!("{field}:")) {
                format!("{field}: {value}")
            } else {
                line.to_string()
            }
        })
        .collect::<Vec<_>>()
        .join("\n");
    repo.write_task(id, &(updated + "\n"));
}

#[test]
fn github_repo_link_and_adopt_use_fixture_client() {
    let fixture_dir = tempfile::tempdir().unwrap();
    let fixture = write_fixture(&fixture_dir);
    let repo = TestRepo::new();
    repo.init();
    write_github_config(&repo);

    let link = repo.ok_json(&["github", "repo", "link", "acme/widget", "--json"]);
    assert_eq!(link["data"]["repository"], "acme/widget");

    let id = repo.add("Sync me", &[]);
    let adopt = repo.ok_json_with_env(
        &[
            "github",
            "adopt",
            id.as_str(),
            "--repo",
            "acme/widget",
            "--number",
            "42",
            "--json",
        ],
        &[("ORBIT_GITHUB_FIXTURE", fixture.to_str().unwrap())],
    );
    assert_eq!(adopt["data"]["github"]["number"], 42);
    assert_eq!(adopt["data"]["issue"]["title"], "Imported issue");

    let show = repo.ok_json(&["show", id.as_str(), "--json"]);
    assert_eq!(show["data"]["github"]["repo"], "acme/widget");
    assert_eq!(show["data"]["title"], "Imported issue");
    assert_eq!(show["data"]["area"], "cli");
}

#[test]
fn github_setup_validate_only_reads_fixture_fields() {
    let fixture_dir = tempfile::tempdir().unwrap();
    let fixture = write_fixture(&fixture_dir);
    let repo = TestRepo::new();
    repo.init();
    write_github_config(&repo);
    let setup = repo.ok_json_with_env(
        &["github", "setup", "--validate-only", "--json"],
        &[("ORBIT_GITHUB_FIXTURE", fixture.to_str().unwrap())],
    );
    assert_eq!(setup["data"]["area_id"], "AREA_ID");
    assert_eq!(setup["data"]["verification_id"], "VERIFICATION_ID");
}

#[test]
fn github_sync_dry_run_reports_assignee_warning() {
    let fixture_dir = tempfile::tempdir().unwrap();
    let fixture = write_fixture(&fixture_dir);
    let repo = TestRepo::new();
    repo.init();
    write_github_config(&repo);
    let id = repo.add("Local title", &[]);
    repo.ok_json_with_env(
        &[
            "github",
            "adopt",
            id.as_str(),
            "--repo",
            "acme/widget",
            "--number",
            "42",
            "--json",
        ],
        &[("ORBIT_GITHUB_FIXTURE", fixture.to_str().unwrap())],
    );

    let plan = repo.ok_json_with_env(
        &["github", "sync", id.as_str(), "--json"],
        &[("ORBIT_GITHUB_FIXTURE", fixture.to_str().unwrap())],
    );
    assert_eq!(plan["data"]["count"], 1);
    let warnings = plan["data"]["tasks"][0]["plan"]["warnings"]
        .as_array()
        .unwrap();
    assert!(
        warnings
            .iter()
            .any(|warning| warning.as_str().unwrap().contains("assignees"))
    );
}

#[test]
fn basic_mode_rejects_organization_issue_field_configuration() {
    let repo = TestRepo::new();
    repo.init();
    let path = repo.board_path().join("config.toml");
    let mut config = fs::read_to_string(&path).unwrap();
    config.push_str(
        "\n[github]\nmode = \"basic\"\norganization = \"acme\"\nrepositories = [\"acme/widget\"]\n",
    );
    fs::write(path, config).unwrap();

    let (status, value) = repo.json(&["github", "repo", "list", "--json"]);
    assert_eq!(status, 1);
    assert_eq!(value["error"]["code"], "CONFIG_ERROR");
    assert!(
        value["error"]["message"]
            .as_str()
            .unwrap()
            .contains("basic")
    );
}

#[test]
fn basic_mode_rejects_create_when_task_has_structured_metadata() {
    let fixture_dir = tempfile::tempdir().unwrap();
    let fixture = write_fixture(&fixture_dir);
    let repo = TestRepo::new();
    repo.init();
    write_basic_github_config(&repo);
    let id = repo.add(
        "Personal issue",
        &["--area", "cli", "--verification", "cargo test"],
    );

    let (status, value) = repo.json_with_env(
        &[
            "github",
            "create",
            id.as_str(),
            "--repo",
            "personal/widget",
            "--json",
        ],
        &[("ORBIT_GITHUB_FIXTURE", fixture.to_str().unwrap())],
    );
    assert_eq!(status, 1);
    assert_eq!(value["error"]["code"], "INVALID_ARGUMENT");
    assert!(value["error"]["message"].as_str().unwrap().contains("area"));
    assert!(
        value["error"]["message"]
            .as_str()
            .unwrap()
            .contains("verification")
    );
}

#[test]
fn basic_mode_creates_personal_repository_issue_without_setup() {
    let fixture_dir = tempfile::tempdir().unwrap();
    let fixture = write_fixture(&fixture_dir);
    let repo = TestRepo::new();
    repo.init();
    write_basic_github_config(&repo);
    let id = repo.add("Personal issue", &["--label", "bug"]);

    let created = repo.ok_json_with_env(
        &[
            "github",
            "create",
            id.as_str(),
            "--repo",
            "personal/widget",
            "--json",
        ],
        &[("ORBIT_GITHUB_FIXTURE", fixture.to_str().unwrap())],
    );
    assert_eq!(created["data"]["issue"]["title"], "Personal issue");
    assert_eq!(
        created["data"]["issue"]["labels"],
        serde_json::json!(["bug"])
    );
    assert!(created["data"]["issue"]["area"].is_null());
    assert!(created["data"]["issue"]["verification"].is_null());
}

#[test]
fn basic_mode_does_not_run_issue_field_setup() {
    let repo = TestRepo::new();
    repo.init();
    write_basic_github_config(&repo);

    let (status, value) = repo.json(&["github", "setup", "--json"]);
    assert_eq!(status, 1);
    assert_eq!(value["error"]["code"], "INVALID_ARGUMENT");
    assert!(
        value["error"]["message"]
            .as_str()
            .unwrap()
            .contains("unavailable in basic mode")
    );
}

#[test]
fn basic_mode_adopts_and_syncs_without_importing_issue_fields() {
    let fixture_dir = tempfile::tempdir().unwrap();
    let fixture = write_fixture(&fixture_dir);
    let fixture_content = fs::read_to_string(&fixture)
        .unwrap()
        .replace("acme/widget#42", "personal/widget#42")
        .replace("acme/widget", "personal/widget");
    fs::write(&fixture, fixture_content).unwrap();
    let repo = TestRepo::new();
    repo.init();
    write_basic_github_config(&repo);
    let id = repo.add("Local task", &[]);

    let adopted = repo.ok_json_with_env(
        &[
            "github",
            "adopt",
            id.as_str(),
            "--repo",
            "personal/widget",
            "--number",
            "42",
            "--json",
        ],
        &[("ORBIT_GITHUB_FIXTURE", fixture.to_str().unwrap())],
    );
    assert!(adopted["data"]["issue"]["area"].is_null());
    assert!(adopted["data"]["issue"]["verification"].is_null());
    let show = repo.ok_json(&["show", id.as_str(), "--json"]);
    assert!(show["data"]["area"].is_null());
    assert!(show["data"]["verification"].is_null());

    let sync = repo.ok_json_with_env(
        &["github", "sync", id.as_str(), "--json"],
        &[("ORBIT_GITHUB_FIXTURE", fixture.to_str().unwrap())],
    );
    assert_eq!(sync["data"]["count"], 1);
    assert!(sync["data"]["tasks"][0]["plan"]["apply_area"].is_null());
    assert!(sync["data"]["tasks"][0]["plan"]["push_verification"].is_null());

    repo.ok_json(&[
        "update",
        id.as_str(),
        "--area",
        "cli",
        "--verification",
        "cargo test",
        "--json",
    ]);
    let (status, value) = repo.json_with_env(
        &[
            "github",
            "sync",
            id.as_str(),
            "--direction",
            "push",
            "--apply",
            "--json",
        ],
        &[("ORBIT_GITHUB_FIXTURE", fixture.to_str().unwrap())],
    );
    assert_eq!(status, 1);
    assert_eq!(value["error"]["code"], "INVALID_ARGUMENT");
    let message = value["error"]["message"].as_str().unwrap();
    assert!(message.contains("area"));
    assert!(message.contains("verification"));
}

#[test]
fn github_sync_human_dry_run_marks_clean_plan_as_all_clear() {
    let fixture_dir = tempfile::tempdir().unwrap();
    let fixture = write_fixture(&fixture_dir);
    let repo = TestRepo::new();
    let id = adopt_fixture_task(&repo, &fixture, "Local title");

    let output = repo.orbit_with_env(
        &["github", "sync", id.as_str()],
        &[("ORBIT_GITHUB_FIXTURE", fixture.to_str().unwrap())],
    );
    assert!(output.status.success());
    let human = String::from_utf8_lossy(&output.stdout);
    assert!(human.contains("nothing was written"), "{human}");
    assert!(human.contains(&format!("Task: {id}")), "{human}");
    assert!(human.contains("acme/widget#42"), "{human}");
    assert!(human.contains("No changes needed"), "{human}");
}

#[test]
fn github_sync_human_dry_run_explains_empty_plan() {
    let fixture_dir = tempfile::tempdir().unwrap();
    let fixture = write_fixture(&fixture_dir);
    let repo = TestRepo::new();
    repo.init();
    write_github_config(&repo);

    let output = repo.orbit_with_env(
        &["github", "sync"],
        &[("ORBIT_GITHUB_FIXTURE", fixture.to_str().unwrap())],
    );
    assert!(output.status.success());
    let human = String::from_utf8_lossy(&output.stdout);
    assert!(human.contains("No linked Tasks to sync"), "{human}");
    assert!(human.contains("nothing was written"), "{human}");
}

#[test]
fn github_sync_human_dry_run_lists_pull_changes() {
    let fixture_dir = tempfile::tempdir().unwrap();
    let fixture = write_fixture(&fixture_dir);
    let repo = TestRepo::new();
    let id = adopt_fixture_task(&repo, &fixture, "Local title");
    set_fixture_issue_field(&fixture, "title", serde_json::json!("Updated remotely"));
    set_fixture_issue_field(
        &fixture,
        "updated_at",
        serde_json::json!("2026-10-24T13:00:00Z"),
    );

    let output = repo.orbit_with_env(
        &["github", "sync", id.as_str()],
        &[("ORBIT_GITHUB_FIXTURE", fixture.to_str().unwrap())],
    );
    assert!(output.status.success());
    let human = String::from_utf8_lossy(&output.stdout);
    assert!(human.contains("Pull into Orbit:"), "{human}");
    assert!(human.contains("title: \"Updated remotely\""), "{human}");
    assert!(human.contains("Push to GitHub:\n  (none)"), "{human}");
    assert!(human.contains("nothing was written"), "{human}");
}

#[test]
fn github_sync_human_dry_run_lists_push_changes() {
    let fixture_dir = tempfile::tempdir().unwrap();
    let fixture = write_fixture(&fixture_dir);
    let repo = TestRepo::new();
    let id = adopt_fixture_task(&repo, &fixture, "Local title");
    set_task_frontmatter(&repo, &id, "area", "setup");
    set_task_frontmatter(&repo, &id, "updated_at", "2026-10-24T14:00:00Z");

    let output = repo.orbit_with_env(
        &["github", "sync", id.as_str(), "--direction", "push"],
        &[("ORBIT_GITHUB_FIXTURE", fixture.to_str().unwrap())],
    );
    assert!(output.status.success());
    let human = String::from_utf8_lossy(&output.stdout);
    assert!(human.contains("Pull into Orbit:\n  (none)"), "{human}");
    assert!(human.contains("Push to GitHub:"), "{human}");
    assert!(human.contains("area: \"setup\""), "{human}");
    assert!(human.contains("nothing was written"), "{human}");
}

#[test]
fn github_sync_human_dry_run_shows_conflicting_values_and_keeps_json_plan() {
    let fixture_dir = tempfile::tempdir().unwrap();
    let fixture = write_fixture(&fixture_dir);
    let repo = TestRepo::new();
    let id = adopt_fixture_task(&repo, &fixture, "Local title");
    set_task_frontmatter(&repo, &id, "title", "\"Local edit\"");
    set_task_frontmatter(&repo, &id, "updated_at", "2026-11-01T14:00:00Z");
    set_fixture_issue_field(&fixture, "title", serde_json::json!("Remote edit"));
    set_fixture_issue_field(
        &fixture,
        "updated_at",
        serde_json::json!("2026-10-24T15:00:00Z"),
    );
    let task_before = repo.read_task(&id);
    let fixture_before = fs::read(&fixture).unwrap();

    let human_output = repo.orbit_with_env(
        &["github", "sync", id.as_str()],
        &[("ORBIT_GITHUB_FIXTURE", fixture.to_str().unwrap())],
    );
    assert!(human_output.status.success());
    let human = String::from_utf8_lossy(&human_output.stdout);
    assert!(human.contains("Conflicts:"), "{human}");
    assert!(
        human.contains("title: Orbit = \"Local edit\"; GitHub = \"Remote edit\""),
        "{human}"
    );
    assert!(human.contains("nothing was written"), "{human}");
    assert_eq!(repo.read_task(&id), task_before);
    assert_eq!(fs::read(&fixture).unwrap(), fixture_before);

    let json = repo.ok_json_with_env(
        &["github", "sync", id.as_str(), "--json"],
        &[("ORBIT_GITHUB_FIXTURE", fixture.to_str().unwrap())],
    );
    let plan = &json["data"]["tasks"][0]["plan"];
    let conflict = &plan["conflicts"][0];
    assert_eq!(json["data"]["apply"], false);
    assert_eq!(conflict["field"], "title");
    assert_eq!(conflict["orbit_value"], "Local edit");
    assert_eq!(conflict["github_value"], "Remote edit");
}

#[test]
fn github_adopt_accepts_issue_url_and_legacy_arguments() {
    let fixture_dir = tempfile::tempdir().unwrap();
    let fixture = write_fixture(&fixture_dir);
    let repo = TestRepo::new();
    repo.init();
    write_github_config_for_repositories(&repo, &["acme/widget"]);
    let by_url = repo.add("URL issue", &[]);
    let adopted = repo.ok_json_with_env(
        &[
            "github",
            "adopt",
            by_url.as_str(),
            "https://github.com/acme/widget/issues/42",
            "--json",
        ],
        &[("ORBIT_GITHUB_FIXTURE", fixture.to_str().unwrap())],
    );
    assert_eq!(adopted["data"]["github"]["repo"], "acme/widget");
    assert_eq!(adopted["data"]["github"]["number"], 42);

    let legacy = repo.add("Legacy issue", &[]);
    let adopted = repo.ok_json_with_env(
        &[
            "github",
            "adopt",
            legacy.as_str(),
            "--repo",
            "acme/widget",
            "--number",
            "42",
            "--json",
        ],
        &[("ORBIT_GITHUB_FIXTURE", fixture.to_str().unwrap())],
    );
    assert_eq!(adopted["data"]["github"]["repo"], "acme/widget");
    assert_eq!(adopted["data"]["github"]["number"], 42);
}

#[test]
fn github_adopt_rejects_malformed_urls_and_mismatched_identifiers() {
    let fixture_dir = tempfile::tempdir().unwrap();
    let fixture = write_fixture(&fixture_dir);
    let repo = TestRepo::new();
    repo.init();
    write_github_config_for_repositories(&repo, &["acme/widget", "acme/other"]);

    let malformed_task = repo.add("Malformed URL", &[]);
    let (_, malformed) = repo.json_with_env(
        &[
            "github",
            "adopt",
            malformed_task.as_str(),
            "https://github.com/acme/widget/pull/42",
            "--json",
        ],
        &[("ORBIT_GITHUB_FIXTURE", fixture.to_str().unwrap())],
    );
    assert_eq!(malformed["error"]["code"], "INVALID_ARGUMENT");
    assert!(
        malformed["error"]["message"]
            .as_str()
            .unwrap()
            .contains("GitHub Issue URL")
    );

    let mismatched_task = repo.add("Mismatched URL", &[]);
    let (_, mismatched) = repo.json_with_env(
        &[
            "github",
            "adopt",
            mismatched_task.as_str(),
            "https://github.com/acme/widget/issues/42",
            "--repo",
            "acme/other",
            "--number",
            "43",
            "--json",
        ],
        &[("ORBIT_GITHUB_FIXTURE", fixture.to_str().unwrap())],
    );
    assert_eq!(mismatched["error"]["code"], "INVALID_ARGUMENT");
    assert!(
        mismatched["error"]["message"]
            .as_str()
            .unwrap()
            .contains("does not match")
    );

    let mismatched_number_task = repo.add("Mismatched issue number", &[]);
    let (_, mismatched_number) = repo.json_with_env(
        &[
            "github",
            "adopt",
            mismatched_number_task.as_str(),
            "https://github.com/acme/widget/issues/42",
            "--number",
            "43",
            "--json",
        ],
        &[("ORBIT_GITHUB_FIXTURE", fixture.to_str().unwrap())],
    );
    assert_eq!(mismatched_number["error"]["code"], "INVALID_ARGUMENT");
    assert!(
        mismatched_number["error"]["message"]
            .as_str()
            .unwrap()
            .contains("issue number 43 does not match")
    );
}

#[test]
fn github_create_defaults_to_only_linked_repository() {
    let fixture_dir = tempfile::tempdir().unwrap();
    let fixture = write_fixture(&fixture_dir);
    let repo = TestRepo::new();
    repo.init();
    write_github_config_for_repositories(&repo, &["acme/widget"]);
    let id = repo.add("Create issue", &[]);

    let created = repo.ok_json_with_env(
        &["github", "create", id.as_str(), "--json"],
        &[("ORBIT_GITHUB_FIXTURE", fixture.to_str().unwrap())],
    );
    assert_eq!(created["data"]["github"]["repo"], "acme/widget");
}

#[test]
fn github_create_requires_repo_when_selection_is_ambiguous_or_missing() {
    for repositories in [&["acme/widget", "acme/other"][..], &[][..]] {
        let repo = TestRepo::new();
        repo.init();
        write_github_config_for_repositories(&repo, repositories);
        let id = repo.add("Create issue", &[]);

        let (_, error) = repo.json(&["github", "create", id.as_str(), "--json"]);
        assert_eq!(error["error"]["code"], "INVALID_ARGUMENT");
        assert!(
            error["error"]["message"]
                .as_str()
                .unwrap()
                .contains("--repo")
        );
    }

    let fixture_dir = tempfile::tempdir().unwrap();
    let fixture = write_fixture(&fixture_dir);
    let repo = TestRepo::new();
    repo.init();
    write_github_config_for_repositories(&repo, &["acme/widget", "acme/other"]);
    let id = repo.add("Explicit repository", &[]);
    let created = repo.ok_json_with_env(
        &[
            "github",
            "create",
            id.as_str(),
            "--repo",
            "acme/other",
            "--json",
        ],
        &[("ORBIT_GITHUB_FIXTURE", fixture.to_str().unwrap())],
    );
    assert_eq!(created["data"]["github"]["repo"], "acme/other");
}

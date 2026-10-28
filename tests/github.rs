mod common;

use common::TestRepo;
use std::fs;
use tempfile::TempDir;

fn write_github_config(repo: &TestRepo) {
    let path = repo.board_path().join("config.toml");
    let mut config = fs::read_to_string(&path).unwrap();
    config.push_str("\n[github]\norganization = \"acme\"\nrepositories = [\"acme/widget\"]\n");
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

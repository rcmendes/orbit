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

fn write_status_fixture(dir: &TempDir) -> std::path::PathBuf {
    let path = write_fixture(dir);
    let mut fixture: serde_json::Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
    fixture["repositories"] = serde_json::json!({
        "acme/widget": { "owner_type": "Organization", "accessible": true }
    });
    fixture["auth_message"] = serde_json::json!(
        "Logged in to github.com account agent (keyring)\nToken scopes: 'read:org', 'repo'"
    );
    fs::write(&path, serde_json::to_vec_pretty(&fixture).unwrap()).unwrap();
    path
}

#[test]
fn unlinked_repository_errors_include_safe_remediation_in_json_and_human_output() {
    let repo = TestRepo::new();
    repo.init();
    write_github_config(&repo);
    let id = repo.add("Create issue", &[]);

    let (_, json_error) = repo.json(&[
        "github",
        "create",
        id.as_str(),
        "--repo",
        "acme/private",
        "--json",
    ]);
    assert_eq!(json_error["error"]["code"], "GITHUB_REPO_NOT_LINKED");
    assert_eq!(json_error["error"]["details"]["repository"], "acme/private");
    let remediation = json_error["error"]["details"]["remediation"]
        .as_str()
        .expect("JSON error should include remediation");
    assert!(remediation.contains("orbit github repo link acme/private"));
    assert!(
        json_error["error"]["message"]
            .as_str()
            .unwrap()
            .contains(remediation)
    );

    let human = repo.orbit(&["github", "create", id.as_str(), "--repo", "acme/private"]);
    assert!(!human.status.success());
    let human = String::from_utf8_lossy(&human.stderr);
    assert!(human.contains("acme/private"));
    assert!(human.contains(remediation));
}

#[test]
fn github_not_configured_errors_include_login_and_connect_actions() {
    let repo = TestRepo::new();
    repo.init();
    write_github_config(&repo);
    repo.fake_command("gh");
    let gh = repo.bin().join("gh");
    let secret = "ghp_exampleSecretValue123456789";
    fs::write(
        &gh,
        format!("#!/bin/sh\necho 'GitHub API unavailable: {secret}' >&2\nexit 1\n"),
    )
    .unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&gh, fs::Permissions::from_mode(0o755)).unwrap();
    }

    let (status, value) = repo.json(&["github", "setup", "--validate-only", "--json"]);
    assert_eq!(status, 1);
    assert_eq!(value["error"]["code"], "GITHUB_NOT_CONFIGURED");
    let remediation = value["error"]["details"]["remediation"]
        .as_str()
        .expect("JSON error should include remediation");
    assert!(remediation.contains("orbit github connect"));
    assert!(remediation.contains("gh auth login"));
    let cause = value["error"]["details"]["cause"].as_str().unwrap();
    assert!(
        cause.contains("GitHub API unavailable") || cause.contains("failed to run `gh`"),
        "unexpected cause: {cause}"
    );
    assert!(!value.to_string().contains(secret));

    let human = repo.orbit(&["github", "setup", "--validate-only"]);
    assert!(!human.status.success());
    assert!(String::from_utf8_lossy(&human.stderr).contains(remediation));
}

#[test]
fn github_setup_incomplete_errors_name_the_missing_field_and_resolution_action() {
    let fixture_dir = tempfile::tempdir().unwrap();
    let fixture = fixture_dir.path().join("empty-fields.json");
    fs::write(&fixture, r#"{"org_fields":{},"issues":{}}"#).unwrap();
    let repo = TestRepo::new();
    repo.init();
    write_github_config(&repo);

    let (status, value) = repo.json_with_env(
        &["github", "setup", "--json"],
        &[("ORBIT_GITHUB_FIXTURE", fixture.to_str().unwrap())],
    );
    assert_eq!(status, 1);
    assert_eq!(value["error"]["code"], "GITHUB_SETUP_INCOMPLETE");
    assert_eq!(value["error"]["details"]["organization"], "acme");
    assert_eq!(value["error"]["details"]["fields"][0], "Area");
    let remediation = value["error"]["details"]["remediation"]
        .as_str()
        .expect("JSON error should include remediation");
    assert!(remediation.contains("orbit github setup --create-fields"));
    assert!(
        value["error"]["details"]["cause"]
            .as_str()
            .unwrap()
            .contains("Issue Field `Area`")
    );

    let human = repo.orbit_with_env(
        &["github", "setup"],
        &[("ORBIT_GITHUB_FIXTURE", fixture.to_str().unwrap())],
    );
    assert!(!human.status.success());
    assert!(String::from_utf8_lossy(&human.stderr).contains(remediation));
}

fn set_configured_issue_field_ids(repo: &TestRepo) {
    let path = repo.board_path().join("config.toml");
    let mut config: toml::Value = toml::from_str(&fs::read_to_string(&path).unwrap()).unwrap();
    let github = config
        .as_table_mut()
        .unwrap()
        .entry("github")
        .or_insert_with(|| toml::Value::Table(Default::default()));
    let issue_fields = github
        .as_table_mut()
        .unwrap()
        .entry("issue_fields")
        .or_insert_with(|| toml::Value::Table(Default::default()));
    let issue_fields = issue_fields.as_table_mut().unwrap();
    issue_fields.insert("area_id".into(), toml::Value::String("AREA_ID".into()));
    issue_fields.insert(
        "verification_id".into(),
        toml::Value::String("VERIFICATION_ID".into()),
    );
    fs::write(path, toml::to_string(&config).unwrap()).unwrap();
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

fn add_fixture_issue(fixture: &std::path::Path, identity: &str, number: u64, title: &str) {
    let mut data: serde_json::Value = serde_json::from_slice(&fs::read(fixture).unwrap()).unwrap();
    let mut issue = data["issues"]["acme/widget#42"].clone();
    issue["number"] = serde_json::json!(number);
    issue["title"] = serde_json::json!(title);
    data["issues"][identity] = issue;
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
fn list_shows_and_filters_github_links() {
    let fixture_dir = tempfile::tempdir().unwrap();
    let fixture = write_fixture(&fixture_dir);
    let repo = TestRepo::new();
    let linked = adopt_fixture_task(&repo, &fixture, "Linked task");
    let local = repo.add("Local task", &[]);

    let all = repo.ok_json(&["list", "--json"]);
    let tasks = all["data"]["tasks"].as_array().unwrap();
    let linked_row = tasks.iter().find(|task| task["id"] == linked).unwrap();
    assert_eq!(linked_row["github"]["number"], 42);
    let local_row = tasks.iter().find(|task| task["id"] == local).unwrap();
    assert!(local_row["github"].is_null());

    let only_linked = repo.ok_json(&["list", "--linked", "--json"]);
    assert_eq!(only_linked["data"]["count"], 1);
    assert_eq!(only_linked["data"]["tasks"][0]["id"], linked);

    let only_local = repo.ok_json(&["list", "--local", "--json"]);
    assert_eq!(only_local["data"]["count"], 1);
    assert_eq!(only_local["data"]["tasks"][0]["id"], local);

    let human = repo.orbit(&["list"]);
    assert!(human.status.success());
    let stdout = String::from_utf8_lossy(&human.stdout);
    assert!(stdout.contains("ISSUE"), "{stdout}");
    let linked_line = stdout.lines().find(|line| line.contains(&linked)).unwrap();
    assert!(linked_line.contains("#42"), "{stdout}");
    let local_line = stdout.lines().find(|line| line.contains(&local)).unwrap();
    assert!(local_line.contains(" - "), "{stdout}");

    let show = repo.orbit(&["show", linked.as_str()]);
    assert!(show.status.success());
    assert!(
        String::from_utf8_lossy(&show.stdout).contains("github: acme/widget#42"),
        "{}",
        String::from_utf8_lossy(&show.stdout)
    );
    let show_local = repo.orbit(&["show", local.as_str()]);
    assert!(show_local.status.success());
    assert!(
        String::from_utf8_lossy(&show_local.stdout).contains("github: -"),
        "{}",
        String::from_utf8_lossy(&show_local.stdout)
    );

    let conflict = repo.orbit(&["list", "--linked", "--local", "--json"]);
    assert_eq!(conflict.status.code(), Some(2));
    let stderr = String::from_utf8_lossy(&conflict.stderr);
    assert!(stderr.contains("cannot be used with"), "{stderr}");
}

#[test]
fn one_github_issue_cannot_be_adopted_by_two_tasks() {
    let fixture_dir = tempfile::tempdir().unwrap();
    let fixture = write_fixture(&fixture_dir);
    let repo = TestRepo::new();
    repo.init();
    write_github_config(&repo);
    let first = repo.add("First task", &[]);
    let second = repo.add("Second task", &[]);
    for id in [&first, &second] {
        if id == &first {
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
        } else {
            let (status, error) = repo.json_with_env(
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
            assert_eq!(status, 1);
            assert!(
                error["error"]["message"]
                    .as_str()
                    .unwrap()
                    .contains("already linked")
            );
        }
    }
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
fn github_setup_create_fields_creates_required_fields_and_reuses_them() {
    let fixture_dir = tempfile::tempdir().unwrap();
    let fixture = fixture_dir.path().join("setup.json");
    fs::write(
        &fixture,
        r#"{
  "organization_admin": true,
  "org_fields": {},
  "issues": {}
}"#,
    )
    .unwrap();
    let repo = TestRepo::new();
    repo.init();
    write_github_config(&repo);

    let setup = repo.ok_json_with_env(
        &["github", "setup", "--create-fields", "--json"],
        &[("ORBIT_GITHUB_FIXTURE", fixture.to_str().unwrap())],
    );
    assert_eq!(setup["data"]["area_id"], "CREATED_AREA_ID");
    assert_eq!(setup["data"]["verification_id"], "CREATED_VERIFICATION_ID");

    let created: serde_json::Value = serde_json::from_slice(&fs::read(&fixture).unwrap()).unwrap();
    assert_eq!(created["created_fields"].as_array().unwrap().len(), 2);
    assert_eq!(
        created["created_fields"][0]["options"],
        serde_json::json!(["board", "cli", "mcp", "setup", "docs", "release"])
    );
    assert_eq!(created["created_fields"][0]["data_type"], "SINGLE_SELECT");
    assert_eq!(created["created_fields"][1]["data_type"], "TEXT");

    let again = repo.ok_json_with_env(
        &["github", "setup", "--create-fields", "--json"],
        &[("ORBIT_GITHUB_FIXTURE", fixture.to_str().unwrap())],
    );
    assert_eq!(again["data"]["area_id"], "CREATED_AREA_ID");
    assert_eq!(again["data"]["verification_id"], "CREATED_VERIFICATION_ID");
    let reused: serde_json::Value = serde_json::from_slice(&fs::read(&fixture).unwrap()).unwrap();
    assert_eq!(reused["created_fields"].as_array().unwrap().len(), 2);

    let config = fs::read_to_string(repo.board_path().join("config.toml")).unwrap();
    assert!(config.contains("area_id = \"CREATED_AREA_ID\""));
    assert!(config.contains("verification_id = \"CREATED_VERIFICATION_ID\""));
}

#[test]
fn github_setup_create_fields_reuses_existing_fields_without_overwriting_them() {
    let fixture_dir = tempfile::tempdir().unwrap();
    let fixture = fixture_dir.path().join("setup.json");
    fs::write(
        &fixture,
        r#"{
  "organization_admin": true,
  "org_fields": { "acme/Area": "EXISTING_AREA_ID" },
  "issues": {}
}"#,
    )
    .unwrap();
    let repo = TestRepo::new();
    repo.init();
    write_github_config(&repo);

    let setup = repo.ok_json_with_env(
        &["github", "setup", "--create-fields", "--json"],
        &[("ORBIT_GITHUB_FIXTURE", fixture.to_str().unwrap())],
    );
    assert_eq!(setup["data"]["area_id"], "EXISTING_AREA_ID");
    assert_eq!(setup["data"]["verification_id"], "CREATED_VERIFICATION_ID");

    let fields: serde_json::Value = serde_json::from_slice(&fs::read(&fixture).unwrap()).unwrap();
    assert_eq!(fields["org_fields"]["acme/Area"], "EXISTING_AREA_ID");
    assert_eq!(fields["created_fields"].as_array().unwrap().len(), 1);
    assert_eq!(fields["created_fields"][0]["name"], "Verification");
}

#[test]
fn github_setup_create_fields_requires_organization_administration() {
    let fixture_dir = tempfile::tempdir().unwrap();
    let fixture = fixture_dir.path().join("setup.json");
    fs::write(
        &fixture,
        r#"{
  "organization_admin": false,
  "org_fields": {},
  "issues": {}
}"#,
    )
    .unwrap();
    let repo = TestRepo::new();
    repo.init();
    write_github_config(&repo);

    let (status, result) = repo.json_with_env(
        &["github", "setup", "--create-fields", "--json"],
        &[("ORBIT_GITHUB_FIXTURE", fixture.to_str().unwrap())],
    );
    assert_ne!(status, 0);
    assert_eq!(result["error"]["code"], "GITHUB_SETUP_INCOMPLETE");
    assert!(
        result["error"]["message"]
            .as_str()
            .unwrap()
            .contains("organization administrator")
    );
    assert!(
        result["error"]["message"]
            .as_str()
            .unwrap()
            .contains("create the fields in GitHub")
    );
    let unchanged: serde_json::Value =
        serde_json::from_slice(&fs::read(&fixture).unwrap()).unwrap();
    assert!(unchanged["created_fields"].is_null());
}

#[test]
fn github_setup_without_create_fields_only_resolves_existing_fields() {
    let fixture_dir = tempfile::tempdir().unwrap();
    let fixture = fixture_dir.path().join("setup.json");
    fs::write(
        &fixture,
        r#"{
  "organization_admin": false,
  "org_fields": {
    "acme/Area": "AREA_ID",
    "acme/Verification": "VERIFICATION_ID"
  },
  "issues": {}
}"#,
    )
    .unwrap();
    let repo = TestRepo::new();
    repo.init();
    write_github_config(&repo);

    let setup = repo.ok_json_with_env(
        &["github", "setup", "--json"],
        &[("ORBIT_GITHUB_FIXTURE", fixture.to_str().unwrap())],
    );
    assert_eq!(setup["data"]["area_id"], "AREA_ID");
    assert_eq!(setup["data"]["verification_id"], "VERIFICATION_ID");
    let config = fs::read_to_string(repo.board_path().join("config.toml")).unwrap();
    assert!(config.contains("area_id = \"AREA_ID\""));
    assert!(config.contains("verification_id = \"VERIFICATION_ID\""));
    let unchanged: serde_json::Value =
        serde_json::from_slice(&fs::read(&fixture).unwrap()).unwrap();
    assert!(unchanged["created_fields"].is_null());
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
            "--apply",
            "--json",
        ],
        &[("ORBIT_GITHUB_FIXTURE", fixture.to_str().unwrap())],
    );
    assert_eq!(status, 0);
    assert_eq!(value["data"]["failure_count"], 1);
    assert_eq!(value["data"]["tasks"][0]["success"], false);
    assert!(
        value["data"]["tasks"][0]["error"]
            .as_str()
            .unwrap()
            .contains("area")
    );
    assert!(
        value["data"]["tasks"][0]["error"]
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
            "--apply",
            "--json",
        ],
        &[("ORBIT_GITHUB_FIXTURE", fixture.to_str().unwrap())],
    );
    assert_eq!(
        created["data"]["tasks"][0]["issue"]["title"],
        "Personal issue"
    );
    assert_eq!(
        created["data"]["tasks"][0]["issue"]["labels"],
        serde_json::json!(["bug"])
    );
    assert!(created["data"]["tasks"][0]["issue"]["area"].is_null());
    assert!(created["data"]["tasks"][0]["issue"]["verification"].is_null());
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
fn github_connect_configures_organization_and_is_idempotent() {
    let fixture_dir = tempfile::tempdir().unwrap();
    let fixture = fixture_dir.path().join("connect.json");
    fs::write(
        &fixture,
        r#"{
  "authenticated": true,
  "repositories": {
    "acme/widget": { "owner_type": "Organization", "accessible": true }
  },
  "org_fields": {
    "acme/Area": "AREA_ID",
    "acme/Verification": "VERIFICATION_ID"
  },
  "issues": {}
}"#,
    )
    .unwrap();
    let repo = TestRepo::new();
    repo.init();
    let config_path = repo.board_path().join("config.toml");
    let config = fs::read_to_string(&config_path).unwrap().replace(
        "[github.issue_fields]",
        "[github]\norganization = \"acme\"\nrepositories = [\"acme/other\"]\n\
[github.issue_fields]\narea_id = \"AREA_ID\"\nverification_id = \"VERIFICATION_ID\"",
    );
    fs::write(&config_path, config).unwrap();
    repo.git(&[
        "remote",
        "add",
        "origin",
        "https://github.com/acme/widget.git",
    ]);

    let first = repo.ok_json_with_env(
        &["github", "connect", "--json"],
        &[("ORBIT_GITHUB_FIXTURE", fixture.to_str().unwrap())],
    );
    assert_eq!(first["data"]["owner_type"], "organization");
    assert_eq!(first["data"]["organization"], "acme");
    assert_eq!(first["data"]["repository"], "acme/widget");
    assert_eq!(first["data"]["field_status"]["area"], "ready");
    assert_eq!(first["data"]["field_status"]["verification"], "ready");
    assert_eq!(first["data"]["next_action"], "connected");
    assert_eq!(first["data"]["changed"], true);

    let config = fs::read_to_string(&config_path).unwrap();
    assert!(config.contains("organization = \"acme\""));
    assert!(config.contains("\"acme/other\""));
    assert!(config.contains("\"acme/widget\""));
    assert_eq!(config.matches("\"acme/widget\"").count(), 1);
    assert!(config.contains("area_id = \"AREA_ID\""));
    assert!(config.contains("verification_id = \"VERIFICATION_ID\""));

    let second = repo.ok_json_with_env(
        &["github", "connect", "--json"],
        &[("ORBIT_GITHUB_FIXTURE", fixture.to_str().unwrap())],
    );
    assert_eq!(second["data"]["changed"], false);

    let human = repo.orbit_with_env(
        &["github", "connect"],
        &[("ORBIT_GITHUB_FIXTURE", fixture.to_str().unwrap())],
    );
    assert!(human.status.success());
    assert!(String::from_utf8_lossy(&human.stdout).contains("Configuration was already current"));
}

#[test]
fn github_connect_personal_owner_reports_choices_without_changing_config() {
    let fixture_dir = tempfile::tempdir().unwrap();
    let fixture = fixture_dir.path().join("connect.json");
    fs::write(
        &fixture,
        r#"{
  "authenticated": true,
  "repositories": {
    "riley/widget": { "owner_type": "User", "accessible": true }
  },
  "org_fields": {},
  "issues": {}
}"#,
    )
    .unwrap();
    let repo = TestRepo::new();
    repo.init();
    repo.git(&["remote", "add", "origin", "git@github.com:riley/widget.git"]);

    let result = repo.ok_json_with_env(
        &["github", "connect", "--json"],
        &[("ORBIT_GITHUB_FIXTURE", fixture.to_str().unwrap())],
    );
    assert_eq!(result["data"]["status"], "blocked");
    assert_eq!(result["data"]["owner_type"], "user");
    assert_eq!(result["data"]["repository"], "riley/widget");
    assert_eq!(
        result["data"]["next_action"],
        "choose_personal_account_path"
    );
    let remediation = result["data"]["remediation"]
        .as_str()
        .expect("personal-account guidance should include remediation");
    assert!(remediation.contains("config.toml"));
    assert!(remediation.contains("preserve any other linked repositories"));
    assert!(remediation.contains("mode = \"basic\""));
    assert!(remediation.contains("repositories = [\"riley/widget\"]"));
    assert!(remediation.contains("orbit github status"));
    assert!(remediation.contains("docs/github-sync.md#basic-mode-personal-repositories"));
    assert!(!remediation.contains("orbit github basic"));
    let next_actions = result["data"]["next_actions"].as_array().unwrap();
    assert_eq!(next_actions.len(), 3);
    assert!(next_actions[1].as_str().unwrap().contains(remediation));
    let config = fs::read_to_string(repo.board_path().join("config.toml")).unwrap();
    assert!(!config.contains("[github]"));

    let human = repo.orbit_with_env(
        &["github", "connect"],
        &[("ORBIT_GITHUB_FIXTURE", fixture.to_str().unwrap())],
    );
    assert!(human.status.success());
    let human = String::from_utf8_lossy(&human.stdout);
    assert!(human.contains(remediation));
    assert!(!human.contains("orbit github basic"));
}

#[test]
fn github_connect_reports_authentication_access_and_field_blockers() {
    let fixture_dir = tempfile::tempdir().unwrap();
    let fixture = fixture_dir.path().join("connect.json");
    let repo = TestRepo::new();
    repo.init();
    repo.git(&[
        "remote",
        "add",
        "origin",
        "https://github.com/acme/widget.git",
    ]);

    fs::write(
        &fixture,
        r#"{
  "authenticated": false,
  "auth_message": "not logged in",
  "repositories": {},
  "org_fields": {},
  "issues": {}
}"#,
    )
    .unwrap();
    let auth = repo.ok_json_with_env(
        &["github", "connect", "--json"],
        &[("ORBIT_GITHUB_FIXTURE", fixture.to_str().unwrap())],
    );
    assert_eq!(auth["data"]["next_action"], "authenticate_gh");
    assert!(
        auth["data"]["remediation"]
            .as_str()
            .unwrap()
            .contains("gh auth login")
    );

    fs::write(
        &fixture,
        r#"{
  "authenticated": true,
  "repositories": {
    "acme/widget": { "owner_type": "Organization", "accessible": false }
  },
  "org_fields": {},
  "issues": {}
}"#,
    )
    .unwrap();
    let access = repo.ok_json_with_env(
        &["github", "connect", "--json"],
        &[("ORBIT_GITHUB_FIXTURE", fixture.to_str().unwrap())],
    );
    assert_eq!(access["data"]["next_action"], "grant_repository_access");
    assert!(
        access["data"]["remediation"]
            .as_str()
            .unwrap()
            .contains("access")
    );

    fs::write(
        &fixture,
        r#"{
  "authenticated": true,
  "repositories": {
    "acme/widget": { "owner_type": "Organization", "accessible": true }
  },
  "org_fields": { "acme/Area": "AREA_ID" },
  "issues": {}
}"#,
    )
    .unwrap();
    let fields = repo.ok_json_with_env(
        &["github", "connect", "--json"],
        &[("ORBIT_GITHUB_FIXTURE", fixture.to_str().unwrap())],
    );
    assert_eq!(fields["data"]["field_status"]["area"], "ready");
    assert_eq!(fields["data"]["field_status"]["verification"], "missing");
    assert_eq!(fields["data"]["next_action"], "create_issue_fields");
    assert!(
        fields["data"]["remediation"]
            .as_str()
            .unwrap()
            .contains("Verification")
    );
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
fn orbit_title_edits_are_previewed_as_github_pushes() {
    let fixture_dir = tempfile::tempdir().unwrap();
    let fixture = write_fixture(&fixture_dir);
    let repo = TestRepo::new();
    let id = adopt_fixture_task(&repo, &fixture, "Local title");
    set_task_frontmatter(&repo, &id, "title", "\"Orbit-owned title\"");
    set_task_frontmatter(&repo, &id, "updated_at", "2026-10-24T14:00:00Z");

    let preview = repo.ok_json_with_env(
        &["github", "sync", id.as_str(), "--json"],
        &[("ORBIT_GITHUB_FIXTURE", fixture.to_str().unwrap())],
    );

    assert_eq!(
        preview["data"]["tasks"][0]["plan"]["push_title"],
        "Orbit-owned title"
    );
    assert!(preview["data"]["tasks"][0]["plan"]["apply_title"].is_null());
    let unchanged: serde_json::Value =
        serde_json::from_slice(&fs::read(&fixture).unwrap()).unwrap();
    assert_eq!(
        unchanged["issues"]["acme/widget#42"]["title"],
        "Imported issue"
    );
}

#[test]
fn github_sync_treats_reordered_labels_as_unchanged() {
    let fixture_dir = tempfile::tempdir().unwrap();
    let fixture = write_fixture(&fixture_dir);
    let repo = TestRepo::new();
    let id = adopt_fixture_task(&repo, &fixture, "Reordered labels");
    // GitHub returns labels in its own order; Orbit keeps them sorted.
    set_fixture_issue_field(
        &fixture,
        "labels",
        serde_json::json!(["zeta", "beta", "alpha", "bug"]),
    );
    repo.ok_json(&["label", "add", id.as_str(), "zeta", "--json"]);
    repo.ok_json(&["label", "add", id.as_str(), "alpha", "--json"]);
    repo.ok_json(&["label", "add", id.as_str(), "beta", "--json"]);

    let preview = repo.ok_json_with_env(
        &["github", "sync", id.as_str(), "--json"],
        &[("ORBIT_GITHUB_FIXTURE", fixture.to_str().unwrap())],
    );
    assert!(
        preview["data"]["tasks"][0]["plan"]["push_labels"].is_null(),
        "{preview}"
    );
    assert!(preview["data"]["tasks"][0]["plan"]["apply_labels"].is_null());

    repo.ok_json_with_env(
        &["github", "sync", id.as_str(), "--apply", "--json"],
        &[("ORBIT_GITHUB_FIXTURE", fixture.to_str().unwrap())],
    );
    let again = repo.ok_json_with_env(
        &["github", "sync", id.as_str(), "--json"],
        &[("ORBIT_GITHUB_FIXTURE", fixture.to_str().unwrap())],
    );
    assert!(
        again["data"]["tasks"][0]["plan"]["push_labels"].is_null(),
        "{again}"
    );
}

#[test]
fn linked_task_status_labels_and_metadata_push_from_orbit() {
    let fixture_dir = tempfile::tempdir().unwrap();
    let fixture = write_fixture(&fixture_dir);
    let repo = TestRepo::new();
    let id = adopt_fixture_task(&repo, &fixture, "Local title");
    set_task_frontmatter(&repo, &id, "status", "done");
    repo.ok_json(&["label", "remove", id.as_str(), "bug", "--json"]);
    repo.ok_json(&["label", "add", id.as_str(), "feature", "--json"]);
    set_task_frontmatter(&repo, &id, "area", "setup");
    set_task_frontmatter(&repo, &id, "verification", "\"cargo test sync\"");
    set_task_frontmatter(&repo, &id, "updated_at", "2026-10-25T14:00:00Z");

    let preview = repo.ok_json_with_env(
        &["github", "sync", id.as_str(), "--json"],
        &[("ORBIT_GITHUB_FIXTURE", fixture.to_str().unwrap())],
    );
    let plan = &preview["data"]["tasks"][0]["plan"];
    assert_eq!(plan["push_state"], "closed");
    assert_eq!(plan["push_labels"], serde_json::json!(["feature"]));
    assert_eq!(plan["push_area"], serde_json::json!("setup"));
    assert_eq!(
        plan["push_verification"],
        serde_json::json!("cargo test sync")
    );

    repo.ok_json_with_env(
        &["github", "sync", id.as_str(), "--apply", "--json"],
        &[("ORBIT_GITHUB_FIXTURE", fixture.to_str().unwrap())],
    );
    let applied: serde_json::Value = serde_json::from_slice(&fs::read(&fixture).unwrap()).unwrap();
    assert_eq!(applied["issues"]["acme/widget#42"]["state"], "closed");
    assert_eq!(
        applied["issues"]["acme/widget#42"]["labels"],
        serde_json::json!(["feature"])
    );
    assert_eq!(applied["issues"]["acme/widget#42"]["area"], "setup");
    assert_eq!(
        applied["issues"]["acme/widget#42"]["verification"],
        "cargo test sync"
    );
    let show = repo.ok_json(&["show", id.as_str(), "--json"]);
    assert_eq!(show["data"]["github"]["last_sync"]["status"], "done");
    assert_eq!(show["data"]["github"]["last_sync"]["issue_state"], "closed");
}

#[test]
fn remote_only_label_and_metadata_changes_pull_when_orbit_is_unchanged() {
    let fixture_dir = tempfile::tempdir().unwrap();
    let fixture = write_fixture(&fixture_dir);
    let repo = TestRepo::new();
    let id = adopt_fixture_task(&repo, &fixture, "Local title");
    set_fixture_issue_field(&fixture, "labels", serde_json::json!(["remote-label"]));
    set_fixture_issue_field(&fixture, "area", serde_json::json!("setup"));
    set_fixture_issue_field(
        &fixture,
        "verification",
        serde_json::json!("cargo test remote"),
    );
    set_fixture_issue_field(
        &fixture,
        "updated_at",
        serde_json::json!("2026-10-25T13:00:00Z"),
    );

    let preview = repo.ok_json_with_env(
        &["github", "sync", id.as_str(), "--json"],
        &[("ORBIT_GITHUB_FIXTURE", fixture.to_str().unwrap())],
    );
    let plan = &preview["data"]["tasks"][0]["plan"];
    assert_eq!(plan["apply_labels"], serde_json::json!(["remote-label"]));
    assert_eq!(plan["apply_area"], serde_json::json!("setup"));
    assert_eq!(
        plan["apply_verification"],
        serde_json::json!("cargo test remote")
    );

    repo.ok_json_with_env(
        &["github", "sync", id.as_str(), "--apply", "--json"],
        &[("ORBIT_GITHUB_FIXTURE", fixture.to_str().unwrap())],
    );
    let show = repo.ok_json(&["show", id.as_str(), "--json"]);
    assert_eq!(show["data"]["labels"], serde_json::json!(["remote-label"]));
    assert_eq!(show["data"]["area"], "setup");
    assert_eq!(show["data"]["verification"], "cargo test remote");
}

#[test]
fn remote_issue_close_pulls_a_terminal_orbit_status() {
    let fixture_dir = tempfile::tempdir().unwrap();
    let fixture = write_fixture(&fixture_dir);
    let repo = TestRepo::new();
    let id = adopt_fixture_task(&repo, &fixture, "Local title");
    set_fixture_issue_field(&fixture, "state", serde_json::json!("closed"));
    set_fixture_issue_field(
        &fixture,
        "updated_at",
        serde_json::json!("2026-10-25T13:00:00Z"),
    );

    let preview = repo.ok_json_with_env(
        &["github", "sync", id.as_str(), "--json"],
        &[("ORBIT_GITHUB_FIXTURE", fixture.to_str().unwrap())],
    );
    assert_eq!(preview["data"]["tasks"][0]["plan"]["apply_status"], "done");
    repo.ok_json_with_env(
        &["github", "sync", id.as_str(), "--apply", "--json"],
        &[("ORBIT_GITHUB_FIXTURE", fixture.to_str().unwrap())],
    );
    let show = repo.ok_json(&["show", id.as_str(), "--json"]);
    assert_eq!(show["data"]["status"], "done");
    assert_eq!(show["data"]["github"]["last_sync"]["issue_state"], "closed");
}

#[test]
fn supported_subtask_relationships_are_previewed_and_applied() {
    let fixture_dir = tempfile::tempdir().unwrap();
    let fixture = write_fixture(&fixture_dir);
    add_fixture_issue(&fixture, "acme/widget#43", 43, "Child issue");
    let repo = TestRepo::new();
    let parent = adopt_fixture_task(&repo, &fixture, "Parent issue");
    let child = repo.add("Child work", &["--parent", parent.as_str()]);
    repo.ok_json_with_env(
        &[
            "github",
            "adopt",
            child.as_str(),
            "--repo",
            "acme/widget",
            "--number",
            "43",
            "--json",
        ],
        &[("ORBIT_GITHUB_FIXTURE", fixture.to_str().unwrap())],
    );

    let preview = repo.ok_json_with_env(
        &["github", "sync", parent.as_str(), "--json"],
        &[("ORBIT_GITHUB_FIXTURE", fixture.to_str().unwrap())],
    );
    assert_eq!(
        preview["data"]["tasks"][0]["relationships"]["supported"][0]["kind"],
        "sub_issue"
    );
    assert_eq!(
        preview["data"]["tasks"][0]["relationships"]["changes"][0]["action"],
        "add"
    );

    repo.ok_json_with_env(
        &["github", "sync", parent.as_str(), "--apply", "--json"],
        &[("ORBIT_GITHUB_FIXTURE", fixture.to_str().unwrap())],
    );
    let applied: serde_json::Value = serde_json::from_slice(&fs::read(&fixture).unwrap()).unwrap();
    assert_eq!(
        applied["issues"]["acme/widget#42"]["blocking"][0]["number"],
        43
    );
}

#[test]
fn supported_blocked_by_relationships_are_previewed_and_applied() {
    let fixture_dir = tempfile::tempdir().unwrap();
    let fixture = write_fixture(&fixture_dir);
    add_fixture_issue(&fixture, "acme/widget#43", 43, "Blocking dependency");
    let repo = TestRepo::new();
    let task = adopt_fixture_task(&repo, &fixture, "Blocked issue");
    let dependency = repo.add("Dependency", &[]);
    repo.ok_json_with_env(
        &[
            "github",
            "adopt",
            dependency.as_str(),
            "--repo",
            "acme/widget",
            "--number",
            "43",
            "--json",
        ],
        &[("ORBIT_GITHUB_FIXTURE", fixture.to_str().unwrap())],
    );
    repo.ok_json(&["depend", task.as_str(), dependency.as_str(), "--json"]);

    let preview = repo.ok_json_with_env(
        &["github", "sync", task.as_str(), "--json"],
        &[("ORBIT_GITHUB_FIXTURE", fixture.to_str().unwrap())],
    );
    assert_eq!(
        preview["data"]["tasks"][0]["relationships"]["supported"][0]["kind"],
        "blocked_by"
    );
    repo.ok_json_with_env(
        &["github", "sync", task.as_str(), "--apply", "--json"],
        &[("ORBIT_GITHUB_FIXTURE", fixture.to_str().unwrap())],
    );
    let applied: serde_json::Value = serde_json::from_slice(&fs::read(&fixture).unwrap()).unwrap();
    assert_eq!(
        applied["issues"]["acme/widget#42"]["blocked_by"][0]["number"],
        43
    );
}

#[test]
fn removing_a_synced_dependency_removes_the_github_blocked_by_relationship() {
    let fixture_dir = tempfile::tempdir().unwrap();
    let fixture = write_fixture(&fixture_dir);
    add_fixture_issue(&fixture, "acme/widget#43", 43, "Blocking dependency");
    let repo = TestRepo::new();
    let task = adopt_fixture_task(&repo, &fixture, "Blocked issue");
    let dependency = repo.add("Dependency", &[]);
    repo.ok_json_with_env(
        &[
            "github",
            "adopt",
            dependency.as_str(),
            "--repo",
            "acme/widget",
            "--number",
            "43",
            "--json",
        ],
        &[("ORBIT_GITHUB_FIXTURE", fixture.to_str().unwrap())],
    );
    repo.ok_json(&["depend", task.as_str(), dependency.as_str(), "--json"]);
    repo.ok_json_with_env(
        &["github", "sync", task.as_str(), "--apply", "--json"],
        &[("ORBIT_GITHUB_FIXTURE", fixture.to_str().unwrap())],
    );
    repo.ok_json(&["undepend", task.as_str(), dependency.as_str(), "--json"]);

    let preview = repo.ok_json_with_env(
        &["github", "sync", task.as_str(), "--json"],
        &[("ORBIT_GITHUB_FIXTURE", fixture.to_str().unwrap())],
    );
    assert!(
        preview["data"]["tasks"][0]["relationships"]["changes"]
            .as_array()
            .unwrap()
            .iter()
            .any(|change| change["action"] == "remove" && change["kind"] == "blocked_by"),
        "{preview}"
    );

    repo.ok_json_with_env(
        &["github", "sync", task.as_str(), "--apply", "--json"],
        &[("ORBIT_GITHUB_FIXTURE", fixture.to_str().unwrap())],
    );
    let applied: serde_json::Value = serde_json::from_slice(&fs::read(&fixture).unwrap()).unwrap();
    assert!(
        applied["issues"]["acme/widget#42"]
            .get("blocked_by")
            .is_none_or(|value| value.as_array().is_some_and(Vec::is_empty)),
        "{applied}"
    );
}

#[test]
fn batch_sync_reports_partial_failures_and_retries_without_advancing_failed_baseline() {
    let fixture_dir = tempfile::tempdir().unwrap();
    let fixture = write_fixture(&fixture_dir);
    add_fixture_issue(&fixture, "acme/widget#43", 43, "Second issue");
    let repo = TestRepo::new();
    let first = adopt_fixture_task(&repo, &fixture, "First issue");
    let second = repo.add("Second task", &[]);
    repo.ok_json_with_env(
        &[
            "github",
            "adopt",
            second.as_str(),
            "--repo",
            "acme/widget",
            "--number",
            "43",
            "--json",
        ],
        &[("ORBIT_GITHUB_FIXTURE", fixture.to_str().unwrap())],
    );
    set_task_frontmatter(&repo, &first, "title", "\"First Orbit edit\"");
    set_task_frontmatter(&repo, &second, "title", "\"Second Orbit edit\"");
    let second_before = repo.ok_json(&["show", second.as_str(), "--json"]);
    let mut data: serde_json::Value = serde_json::from_slice(&fs::read(&fixture).unwrap()).unwrap();
    data["update_failures"] = serde_json::json!({ "acme/widget#43": "temporary fixture failure" });
    fs::write(&fixture, serde_json::to_vec_pretty(&data).unwrap()).unwrap();

    let first_apply = repo.ok_json_with_env(
        &["github", "sync", "--apply", "--json"],
        &[("ORBIT_GITHUB_FIXTURE", fixture.to_str().unwrap())],
    );
    assert_eq!(first_apply["data"]["success_count"], 1);
    assert_eq!(first_apply["data"]["failure_count"], 1);
    assert_eq!(first_apply["data"]["failures"][0]["task"], second);
    let after_partial: serde_json::Value =
        serde_json::from_slice(&fs::read(&fixture).unwrap()).unwrap();
    assert_eq!(
        after_partial["issues"]["acme/widget#42"]["title"],
        "First Orbit edit"
    );
    assert_eq!(
        after_partial["issues"]["acme/widget#43"]["title"],
        "Second issue"
    );
    let second_after = repo.ok_json(&["show", second.as_str(), "--json"]);
    assert_eq!(
        second_after["data"]["github"]["last_sync"]["title"],
        second_before["data"]["github"]["last_sync"]["title"]
    );

    data["update_failures"] = serde_json::json!({});
    fs::write(&fixture, serde_json::to_vec_pretty(&data).unwrap()).unwrap();
    let retry = repo.ok_json_with_env(
        &["github", "sync", "--apply", "--json"],
        &[("ORBIT_GITHUB_FIXTURE", fixture.to_str().unwrap())],
    );
    assert_eq!(retry["data"]["failure_count"], 0);
    let after_retry: serde_json::Value =
        serde_json::from_slice(&fs::read(&fixture).unwrap()).unwrap();
    assert_eq!(
        after_retry["issues"]["acme/widget#43"]["title"],
        "Second Orbit edit"
    );
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

    let (status, error) = repo.json_with_env(
        &["github", "sync", id.as_str(), "--apply", "--json"],
        &[("ORBIT_GITHUB_FIXTURE", fixture.to_str().unwrap())],
    );
    assert_eq!(status, 1);
    assert_eq!(error["error"]["code"], "GITHUB_ISSUE_CONFLICT");
    assert_eq!(error["error"]["details"]["task"], id);
    assert_eq!(error["error"]["details"]["repository"], "acme/widget");
    assert_eq!(error["error"]["details"]["number"], 42);
    assert_eq!(error["error"]["details"]["conflicting_fields"], "title");
    let remediation = error["error"]["details"]["remediation"].as_str().unwrap();
    assert!(remediation.contains(&format!("Task {id}")));
    assert!(remediation.contains("acme/widget#42"));
    assert!(remediation.contains("title"));
    assert!(
        error["error"]["message"]
            .as_str()
            .unwrap()
            .contains(remediation)
    );

    let human = repo.orbit_with_env(
        &["github", "sync", id.as_str(), "--apply"],
        &[("ORBIT_GITHUB_FIXTURE", fixture.to_str().unwrap())],
    );
    assert!(!human.status.success());
    assert!(String::from_utf8_lossy(&human.stderr).contains(remediation));
}

#[test]
fn github_sync_can_keep_orbit_value_for_a_field_conflict() {
    let fixture_dir = tempfile::tempdir().unwrap();
    let fixture = write_fixture(&fixture_dir);
    let repo = TestRepo::new();
    let id = adopt_fixture_task(&repo, &fixture, "Local title");
    set_task_frontmatter(&repo, &id, "title", "\"Orbit edit\"");
    set_task_frontmatter(&repo, &id, "updated_at", "2026-10-25T14:00:00Z");
    set_fixture_issue_field(&fixture, "title", serde_json::json!("GitHub edit"));
    set_fixture_issue_field(
        &fixture,
        "updated_at",
        serde_json::json!("2026-10-25T13:00:00Z"),
    );

    let result = repo.ok_json_with_env(
        &[
            "github",
            "sync",
            id.as_str(),
            "--resolve",
            "title=orbit",
            "--apply",
            "--json",
        ],
        &[("ORBIT_GITHUB_FIXTURE", fixture.to_str().unwrap())],
    );

    assert_eq!(result["data"]["apply"], true);
    assert_eq!(
        result["data"]["tasks"][0]["plan"]["conflicts"],
        serde_json::json!([])
    );
    let applied: serde_json::Value = serde_json::from_slice(&fs::read(&fixture).unwrap()).unwrap();
    assert_eq!(applied["issues"]["acme/widget#42"]["title"], "Orbit edit");
    assert!(repo.read_task(&id).contains("title: Orbit edit"));
}

#[test]
fn github_adopt_accepts_issue_url_and_legacy_arguments() {
    let fixture_dir = tempfile::tempdir().unwrap();
    let fixture = write_fixture(&fixture_dir);
    add_fixture_issue(&fixture, "acme/widget#43", 43, "Legacy issue");
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
            "43",
            "--json",
        ],
        &[("ORBIT_GITHUB_FIXTURE", fixture.to_str().unwrap())],
    );
    assert_eq!(adopted["data"]["github"]["repo"], "acme/widget");
    assert_eq!(adopted["data"]["github"]["number"], 43);
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
fn github_create_applies_to_only_linked_repository() {
    let fixture_dir = tempfile::tempdir().unwrap();
    let fixture = write_fixture(&fixture_dir);
    let repo = TestRepo::new();
    repo.init();
    write_github_config_for_repositories(&repo, &["acme/widget"]);
    let id = repo.add("Create issue", &[]);

    let created = repo.ok_json_with_env(
        &["github", "create", id.as_str(), "--apply", "--json"],
        &[("ORBIT_GITHUB_FIXTURE", fixture.to_str().unwrap())],
    );
    assert_eq!(created["data"]["tasks"][0]["github"]["repo"], "acme/widget");
}

#[test]
fn github_create_previews_issue_without_writing_until_apply() {
    let fixture_dir = tempfile::tempdir().unwrap();
    let fixture = write_fixture(&fixture_dir);
    let repo = TestRepo::new();
    repo.init();
    write_github_config(&repo);
    let id = repo.add(
        "Preview issue",
        &["--label", "bug", "--body", "Description for GitHub"],
    );
    let task_before = repo.read_task(&id);
    let fixture_before = fs::read(&fixture).unwrap();

    let preview = repo.ok_json_with_env(
        &["github", "create", id.as_str(), "--json"],
        &[("ORBIT_GITHUB_FIXTURE", fixture.to_str().unwrap())],
    );

    assert_eq!(preview["data"]["apply_required"], true);
    assert_eq!(preview["data"]["count"], 1);
    assert_eq!(preview["data"]["preview_count"], 1);
    assert!(preview["data"]["success_count"].is_null());
    assert_eq!(preview["data"]["tasks"][0]["task"], id);
    assert_eq!(preview["data"]["tasks"][0]["repository"], "acme/widget");
    assert_eq!(preview["data"]["tasks"][0]["title"], "Preview issue");
    assert_eq!(preview["data"]["tasks"][0]["labels"][0], "bug");
    assert_eq!(fs::read(&fixture).unwrap(), fixture_before);
    assert_eq!(repo.read_task(&id), task_before);

    let human = repo.orbit_with_env(
        &["github", "create", &id],
        &[("ORBIT_GITHUB_FIXTURE", fixture.to_str().unwrap())],
    );
    assert!(human.status.success());
    let human = String::from_utf8_lossy(&human.stdout);
    for expected in [
        "acme/widget",
        "Preview issue",
        "Description for GitHub",
        "Labels: bug",
        "Task status: todo",
        "GitHub Issue state: open",
        "Apply with --apply",
    ] {
        assert!(human.contains(expected), "missing {expected:?} in {human}");
    }
}

#[test]
fn github_create_batches_report_partial_failures_and_persist_successful_links() {
    let fixture_dir = tempfile::tempdir().unwrap();
    let fixture = write_fixture(&fixture_dir);
    let mut data: serde_json::Value = serde_json::from_slice(&fs::read(&fixture).unwrap()).unwrap();
    data["create_failures"] = serde_json::json!({
        "acme/widget#Will fail": "fixture rejected this Issue"
    });
    fs::write(&fixture, serde_json::to_vec_pretty(&data).unwrap()).unwrap();

    let repo = TestRepo::new();
    repo.init();
    write_github_config(&repo);
    let created = repo.add("Will succeed", &["--label", "enhancement"]);
    let failed = repo.add("Will fail", &[]);

    let result = repo.ok_json_with_env(
        &["github", "create", &created, &failed, "--apply", "--json"],
        &[("ORBIT_GITHUB_FIXTURE", fixture.to_str().unwrap())],
    );

    assert_eq!(result["data"]["success_count"], 1);
    assert_eq!(result["data"]["failure_count"], 1);
    assert_eq!(result["data"]["tasks"][0]["success"], true);
    assert_eq!(
        result["data"]["tasks"][0]["issue"]["labels"][0],
        "enhancement"
    );
    assert_eq!(result["data"]["tasks"][1]["success"], false);
    assert_eq!(
        result["data"]["tasks"][1]["error"],
        "fixture rejected this Issue"
    );

    let created_task = repo.read_task(&created);
    assert!(created_task.contains("repo: acme/widget"));
    assert!(created_task.contains("last_sync:"));
    assert!(!repo.read_task(&failed).contains("github:"));
    let fixture: serde_json::Value = serde_json::from_slice(&fs::read(fixture).unwrap()).unwrap();
    assert_eq!(fixture["issues"].as_object().unwrap().len(), 2);
}

#[test]
fn github_create_includes_configured_issue_fields_in_the_created_issue() {
    let fixture_dir = tempfile::tempdir().unwrap();
    let fixture = write_fixture(&fixture_dir);
    let repo = TestRepo::new();
    repo.init();
    write_github_config(&repo);
    set_configured_issue_field_ids(&repo);
    let id = repo.add(
        "Issue with fields",
        &["--area", "cli", "--verification", "cargo test"],
    );

    let result = repo.ok_json_with_env(
        &["github", "create", &id, "--apply", "--json"],
        &[("ORBIT_GITHUB_FIXTURE", fixture.to_str().unwrap())],
    );

    assert_eq!(result["data"]["success_count"], 1);
    assert_eq!(result["data"]["tasks"][0]["area"], "cli");
    assert_eq!(result["data"]["tasks"][0]["verification"], "cargo test");
    assert_eq!(result["data"]["tasks"][0]["issue"]["area"], "cli");
    assert_eq!(
        result["data"]["tasks"][0]["issue"]["verification"],
        "cargo test"
    );
}

#[test]
fn github_create_requires_closed_opt_in_and_closes_opted_in_tasks() {
    let fixture_dir = tempfile::tempdir().unwrap();
    let fixture = write_fixture(&fixture_dir);
    let repo = TestRepo::new();
    repo.init();
    write_github_config(&repo);
    let id = repo.add("Terminal export", &[]);
    repo.ok_json(&["done", &id, "--by", "test", "--json"]);

    let refused = repo.ok_json_with_env(
        &["github", "create", &id, "--apply", "--json"],
        &[("ORBIT_GITHUB_FIXTURE", fixture.to_str().unwrap())],
    );
    assert_eq!(refused["data"]["failure_count"], 1);
    assert!(
        refused["data"]["tasks"][0]["error"]
            .as_str()
            .unwrap()
            .contains("--include-closed")
    );
    let before: serde_json::Value = serde_json::from_slice(&fs::read(&fixture).unwrap()).unwrap();
    assert_eq!(before["issues"].as_object().unwrap().len(), 1);

    let result = repo.ok_json_with_env(
        &[
            "github",
            "create",
            &id,
            "--include-closed",
            "--apply",
            "--json",
        ],
        &[("ORBIT_GITHUB_FIXTURE", fixture.to_str().unwrap())],
    );
    assert_eq!(result["data"]["success_count"], 1);
    assert_eq!(result["data"]["tasks"][0]["issue"]["state"], "closed");
    assert!(repo.read_task(&id).contains("repo: acme/widget"));
    let task = repo.ok_json(&["show", &id, "--json"]);
    assert_eq!(task["data"]["status"], "done");
}

#[test]
fn github_create_accepts_legacy_terminal_alias() {
    let fixture_dir = tempfile::tempdir().unwrap();
    let fixture = write_fixture(&fixture_dir);
    let repo = TestRepo::new();
    repo.init();
    write_github_config(&repo);
    let id = repo.add("Closed export via legacy alias", &[]);
    repo.ok_json(&["done", &id, "--by", "test", "--json"]);

    let result = repo.ok_json_with_env(
        &[
            "github",
            "create",
            &id,
            "--include-terminal",
            "--apply",
            "--json",
        ],
        &[("ORBIT_GITHUB_FIXTURE", fixture.to_str().unwrap())],
    );
    assert_eq!(result["data"]["success_count"], 1);
    assert_eq!(result["data"]["tasks"][0]["issue"]["state"], "closed");
}

#[test]
fn github_create_retry_rejects_already_linked_task_without_duplicate_issue() {
    let fixture_dir = tempfile::tempdir().unwrap();
    let fixture = write_fixture(&fixture_dir);
    let repo = TestRepo::new();
    repo.init();
    write_github_config(&repo);
    let id = repo.add("Create once", &[]);
    let env = [("ORBIT_GITHUB_FIXTURE", fixture.to_str().unwrap())];

    let first = repo.ok_json_with_env(&["github", "create", &id, "--apply", "--json"], &env);
    assert_eq!(first["data"]["success_count"], 1);
    let second = repo.ok_json_with_env(&["github", "create", &id, "--apply", "--json"], &env);
    assert_eq!(second["data"]["failure_count"], 1);
    assert!(
        second["data"]["tasks"][0]["error"]
            .as_str()
            .unwrap()
            .contains("orbit github sync")
    );
    let fixture: serde_json::Value = serde_json::from_slice(&fs::read(fixture).unwrap()).unwrap();
    assert_eq!(fixture["issues"].as_object().unwrap().len(), 2);
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
            "--apply",
            "--json",
        ],
        &[("ORBIT_GITHUB_FIXTURE", fixture.to_str().unwrap())],
    );
    assert_eq!(created["data"]["tasks"][0]["github"]["repo"], "acme/other");
}

#[test]
fn github_status_reports_healthy_configuration_and_is_read_only() {
    let fixture_dir = tempfile::tempdir().unwrap();
    let fixture = write_status_fixture(&fixture_dir);
    let repo = TestRepo::new();
    let id = adopt_fixture_task(&repo, &fixture, "Imported issue");
    set_configured_issue_field_ids(&repo);
    let config_before = fs::read(repo.board_path().join("config.toml")).unwrap();
    let task_before = repo.read_task(&id);
    let fixture_before = fs::read(&fixture).unwrap();
    let board_status_before = repo.git_in(&repo.board_path(), &["status", "--porcelain"]);

    let status = repo.ok_json_with_env(
        &["github", "status", "--json"],
        &[("ORBIT_GITHUB_FIXTURE", fixture.to_str().unwrap())],
    );

    assert_eq!(status["data"]["configured"], true);
    assert_eq!(status["data"]["auth"]["authenticated"], true);
    assert_eq!(status["data"]["auth"]["username"], "agent");
    assert_eq!(status["data"]["auth"]["scopes_known"], true);
    assert!(
        status["data"]["auth"]["scopes"]
            .as_array()
            .unwrap()
            .iter()
            .any(|scope| scope == "repo")
    );
    assert_eq!(status["data"]["organization"], "acme");
    assert_eq!(status["data"]["repositories"][0]["name"], "acme/widget");
    assert_eq!(status["data"]["fields"]["area"]["configured"], true);
    assert_eq!(status["data"]["fields"]["area"]["resolved"], true);
    assert_eq!(status["data"]["fields"]["verification"]["resolved"], true);
    assert_eq!(status["data"]["linked_tasks"], 1);
    assert_eq!(status["data"]["pending"]["pull"], 0);
    assert_eq!(status["data"]["pending"]["push"], 0);
    assert_eq!(status["data"]["conflicts"], 0);
    assert_eq!(status["data"]["unsupported_relationships"], 0);
    assert_eq!(status["data"]["next_action"], "none");
    assert_eq!(status["data"]["next_command"], serde_json::Value::Null);
    assert_eq!(
        fs::read(repo.board_path().join("config.toml")).unwrap(),
        config_before
    );
    assert_eq!(repo.read_task(&id), task_before);
    assert_eq!(fs::read(&fixture).unwrap(), fixture_before);
    let board_status_after = repo.git_in(&repo.board_path(), &["status", "--porcelain"]);
    assert_eq!(board_status_after.stdout, board_status_before.stdout);

    let human = repo.orbit_with_env(
        &["github", "status"],
        &[("ORBIT_GITHUB_FIXTURE", fixture.to_str().unwrap())],
    );
    assert!(human.status.success());
    let output = String::from_utf8_lossy(&human.stdout);
    assert!(output.contains("GitHub integration: healthy"), "{output}");
    assert!(output.contains("acme/widget"), "{output}");
    assert!(output.contains("scopes: read:org, repo"), "{output}");
}

#[test]
fn github_status_reports_unconfigured_and_missing_fields() {
    let fixture_dir = tempfile::tempdir().unwrap();
    let fixture = write_status_fixture(&fixture_dir);
    let repo = TestRepo::new();
    repo.init();

    let unconfigured = repo.ok_json_with_env(
        &["github", "status", "--json"],
        &[("ORBIT_GITHUB_FIXTURE", fixture.to_str().unwrap())],
    );
    assert_eq!(unconfigured["data"]["configured"], false);
    assert_eq!(unconfigured["data"]["next_action"], "orbit github connect");

    let mut fixture_data: serde_json::Value =
        serde_json::from_slice(&fs::read(&fixture).unwrap()).unwrap();
    fixture_data["auth_message"] =
        serde_json::json!("Logged in to github.com account agent (keyring)\nToken scopes: 'repo'");
    fs::write(&fixture, serde_json::to_vec_pretty(&fixture_data).unwrap()).unwrap();
    let missing_scope = repo.ok_json_with_env(
        &["github", "status", "--json"],
        &[("ORBIT_GITHUB_FIXTURE", fixture.to_str().unwrap())],
    );
    assert_eq!(
        missing_scope["data"]["auth"]["missing_scopes"][0],
        "read:org"
    );
    assert_eq!(
        missing_scope["data"]["next_command"],
        "gh auth refresh --hostname github.com --scopes read:org,repo"
    );

    fixture_data["auth_message"] = serde_json::json!(
        "Logged in to github.com account agent (keyring)\nToken scopes: 'read:org', 'repo'"
    );
    fixture_data["authenticated"] = serde_json::json!(false);
    fixture_data["auth_message"] = serde_json::json!("not logged in");
    fs::write(&fixture, serde_json::to_vec_pretty(&fixture_data).unwrap()).unwrap();
    let unauthenticated = repo.ok_json_with_env(
        &["github", "status", "--json"],
        &[("ORBIT_GITHUB_FIXTURE", fixture.to_str().unwrap())],
    );
    assert_eq!(unauthenticated["data"]["auth"]["authenticated"], false);
    assert_eq!(unauthenticated["data"]["next_action"], "gh auth login");

    write_github_config(&repo);
    fixture_data["authenticated"] = serde_json::json!(true);
    fixture_data["org_fields"] = serde_json::json!({ "acme/Area": "AREA_ID" });
    fs::write(&fixture, serde_json::to_vec_pretty(&fixture_data).unwrap()).unwrap();
    let missing = repo.ok_json_with_env(
        &["github", "status", "--json"],
        &[("ORBIT_GITHUB_FIXTURE", fixture.to_str().unwrap())],
    );
    assert_eq!(missing["data"]["fields"]["area"]["resolved"], true);
    assert_eq!(missing["data"]["fields"]["verification"]["resolved"], false);
    assert_eq!(missing["data"]["next_action"], "orbit github setup");
    assert_eq!(missing["data"]["next_command"], "orbit github setup");

    fixture_data["org_fields"] = serde_json::json!({});
    fs::write(&fixture, serde_json::to_vec_pretty(&fixture_data).unwrap()).unwrap();
    let missing_both = repo.ok_json_with_env(
        &["github", "status", "--json"],
        &[("ORBIT_GITHUB_FIXTURE", fixture.to_str().unwrap())],
    );
    assert_eq!(missing_both["data"]["fields"]["area"]["resolved"], false);
    assert_eq!(
        missing_both["data"]["fields"]["verification"]["resolved"],
        false
    );
}

#[test]
fn github_status_reports_conflicts_and_doctor_reports_configuration_problems() {
    let fixture_dir = tempfile::tempdir().unwrap();
    let fixture = write_status_fixture(&fixture_dir);
    let repo = TestRepo::new();
    let id = adopt_fixture_task(&repo, &fixture, "Imported issue");
    set_configured_issue_field_ids(&repo);
    set_task_frontmatter(&repo, &id, "title", "\"Orbit edit\"");
    set_task_frontmatter(&repo, &id, "area", "setup");
    set_task_frontmatter(&repo, &id, "updated_at", "2026-11-28T15:00:00Z");
    set_fixture_issue_field(&fixture, "title", serde_json::json!("GitHub edit"));
    set_fixture_issue_field(
        &fixture,
        "updated_at",
        serde_json::json!("2026-10-27T15:00:00Z"),
    );

    let status = repo.ok_json_with_env(
        &["github", "status", "--json"],
        &[("ORBIT_GITHUB_FIXTURE", fixture.to_str().unwrap())],
    );
    assert_eq!(status["data"]["conflicts"], 1, "{status}");
    assert_eq!(status["data"]["pending"]["pull"], 0);
    assert!(status["data"]["pending"]["push"].as_u64().unwrap() > 0);
    assert_eq!(status["data"]["conflict_details"][0]["task"], id);
    assert_eq!(
        status["data"]["next_action"],
        "resolve_github_sync_conflicts"
    );
    assert_eq!(
        status["data"]["next_command"],
        format!("orbit github sync {id}")
    );

    let mut fixture_data: serde_json::Value =
        serde_json::from_slice(&fs::read(&fixture).unwrap()).unwrap();
    fixture_data["org_fields"] = serde_json::json!({});
    fs::write(&fixture, serde_json::to_vec_pretty(&fixture_data).unwrap()).unwrap();
    let (exit, doctor) = repo.json_with_env(
        &["doctor", "--json"],
        &[("ORBIT_GITHUB_FIXTURE", fixture.to_str().unwrap())],
    );
    assert_eq!(exit, 1);
    assert_eq!(doctor["data"]["tasks"], 1);
    assert!(
        doctor["data"]["issues"]
            .as_array()
            .unwrap()
            .iter()
            .any(|issue| issue["code"] == "GITHUB_SETUP_INCOMPLETE")
    );
}

#[test]
fn github_status_reports_unlinked_repositories_and_cross_repo_relationships() {
    let fixture_dir = tempfile::tempdir().unwrap();
    let fixture = write_status_fixture(&fixture_dir);
    let mut fixture_data: serde_json::Value =
        serde_json::from_slice(&fs::read(&fixture).unwrap()).unwrap();
    fixture_data["repositories"]["acme/other"] =
        serde_json::json!({ "owner_type": "Organization", "accessible": true });
    let mut child_issue = fixture_data["issues"]["acme/widget#42"].clone();
    child_issue["repo"] = serde_json::json!("acme/other");
    child_issue["number"] = serde_json::json!(43);
    fixture_data["issues"]["acme/other#43"] = child_issue;
    fs::write(&fixture, serde_json::to_vec_pretty(&fixture_data).unwrap()).unwrap();

    let repo = TestRepo::new();
    repo.init();
    write_github_config_for_repositories(&repo, &["acme/widget", "acme/other"]);
    let parent = repo.add("Parent", &[]);
    let child = repo.add("Child", &["--parent", parent.as_str()]);
    repo.ok_json_with_env(
        &[
            "github",
            "adopt",
            parent.as_str(),
            "https://github.com/acme/widget/issues/42",
            "--json",
        ],
        &[("ORBIT_GITHUB_FIXTURE", fixture.to_str().unwrap())],
    );
    repo.ok_json_with_env(
        &[
            "github",
            "adopt",
            child.as_str(),
            "https://github.com/acme/other/issues/43",
            "--json",
        ],
        &[("ORBIT_GITHUB_FIXTURE", fixture.to_str().unwrap())],
    );
    repo.ok_json(&["github", "repo", "unlink", "acme/other", "--json"]);

    let status = repo.ok_json_with_env(
        &["github", "status", "--json"],
        &[("ORBIT_GITHUB_FIXTURE", fixture.to_str().unwrap())],
    );

    assert_eq!(status["data"]["unsupported_relationships"], 1);
    assert!(
        status["data"]["unsupported_details"][0]["message"]
            .as_str()
            .unwrap()
            .contains("cross-repository")
    );
    assert!(
        status["data"]["issues"]
            .as_array()
            .unwrap()
            .iter()
            .any(|issue| issue["code"] == "GITHUB_REPO_NOT_LINKED" && issue["task"] == child)
    );
}

#[test]
fn github_status_does_not_repair_an_orphan_board_during_discovery() {
    let repo = TestRepo::new();
    repo.init();
    repo.add("Orphaned task", &[]);
    fs::remove_dir_all(repo.board_path()).unwrap();

    let (status, result) = repo.json(&["github", "status", "--json"]);

    assert_ne!(status, 0);
    assert_eq!(result["ok"], false);
    assert!(!repo.board_path().exists());
}

#[test]
fn github_status_handles_basic_mode_without_issue_fields() {
    let fixture_dir = tempfile::tempdir().unwrap();
    let fixture = write_fixture(&fixture_dir);
    let mut data: serde_json::Value = serde_json::from_slice(&fs::read(&fixture).unwrap()).unwrap();
    data["repositories"] = serde_json::json!({
        "personal/widget": { "owner_type": "User", "accessible": true }
    });
    data["auth_message"] =
        serde_json::json!("Logged in to github.com account agent (keyring)\nToken scopes: 'repo'");
    fs::write(&fixture, serde_json::to_vec_pretty(&data).unwrap()).unwrap();

    let repo = TestRepo::new();
    repo.init();
    write_basic_github_config(&repo);

    let status = repo.ok_json_with_env(
        &["github", "status", "--json"],
        &[("ORBIT_GITHUB_FIXTURE", fixture.to_str().unwrap())],
    );
    assert_eq!(status["data"]["configured"], true);
    assert_eq!(status["data"]["mode"], "basic");
    assert_eq!(status["data"]["fields"]["area"]["applicable"], false);
    assert_eq!(
        status["data"]["fields"]["area"]["configured"],
        serde_json::Value::Null
    );
    assert_eq!(
        status["data"]["auth"]["missing_scopes"],
        serde_json::json!([])
    );
    assert_eq!(status["data"]["next_action"], "none");

    let doctor = repo.ok_json_with_env(
        &["doctor", "--json"],
        &[("ORBIT_GITHUB_FIXTURE", fixture.to_str().unwrap())],
    );
    assert_eq!(doctor["data"]["healthy"], true);
}

#[test]
fn github_status_and_doctor_report_the_same_missing_scopes_in_each_mode() {
    for (mode, scopes, expected_missing) in [
        ("full", "'repo'", "read:org"),
        ("basic", "'read:org'", "repo"),
    ] {
        let fixture_dir = tempfile::tempdir().unwrap();
        let fixture = write_status_fixture(&fixture_dir);
        let mut data: serde_json::Value =
            serde_json::from_slice(&fs::read(&fixture).unwrap()).unwrap();
        data["auth_message"] = serde_json::json!(format!(
            "Logged in to github.com account agent (keyring)\nToken scopes: {scopes}"
        ));
        fs::write(&fixture, serde_json::to_vec_pretty(&data).unwrap()).unwrap();

        let repo = TestRepo::new();
        repo.init();
        if mode == "basic" {
            write_basic_github_config(&repo);
        } else {
            write_github_config(&repo);
        }

        let status = repo.ok_json_with_env(
            &["github", "status", "--json"],
            &[("ORBIT_GITHUB_FIXTURE", fixture.to_str().unwrap())],
        );
        assert_eq!(
            status["data"]["auth"]["missing_scopes"],
            serde_json::json!([expected_missing]),
            "status should apply the {mode} mode scope rules"
        );

        let (exit, doctor) = repo.json_with_env(
            &["doctor", "--json"],
            &[("ORBIT_GITHUB_FIXTURE", fixture.to_str().unwrap())],
        );
        assert_eq!(exit, 1);
        let issue = doctor["data"]["issues"]
            .as_array()
            .unwrap()
            .iter()
            .find(|issue| issue["code"] == "GITHUB_SCOPES_MISSING")
            .unwrap_or_else(|| panic!("doctor should report the missing scope in {mode} mode"));
        assert!(
            issue["message"]
                .as_str()
                .unwrap()
                .contains(expected_missing),
            "doctor should report {expected_missing} in {mode} mode: {issue}"
        );
    }
}

#[test]
fn injected_sync_execution_previews_then_pulls_without_cli_transport_selection() {
    use orbit_tracker::board::Board;
    use orbit_tracker::github::client::FixtureClient;
    use orbit_tracker::github::execution::{SyncOutcome, SyncRequest, synchronize};
    use orbit_tracker::github::model::SyncDirection;

    let repo = TestRepo::new();
    let fixture_dir = TempDir::new().unwrap();
    let fixture = write_fixture(&fixture_dir);
    let id = adopt_fixture_task(&repo, &fixture, "Local title");
    set_fixture_issue_field(&fixture, "title", serde_json::json!("Remote revision"));
    let board = Board::at_root(repo.board_path(), Vec::new()).unwrap();
    let client = FixtureClient::load(&fixture).unwrap();
    let original_task = fs::read(board.task_path(&id)).unwrap();
    let original_fixture = fs::read(&fixture).unwrap();
    let _lock = board.lock_exclusive().unwrap();
    let preview = synchronize(
        &board,
        &client,
        SyncRequest {
            task: Some(&id),
            direction: SyncDirection::Pull,
            resolutions: &[],
            apply: false,
        },
    )
    .unwrap();
    assert_eq!(preview.tasks.len(), 1);
    assert_eq!(
        preview.tasks[0].plan.apply_title.as_deref(),
        Some("Remote revision")
    );
    assert!(matches!(preview.tasks[0].outcome, SyncOutcome::Preview));
    assert_eq!(fs::read(board.task_path(&id)).unwrap(), original_task);
    assert_eq!(fs::read(&fixture).unwrap(), original_fixture);
    let applied = synchronize(
        &board,
        &client,
        SyncRequest {
            task: Some(&id),
            direction: SyncDirection::Pull,
            resolutions: &[],
            apply: true,
        },
    )
    .unwrap();
    assert_eq!(applied.success_count, 1);
    assert!(applied.failures.is_empty());
    drop(_lock);
    let shown = repo.ok_json(&["show", &id, "--json"]);
    assert_eq!(shown["data"]["title"], "Remote revision");
}

#[test]
fn sync_reports_commit_failure_without_losing_pulled_changes() {
    let repo = TestRepo::new();
    let fixture_dir = TempDir::new().unwrap();
    let fixture = write_fixture(&fixture_dir);
    let id = adopt_fixture_task(&repo, &fixture, "Local title");
    set_fixture_issue_field(&fixture, "title", serde_json::json!("Remote revision"));
    let hooks = repo.path().join("failing-hooks");
    fs::create_dir(&hooks).unwrap();
    let hook = hooks.join("pre-commit");
    fs::write(&hook, "#!/bin/sh\nexit 1\n").unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&hook, fs::Permissions::from_mode(0o755)).unwrap();
    }
    repo.git(&["config", "core.hooksPath", hooks.to_str().unwrap()]);
    let applied = repo.ok_json_with_env(
        &[
            "github",
            "sync",
            &id,
            "--direction",
            "pull",
            "--apply",
            "--json",
        ],
        &[("ORBIT_GITHUB_FIXTURE", fixture.to_str().unwrap())],
    );
    assert_eq!(applied["data"]["success_count"], 1);
    assert!(
        applied["meta"]["warnings"]
            .as_array()
            .unwrap()
            .iter()
            .any(|warning| warning.as_str().unwrap().starts_with("COMMIT_FAILED:"))
    );
    assert_eq!(
        repo.ok_json(&["show", &id, "--json"])["data"]["title"],
        "Remote revision"
    );
}

fn sync_request<'a>(
    id: &'a str,
    resolutions: &'a [String],
    apply: bool,
) -> orbit_tracker::github::execution::SyncRequest<'a> {
    orbit_tracker::github::execution::SyncRequest {
        task: Some(id),
        direction: orbit_tracker::github::model::SyncDirection::Both,
        resolutions,
        apply,
    }
}

#[test]
fn injected_sync_requires_a_conflict_choice_and_applies_the_github_side() {
    use orbit_tracker::board::Board;
    use orbit_tracker::github::client::FixtureClient;
    use orbit_tracker::github::execution::synchronize;

    let fixture_dir = TempDir::new().unwrap();
    let fixture = write_fixture(&fixture_dir);
    let repo = TestRepo::new();
    let id = adopt_fixture_task(&repo, &fixture, "Local title");
    set_task_frontmatter(&repo, &id, "title", "\"Orbit edit\"");
    set_fixture_issue_field(&fixture, "title", serde_json::json!("GitHub edit"));
    let board = Board::at_root(repo.board_path(), Vec::new()).unwrap();
    let client = FixtureClient::load(&fixture).unwrap();
    let lock = board.lock_exclusive().unwrap();

    let error = synchronize(&board, &client, sync_request(&id, &[], true))
        .err()
        .expect("unresolved conflicts must stop before writing");
    assert_eq!(error.code, "GITHUB_ISSUE_CONFLICT");

    let choice = ["title=github".to_string()];
    let report = synchronize(&board, &client, sync_request(&id, &choice, true)).unwrap();
    assert_eq!(report.success_count, 1);
    drop(lock);
    assert_eq!(
        repo.ok_json(&["show", &id, "--json"])["data"]["title"],
        "GitHub edit"
    );
}

#[test]
fn injected_sync_reports_typed_failures_and_retries_without_advancing_the_baseline() {
    use orbit_tracker::board::Board;
    use orbit_tracker::github::client::FixtureClient;
    use orbit_tracker::github::execution::{SyncOutcome, synchronize};

    let fixture_dir = TempDir::new().unwrap();
    let fixture = write_fixture(&fixture_dir);
    let repo = TestRepo::new();
    let id = adopt_fixture_task(&repo, &fixture, "Local title");
    set_task_frontmatter(&repo, &id, "title", "\"Orbit edit\"");
    let mut data: serde_json::Value = serde_json::from_slice(&fs::read(&fixture).unwrap()).unwrap();
    data["update_failures"] = serde_json::json!({ "acme/widget#42": "temporary fixture failure" });
    fs::write(&fixture, serde_json::to_vec_pretty(&data).unwrap()).unwrap();
    let board = Board::at_root(repo.board_path(), Vec::new()).unwrap();
    let _lock = board.lock_exclusive().unwrap();

    let client = FixtureClient::load(&fixture).unwrap();
    let failed = synchronize(&board, &client, sync_request(&id, &[], true)).unwrap();
    assert_eq!(failed.success_count, 0);
    assert_eq!(failed.failures.len(), 1);
    assert_eq!(failed.failures[0].task, id);
    assert_eq!(failed.failures[0].number, 42);
    assert!(failed.failures[0].retry_safe);
    assert!(matches!(failed.tasks[0].outcome, SyncOutcome::Failed(_)));

    data["update_failures"] = serde_json::json!({});
    fs::write(&fixture, serde_json::to_vec_pretty(&data).unwrap()).unwrap();
    let client = FixtureClient::load(&fixture).unwrap();
    let retried = synchronize(&board, &client, sync_request(&id, &[], true)).unwrap();
    assert_eq!(retried.success_count, 1);
    assert!(retried.failures.is_empty());
    assert!(matches!(retried.tasks[0].outcome, SyncOutcome::Applied));
    let remote: serde_json::Value = serde_json::from_slice(&fs::read(&fixture).unwrap()).unwrap();
    assert_eq!(remote["issues"]["acme/widget#42"]["title"], "Orbit edit");
}

#[test]
fn injected_sync_rejects_issue_fields_in_basic_mode() {
    use orbit_tracker::board::Board;
    use orbit_tracker::github::client::FixtureClient;
    use orbit_tracker::github::execution::synchronize;

    let fixture_dir = TempDir::new().unwrap();
    let fixture = write_fixture(&fixture_dir);
    let personal = fs::read_to_string(&fixture)
        .unwrap()
        .replace("acme/widget", "personal/widget");
    fs::write(&fixture, personal).unwrap();
    let repo = TestRepo::new();
    repo.init();
    write_basic_github_config(&repo);
    let id = repo.add("Local task", &[]);
    repo.ok_json_with_env(
        &[
            "github",
            "adopt",
            &id,
            "--repo",
            "personal/widget",
            "--number",
            "42",
            "--json",
        ],
        &[("ORBIT_GITHUB_FIXTURE", fixture.to_str().unwrap())],
    );
    repo.ok_json(&["update", &id, "--area", "cli", "--json"]);
    let board = Board::at_root(repo.board_path(), Vec::new()).unwrap();
    let client = FixtureClient::load(&fixture).unwrap();
    let _lock = board.lock_exclusive().unwrap();

    let error = synchronize(&board, &client, sync_request(&id, &[], true))
        .err()
        .expect("basic mode cannot publish structured metadata");
    assert_eq!(error.code, "INVALID_ARGUMENT");
    assert!(error.message.contains("area"));
}

#[test]
fn github_auto_push_toggle_commits_the_board_flag() {
    let repo = TestRepo::new();
    repo.init();
    write_github_config(&repo);

    let on = repo.ok_json(&["github", "auto-push", "on", "--json"]);
    assert_eq!(on["data"]["auto_push"]["enabled"], true);
    assert_eq!(on["data"]["auto_push"]["previous"], false);
    assert_eq!(on["data"]["auto_push"]["changed"], true);
    let config = fs::read_to_string(repo.board_path().join("config.toml")).unwrap();
    assert!(config.contains("auto_push = true"), "{config}");
    let log = repo.git_in(&repo.board_path(), &["log", "-1", "--format=%s"]);
    assert_eq!(
        String::from_utf8_lossy(&log.stdout).trim(),
        "orbit: github auto-push on"
    );

    let again = repo.ok_json(&["github", "auto-push", "enable", "--json"]);
    assert_eq!(again["data"]["auto_push"]["enabled"], true);
    assert_eq!(again["data"]["auto_push"]["changed"], false);

    let off = repo.ok_json(&["github", "auto-push", "disable", "--json"]);
    assert_eq!(off["data"]["auto_push"]["enabled"], false);
    assert_eq!(off["data"]["auto_push"]["previous"], true);
    let config = fs::read_to_string(repo.board_path().join("config.toml")).unwrap();
    assert!(!config.contains("auto_push = true"), "{config}");
}

fn enable_auto_push(repo: &TestRepo, repositories: &[&str], basic: bool) {
    let path = repo.board_path().join("config.toml");
    let mut config = fs::read_to_string(&path).unwrap();
    let repositories = repositories
        .iter()
        .map(|repository| format!("\"{repository}\""))
        .collect::<Vec<_>>()
        .join(", ");
    config.push_str(&format!(
        "\n[github]\nmode = \"{}\"\nrepositories = [{repositories}]\nauto_push = true\n",
        if basic { "basic" } else { "full" }
    ));
    if !basic {
        config.push_str("organization = \"acme\"\n");
    }
    fs::write(path, config).unwrap();
}

fn empty_fixture(dir: &TempDir) -> std::path::PathBuf {
    let path = dir.path().join("fixture.json");
    fs::write(
        &path,
        r#"{
  "org_fields": {
    "acme/Area": "AREA_ID",
    "acme/Verification": "VERIFICATION_ID"
  },
  "issues": {},
  "repositories": {
    "acme/widget": { "owner_type": "Organization", "accessible": true }
  }
}"#,
    )
    .unwrap();
    path
}

fn auto_push_fixture() -> (TempDir, std::path::PathBuf) {
    let dir = tempfile::tempdir().unwrap();
    let path = empty_fixture(&dir);
    (dir, path)
}

fn fixture_issue(path: &std::path::Path, number: u64) -> serde_json::Value {
    let fixture: serde_json::Value = serde_json::from_slice(&fs::read(path).unwrap()).unwrap();
    fixture["issues"]
        .as_object()
        .and_then(|issues| issues.get(&format!("acme/widget#{number}")))
        .cloned()
        .unwrap_or_else(|| panic!("missing fixture issue {number}: {fixture}"))
}

#[test]
fn auto_push_creates_an_issue_and_tracks_status_and_labels() {
    let repo = TestRepo::new();
    repo.init();
    enable_auto_push(&repo, &["acme/widget"], false);
    let (_fixture_dir, fixture) = auto_push_fixture();
    let env = [("ORBIT_GITHUB_FIXTURE", fixture.to_str().unwrap())];

    let add = repo.ok_json_with_env(&["add", "Auto-created task", "-p", "1", "--json"], &env);
    let id = add["data"]["id"].as_str().unwrap().to_string();
    let created = fixture_issue(&fixture, 1);
    assert_eq!(created["title"], "Auto-created task");
    assert_eq!(created["state"], "open");
    assert!(created["labels"].as_array().unwrap().is_empty());

    repo.ok_json_with_env(&["claim", &id, "--by", "agent", "--json"], &env);
    let claimed = fixture_issue(&fixture, 1);
    assert!(
        claimed["labels"]
            .as_array()
            .unwrap()
            .iter()
            .any(|label| label == "orbit:in-progress")
    );

    repo.ok_json_with_env(&["release", &id, "--by", "agent", "--json"], &env);
    assert!(
        fixture_issue(&fixture, 1)["labels"]
            .as_array()
            .unwrap()
            .iter()
            .all(|label| label != "orbit:in-progress")
    );

    repo.ok_json_with_env(&["claim", &id, "--by", "agent", "--json"], &env);
    repo.ok_json_with_env(&["done", &id, "--by", "agent", "--json"], &env);
    let closed = fixture_issue(&fixture, 1);
    assert_eq!(closed["state"], "closed");
    assert_eq!(closed["state_reason"], "completed");
    assert!(
        closed["labels"]
            .as_array()
            .unwrap()
            .iter()
            .all(|label| label != "orbit:in-progress")
    );
}

#[test]
fn auto_push_cancels_with_not_planned_and_reopens() {
    let repo = TestRepo::new();
    repo.init();
    enable_auto_push(&repo, &["acme/widget"], false);
    let (_fixture_dir, fixture) = auto_push_fixture();
    let env = [("ORBIT_GITHUB_FIXTURE", fixture.to_str().unwrap())];
    let add = repo.ok_json_with_env(&["add", "Cancel me", "--json"], &env);
    let id = add["data"]["id"].as_str().unwrap().to_string();

    repo.ok_json_with_env(&["cancel", &id, "--by", "agent", "--json"], &env);
    let cancelled = fixture_issue(&fixture, 1);
    assert_eq!(cancelled["state"], "closed");
    assert_eq!(cancelled["state_reason"], "not_planned");

    repo.ok_json_with_env(&["reopen", &id, "--json"], &env);
    let reopened = fixture_issue(&fixture, 1);
    assert_eq!(reopened["state"], "open");
    assert!(reopened.get("state_reason").is_none());
}

#[test]
fn auto_push_pushes_mapped_fields_but_not_priority_changes() {
    let repo = TestRepo::new();
    repo.init();
    enable_auto_push(&repo, &["acme/widget"], false);
    let (_fixture_dir, fixture) = auto_push_fixture();
    let env = [("ORBIT_GITHUB_FIXTURE", fixture.to_str().unwrap())];
    let add = repo.ok_json_with_env(&["add", "Field task", "--json"], &env);
    let id = add["data"]["id"].as_str().unwrap().to_string();
    let before = fixture_issue(&fixture, 1)["updated_at"].clone();

    repo.ok_json_with_env(&["update", &id, "--priority", "0", "--json"], &env);
    let after_priority = fixture_issue(&fixture, 1);
    assert_eq!(after_priority["updated_at"], before);
    assert_eq!(after_priority["title"], "Field task");

    repo.ok_json_with_env(&["update", &id, "--title", "Renamed", "--json"], &env);
    let after_title = fixture_issue(&fixture, 1);
    assert_eq!(after_title["title"], "Renamed");
    assert_ne!(after_title["updated_at"], before);
}

#[test]
fn auto_push_warns_and_preserves_a_remote_conflict() {
    let repo = TestRepo::new();
    repo.init();
    enable_auto_push(&repo, &["acme/widget"], false);
    let (_fixture_dir, fixture) = auto_push_fixture();
    let env = [("ORBIT_GITHUB_FIXTURE", fixture.to_str().unwrap())];
    let add = repo.ok_json_with_env(&["add", "Original", "--json"], &env);
    let id = add["data"]["id"].as_str().unwrap().to_string();

    let mut document: serde_json::Value =
        serde_json::from_slice(&fs::read(&fixture).unwrap()).unwrap();
    document["issues"]["acme/widget#1"]["title"] = serde_json::json!("Remote edit");
    fs::write(&fixture, serde_json::to_vec_pretty(&document).unwrap()).unwrap();

    let update = repo.ok_json_with_env(&["update", &id, "--title", "Local edit", "--json"], &env);
    let warnings = update["meta"]["warnings"].as_array().unwrap();
    assert!(
        warnings.iter().any(|warning| warning
            .as_str()
            .unwrap()
            .contains("GITHUB_AUTO_PUSH_CONFLICT")),
        "{warnings:?}"
    );
    assert_eq!(fixture_issue(&fixture, 1)["title"], "Remote edit");
    let shown = repo.ok_json_with_env(&["show", &id, "--json"], &env);
    assert_eq!(shown["data"]["github"]["last_sync"]["title"], "Original");
}

#[test]
fn auto_push_respects_no_sync_and_the_environment_switch() {
    let repo = TestRepo::new();
    repo.init();
    enable_auto_push(&repo, &["acme/widget"], false);
    let (_fixture_dir, fixture) = auto_push_fixture();
    let env = [("ORBIT_GITHUB_FIXTURE", fixture.to_str().unwrap())];

    repo.ok_json_with_env(&["--no-sync", "add", "Command local", "--json"], &env);
    repo.ok_json_with_env(
        &["add", "Environment local", "--json"],
        &[
            ("ORBIT_GITHUB_FIXTURE", fixture.to_str().unwrap()),
            ("ORBIT_NO_AUTO_SYNC", "1"),
        ],
    );
    let fixture_json: serde_json::Value =
        serde_json::from_slice(&fs::read(&fixture).unwrap()).unwrap();
    assert!(
        fixture_json["issues"].as_object().unwrap().is_empty(),
        "{fixture_json}"
    );
}

#[test]
fn auto_push_requires_exactly_one_destination_repository() {
    let repo = TestRepo::new();
    repo.init();
    enable_auto_push(&repo, &["acme/widget", "acme/other"], false);
    let (_fixture_dir, fixture) = auto_push_fixture();
    let env = [("ORBIT_GITHUB_FIXTURE", fixture.to_str().unwrap())];

    let add = repo.ok_json_with_env(&["add", "Ambiguous", "--json"], &env);
    let warnings = add["meta"]["warnings"].as_array().unwrap();
    assert!(
        warnings
            .iter()
            .any(|warning| warning.as_str().unwrap().contains("exactly one")),
        "{warnings:?}"
    );
    let fixture_json: serde_json::Value =
        serde_json::from_slice(&fs::read(&fixture).unwrap()).unwrap();
    assert!(fixture_json["issues"].as_object().unwrap().is_empty());
}

#[test]
fn auto_push_does_not_create_an_issue_for_a_preexisting_local_task() {
    let repo = TestRepo::new();
    repo.init();
    let id = repo.add("Local first", &[]);
    enable_auto_push(&repo, &["acme/widget"], false);
    let (_fixture_dir, fixture) = auto_push_fixture();
    let env = [("ORBIT_GITHUB_FIXTURE", fixture.to_str().unwrap())];

    repo.ok_json_with_env(&["update", &id, "--title", "Local edited", "--json"], &env);
    let fixture_json: serde_json::Value =
        serde_json::from_slice(&fs::read(&fixture).unwrap()).unwrap();
    assert!(fixture_json["issues"].as_object().unwrap().is_empty());
}

#[test]
fn auto_push_works_with_auto_commit_disabled() {
    let repo = TestRepo::new();
    repo.init();
    let config_path = repo.board_path().join("config.toml");
    let config = fs::read_to_string(&config_path)
        .unwrap()
        .replace("auto_commit = true", "auto_commit = false");
    fs::write(&config_path, config).unwrap();
    enable_auto_push(&repo, &["acme/widget"], false);
    let (_fixture_dir, fixture) = auto_push_fixture();
    let env = [("ORBIT_GITHUB_FIXTURE", fixture.to_str().unwrap())];

    let add = repo.ok_json_with_env(&["add", "No commit", "--json"], &env);
    let id = add["data"]["id"].as_str().unwrap();
    let shown = repo.ok_json_with_env(&["show", id, "--json"], &env);
    assert_eq!(shown["data"]["github"]["number"], 1);
    assert_eq!(fixture_issue(&fixture, 1)["title"], "No commit");
}

#[test]
fn auto_push_basic_mode_keeps_area_and_verification_local() {
    let repo = TestRepo::new();
    repo.init();
    enable_auto_push(&repo, &["personal/widget"], true);
    let (_fixture_dir, fixture) = auto_push_fixture();
    let env = [("ORBIT_GITHUB_FIXTURE", fixture.to_str().unwrap())];

    let add = repo.ok_json_with_env(&["add", "Metadata task", "--area", "cli", "--json"], &env);
    let warnings = add["meta"]["warnings"].as_array().unwrap();
    assert!(
        warnings
            .iter()
            .any(|warning| warning.as_str().unwrap().contains("basic mode cannot sync")),
        "{warnings:?}"
    );
    let fixture_json: serde_json::Value =
        serde_json::from_slice(&fs::read(&fixture).unwrap()).unwrap();
    assert!(fixture_json["issues"].as_object().unwrap().is_empty());
}

#[test]
fn github_status_reports_auto_push_state() {
    let repo = TestRepo::new();
    repo.init();
    enable_auto_push(&repo, &["acme/widget"], false);
    let (_fixture_dir, fixture) = auto_push_fixture();
    let env = [("ORBIT_GITHUB_FIXTURE", fixture.to_str().unwrap())];

    let status = repo.ok_json_with_env(&["github", "status", "--json"], &env);
    assert_eq!(status["data"]["auto_push"]["enabled"], true);
    assert_eq!(
        status["data"]["auto_push"]["reserved_label"],
        "orbit:in-progress"
    );
    assert_eq!(status["data"]["linked_tasks"], 0);
    assert_eq!(status["data"]["local_tasks"], 0);
}

#[test]
fn doctor_reports_a_missing_reserved_label_when_auto_push_is_enabled() {
    let repo = TestRepo::new();
    repo.init();
    enable_auto_push(&repo, &["acme/widget"], false);
    let (_fixture_dir, fixture) = auto_push_fixture();
    let env = [("ORBIT_GITHUB_FIXTURE", fixture.to_str().unwrap())];

    let (exit, doctor) = repo.json_with_env(&["doctor", "--json"], &env);
    assert_eq!(exit, 1, "{doctor}");
    assert!(
        doctor["data"]["issues"]
            .as_array()
            .unwrap()
            .iter()
            .any(|issue| issue["message"]
                .as_str()
                .unwrap_or_default()
                .contains("orbit:in-progress")),
        "{doctor}"
    );
}

#[test]
fn auto_push_is_skipped_when_the_local_commit_fails() {
    let repo = TestRepo::new();
    repo.init();
    enable_auto_push(&repo, &["acme/widget"], false);
    let hooks = repo.path().join("failing-hooks");
    fs::create_dir(&hooks).unwrap();
    let hook = hooks.join("pre-commit");
    fs::write(&hook, "#!/bin/sh\nexit 1\n").unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&hook, fs::Permissions::from_mode(0o755)).unwrap();
    }
    repo.git(&["config", "core.hooksPath", hooks.to_str().unwrap()]);
    let (_fixture_dir, fixture) = auto_push_fixture();
    let env = [("ORBIT_GITHUB_FIXTURE", fixture.to_str().unwrap())];

    let add = repo.ok_json_with_env(&["add", "Commit fails", "--json"], &env);
    let warnings = add["meta"]["warnings"].as_array().unwrap();
    assert!(
        warnings
            .iter()
            .any(|warning| warning.as_str().unwrap().starts_with("COMMIT_FAILED:")),
        "{warnings:?}"
    );
    assert!(
        warnings
            .iter()
            .any(|warning| warning.as_str().unwrap().contains("auto-push skipped")),
        "{warnings:?}"
    );
    let fixture_json: serde_json::Value =
        serde_json::from_slice(&fs::read(&fixture).unwrap()).unwrap();
    assert!(fixture_json["issues"].as_object().unwrap().is_empty());
}

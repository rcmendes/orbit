mod common;

use std::fs;

use common::TestRepo;
use serde_json::json;
use tempfile::TempDir;

fn write_github_config(repo: &TestRepo) {
    let path = repo.board_path().join("config.toml");
    let mut config = fs::read_to_string(&path).unwrap();
    config.push_str("\n[github]\norganization = \"acme\"\nrepositories = [\"acme/widget\"]\n");
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
    "acme/widget#7": {
      "repo": "acme/widget",
      "number": 7,
      "title": "GitHub title",
      "body": "GitHub body",
      "state": "open",
      "labels": ["needs-triage"],
      "assignees": [],
      "updated_at": "2026-10-23T13:00:00Z"
    }
  }
}"#,
    )
    .unwrap();
    path
}

fn adopt_linked(repo: &TestRepo, fixture: &std::path::Path) -> String {
    write_github_config(repo);
    repo.ok_json(&["github", "repo", "link", "acme/widget", "--json"]);
    let id = repo.add("Triage me", &["--label", "needs-triage"]);
    repo.ok_json_with_env(
        &[
            "github",
            "adopt",
            id.as_str(),
            "--repo",
            "acme/widget",
            "--number",
            "7",
            "--json",
        ],
        &[("ORBIT_GITHUB_FIXTURE", fixture.to_str().unwrap())],
    );
    id
}

fn adopt_basic_linked(repo: &TestRepo, fixture: &std::path::Path) -> String {
    let config_path = repo.board_path().join("config.toml");
    let mut config = fs::read_to_string(&config_path).unwrap();
    config.push_str("\n[github]\nmode = \"basic\"\nrepositories = [\"personal/widget\"]\n");
    fs::write(config_path, config).unwrap();
    let fixture_content = fs::read_to_string(fixture)
        .unwrap()
        .replace("acme/widget#7", "personal/widget#7")
        .replace("acme/widget", "personal/widget");
    fs::write(fixture, fixture_content).unwrap();
    repo.ok_json(&["github", "repo", "link", "personal/widget", "--json"]);
    let id = repo.add("Triage me", &["--label", "needs-triage"]);
    repo.ok_json_with_env(
        &[
            "github",
            "adopt",
            id.as_str(),
            "--repo",
            "personal/widget",
            "--number",
            "7",
            "--json",
        ],
        &[("ORBIT_GITHUB_FIXTURE", fixture.to_str().unwrap())],
    );
    id
}

#[test]
fn triage_context_requires_github_link() {
    let repo = TestRepo::new();
    repo.init();
    let id = repo.add("Local only", &[]);
    let (status, value) = repo.json(&["triage", "context", id.as_str(), "--json"]);
    assert_eq!(status, 1);
    assert_eq!(value["error"]["code"], "TRIAGE_NOT_LINKED");
}

#[test]
fn triage_context_includes_guidance_and_canonical_labels() {
    let fixture_dir = tempfile::tempdir().unwrap();
    let fixture = write_fixture(&fixture_dir);
    let repo = TestRepo::new();
    repo.init();
    let id = adopt_linked(&repo, &fixture);

    let guidance_dir = repo.path().join("docs/agents");
    fs::create_dir_all(&guidance_dir).unwrap();
    fs::write(
        guidance_dir.join("project-guidance.md"),
        "## Work areas (`area`)\n\nShared guidance for agents.\n",
    )
    .unwrap();

    let ctx = repo.ok_json_with_env(
        &["triage", "context", id.as_str(), "--json"],
        &[("ORBIT_GITHUB_FIXTURE", fixture.to_str().unwrap())],
    );
    assert_eq!(ctx["data"]["id"], id);
    assert!(
        ctx["data"]["project_guidance"]
            .as_str()
            .unwrap()
            .contains("Work areas")
    );
    let labels = ctx["data"]["canonical_labels"].as_array().unwrap();
    assert!(labels.iter().any(|v| v == "needs-triage"));
    assert!(labels.iter().any(|v| v == "bug"));
}

#[test]
fn triage_review_previews_proposed_metadata_without_applying() {
    let fixture_dir = tempfile::tempdir().unwrap();
    let fixture = write_fixture(&fixture_dir);
    let repo = TestRepo::new();
    repo.init();
    let id = adopt_linked(&repo, &fixture);

    let proposal = repo.path().join("proposal.json");
    fs::write(
        &proposal,
        r#"{
  "rationale": "CLI-facing workflow",
  "area": "cli",
  "labels_add": ["bug"],
  "labels_remove": ["needs-triage"]
}"#,
    )
    .unwrap();

    let review = repo.ok_json_with_env(
        &[
            "triage",
            "review",
            id.as_str(),
            "--proposal-file",
            proposal.to_str().unwrap(),
            "--json",
        ],
        &[("ORBIT_GITHUB_FIXTURE", fixture.to_str().unwrap())],
    );
    assert_eq!(review["data"]["apply_ready"], true);
    assert_eq!(review["data"]["changes"]["area"]["to"], "cli");
    assert_eq!(review["data"]["changes"]["labels"]["add"], json!(["bug"]));

    let show = repo.ok_json(&["show", id.as_str(), "--json"]);
    assert!(show["data"]["area"].is_null());
    assert_eq!(show["data"]["labels"], json!(["needs-triage"]));
}

#[test]
fn triage_review_reads_proposal_from_stdin_without_applying_in_json_mode() {
    let fixture_dir = tempfile::tempdir().unwrap();
    let fixture = write_fixture(&fixture_dir);
    let repo = TestRepo::new();
    repo.init();
    let id = adopt_linked(&repo, &fixture);
    let before = repo.read_task(&id);

    let (status, review) = repo.json_with_stdin(
        &[
            "triage",
            "review",
            id.as_str(),
            "--proposal-file",
            "-",
            "--json",
        ],
        r#"{"area":"cli","labels_add":["bug"],"labels_remove":["needs-triage"]}"#,
    );

    assert_eq!(status, 0, "{review}");
    assert_eq!(review["data"]["changes"]["area"]["to"], "cli");
    assert_eq!(review["data"]["changes"]["labels"]["add"], json!(["bug"]));
    assert_eq!(repo.read_task(&id), before);
}

#[test]
fn triage_review_renders_field_diff_and_does_not_prompt_without_a_tty() {
    let fixture_dir = tempfile::tempdir().unwrap();
    let fixture = write_fixture(&fixture_dir);
    let repo = TestRepo::new();
    repo.init();
    let id = adopt_linked(&repo, &fixture);
    let proposal = repo.path().join("proposal.json");
    fs::write(
        &proposal,
        r#"{
  "area": "cli",
  "verification": "cargo test triage",
  "labels_add": ["bug"],
  "labels_remove": ["needs-triage"]
}"#,
    )
    .unwrap();
    let before = repo.read_task(&id);
    let github_before = fs::read(&fixture).unwrap();

    let output = repo.orbit(&[
        "triage",
        "review",
        id.as_str(),
        "--proposal-file",
        proposal.to_str().unwrap(),
    ]);

    assert!(output.status.success());
    assert!(output.stderr.is_empty());
    let preview = String::from_utf8_lossy(&output.stdout);
    assert!(
        !preview.contains("Apply these Orbit changes?"),
        "non-TTY review must not prompt: {preview}"
    );
    assert!(preview.contains("Area: (unset) -> cli"), "{preview}");
    assert!(
        preview.contains("Verification: (unset) -> cargo test triage"),
        "{preview}"
    );
    assert!(preview.contains("Labels added: bug"), "{preview}");
    assert!(
        preview.contains("Labels removed: needs-triage"),
        "{preview}"
    );
    assert_eq!(repo.read_task(&id), before);
    assert_eq!(fs::read(&fixture).unwrap(), github_before);
}

#[test]
fn triage_review_rejects_non_canonical_labels() {
    let fixture_dir = tempfile::tempdir().unwrap();
    let fixture = write_fixture(&fixture_dir);
    let repo = TestRepo::new();
    repo.init();
    let id = adopt_linked(&repo, &fixture);
    let proposal = repo.path().join("proposal.json");
    fs::write(&proposal, r#"{"labels_add": ["custom-label"]}"#).unwrap();

    let (status, value) = repo.json_with_env(
        &[
            "triage",
            "review",
            id.as_str(),
            "--proposal-file",
            proposal.to_str().unwrap(),
            "--json",
        ],
        &[("ORBIT_GITHUB_FIXTURE", fixture.to_str().unwrap())],
    );
    assert_eq!(status, 1);
    assert_eq!(value["error"]["code"], "TRIAGE_INVALID_PROPOSAL");
}

#[test]
fn triage_review_rejects_forbidden_github_owned_fields() {
    let fixture_dir = tempfile::tempdir().unwrap();
    let fixture = write_fixture(&fixture_dir);
    let repo = TestRepo::new();
    repo.init();
    let id = adopt_linked(&repo, &fixture);
    let proposal = repo.path().join("proposal.json");
    fs::write(&proposal, r#"{"title": "new title"}"#).unwrap();

    let (status, value) = repo.json_with_env(
        &[
            "triage",
            "review",
            id.as_str(),
            "--proposal-file",
            proposal.to_str().unwrap(),
            "--json",
        ],
        &[("ORBIT_GITHUB_FIXTURE", fixture.to_str().unwrap())],
    );
    assert_eq!(status, 1);
    assert_eq!(value["error"]["code"], "TRIAGE_INVALID_PROPOSAL");
}

#[test]
fn triage_apply_requires_explicit_approval() {
    let fixture_dir = tempfile::tempdir().unwrap();
    let fixture = write_fixture(&fixture_dir);
    let repo = TestRepo::new();
    repo.init();
    let id = adopt_linked(&repo, &fixture);
    let proposal = repo.path().join("proposal.json");
    fs::write(&proposal, r#"{"area": "cli"}"#).unwrap();

    let (status, value) = repo.json_with_env(
        &[
            "triage",
            "apply",
            id.as_str(),
            "--proposal-file",
            proposal.to_str().unwrap(),
            "--json",
        ],
        &[("ORBIT_GITHUB_FIXTURE", fixture.to_str().unwrap())],
    );
    assert_eq!(status, 1);
    assert_eq!(value["error"]["code"], "TRIAGE_APPROVAL_REQUIRED");
}

#[test]
fn triage_apply_writes_orbit_only_and_preserves_github_title() {
    let fixture_dir = tempfile::tempdir().unwrap();
    let fixture = write_fixture(&fixture_dir);
    let repo = TestRepo::new();
    repo.init();
    let id = adopt_linked(&repo, &fixture);
    let proposal = repo.path().join("proposal.json");
    fs::write(
        &proposal,
        r#"{
  "area": "cli",
  "verification": "cargo test triage",
  "labels_add": ["ready-for-agent"],
  "labels_remove": ["needs-triage"]
}"#,
    )
    .unwrap();

    let applied = repo.ok_json_with_env(
        &[
            "triage",
            "apply",
            id.as_str(),
            "--proposal-file",
            proposal.to_str().unwrap(),
            "--approve",
            "--json",
        ],
        &[("ORBIT_GITHUB_FIXTURE", fixture.to_str().unwrap())],
    );
    assert_eq!(applied["data"]["applied"], true);
    assert!(applied["data"]["sync_required"].as_bool().unwrap());

    let show = repo.ok_json(&["show", id.as_str(), "--json"]);
    assert_eq!(show["data"]["area"], "cli");
    assert_eq!(show["data"]["verification"], "cargo test triage");
    assert_eq!(show["data"]["labels"], json!(["ready-for-agent"]));
    assert_eq!(show["data"]["title"], "GitHub title");
}

#[test]
fn triage_uncertainty_without_changes_leaves_metadata_unchanged() {
    let fixture_dir = tempfile::tempdir().unwrap();
    let fixture = write_fixture(&fixture_dir);
    let repo = TestRepo::new();
    repo.init();
    let id = adopt_linked(&repo, &fixture);
    let proposal = repo.path().join("proposal.json");
    fs::write(
        &proposal,
        r#"{"uncertainty": "Not enough detail in the issue body to classify area."}"#,
    )
    .unwrap();

    let review = repo.ok_json_with_env(
        &[
            "triage",
            "review",
            id.as_str(),
            "--proposal-file",
            proposal.to_str().unwrap(),
            "--json",
        ],
        &[("ORBIT_GITHUB_FIXTURE", fixture.to_str().unwrap())],
    );
    assert_eq!(review["data"]["apply_ready"], false);
    assert_eq!(review["data"]["changes"], json!({}));

    let (status, value) = repo.json_with_env(
        &[
            "triage",
            "apply",
            id.as_str(),
            "--proposal-file",
            proposal.to_str().unwrap(),
            "--approve",
            "--json",
        ],
        &[("ORBIT_GITHUB_FIXTURE", fixture.to_str().unwrap())],
    );
    assert_eq!(status, 1);
    assert_eq!(value["error"]["code"], "TRIAGE_INVALID_PROPOSAL");
}

#[test]
fn basic_mode_triage_exposes_only_label_capability() {
    let fixture_dir = tempfile::tempdir().unwrap();
    let fixture = write_fixture(&fixture_dir);
    let repo = TestRepo::new();
    repo.init();
    let id = adopt_basic_linked(&repo, &fixture);

    let context = repo.ok_json_with_env(
        &["triage", "context", id.as_str(), "--json"],
        &[("ORBIT_GITHUB_FIXTURE", fixture.to_str().unwrap())],
    );
    assert_eq!(context["data"]["metadata_capabilities"]["area"], false);
    assert_eq!(
        context["data"]["metadata_capabilities"]["verification"],
        false
    );
    assert_eq!(context["data"]["metadata_capabilities"]["labels"], true);

    let proposal = repo.path().join("proposal.json");
    fs::write(&proposal, r#"{"area":"cli","labels_add":["bug"]}"#).unwrap();
    let (status, value) = repo.json_with_env(
        &[
            "triage",
            "review",
            id.as_str(),
            "--proposal-file",
            proposal.to_str().unwrap(),
            "--json",
        ],
        &[("ORBIT_GITHUB_FIXTURE", fixture.to_str().unwrap())],
    );
    assert_eq!(status, 1);
    assert_eq!(value["error"]["code"], "INVALID_ARGUMENT");
    assert!(
        value["error"]["message"]
            .as_str()
            .unwrap()
            .contains("labels only")
    );

    fs::write(&proposal, r#"{"labels_add":["bug"]}"#).unwrap();
    let applied = repo.ok_json_with_env(
        &[
            "triage",
            "apply",
            id.as_str(),
            "--proposal-file",
            proposal.to_str().unwrap(),
            "--approve",
            "--json",
        ],
        &[("ORBIT_GITHUB_FIXTURE", fixture.to_str().unwrap())],
    );
    assert_eq!(applied["data"]["applied"], true);
    assert_eq!(applied["data"]["sync_required"], true);
}

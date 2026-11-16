//! Profiles (ADR 0022): `orbit setup <plugin>` resolves a profile TOML —
//! shipped, Board-local, project, or user-level — negotiates its
//! `requires`/`contract` against this binary, and applies it to
//! `config.toml` idempotently. `orbit doctor` re-checks applied profiles.

mod common;

use common::TestRepo;

/// Writes `profiles/<name>.toml` under the isolated user home —
/// `~/.config/orbit/profiles/` is the lowest-priority search root.
fn user_profile(repo: &TestRepo, name: &str, toml: &str) {
    let dir = repo.home().join(".config").join("orbit").join("profiles");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join(format!("{name}.toml")), toml).unwrap();
}

fn board_config(repo: &TestRepo) -> String {
    std::fs::read_to_string(repo.board_path().join("config.toml")).unwrap()
}

#[test]
fn setup_applies_a_profile_idempotently() {
    let repo = TestRepo::new();
    repo.init();
    user_profile(
        &repo,
        "acme",
        r#"
name = "acme"
description = "Acme plugin profile"
contract = "1.0.0"
requires = ["workflow", "links"]
optional = ["remote"]
holds = ["needs-info"]

[roles]
reviewer = "human"

[vocabulary.status]
triage = "in_review"

[workflow]
template = "minimal"

[board]
components = ["api"]
"#,
    );

    let applied = repo.ok_json(&["setup", "acme", "--json"]);
    assert_eq!(applied["data"]["profile"]["name"], "acme");
    assert_eq!(applied["data"]["profile"]["source"], "user");
    assert_eq!(applied["data"]["changed"], true);

    let config = board_config(&repo);
    assert!(config.contains("template = \"minimal\""), "{config}");
    assert!(config.contains("components = [\"api\"]"), "{config}");
    assert!(config.contains("needs-info"), "{config}");
    assert!(config.contains("[[profiles]]"), "{config}");
    assert!(config.contains("name = \"acme\""), "{config}");
    assert!(config.contains("[profiles.vocabulary.status]"), "{config}");

    // Re-applying is a no-op: same config, no new commit.
    let again = repo.ok_json(&["setup", "acme", "--json"]);
    assert_eq!(again["data"]["changed"], false, "{again}");
    assert_eq!(again["data"]["committed"], false, "{again}");
    assert_eq!(board_config(&repo), config);

    // The handshake reports the applied profile and the capability set.
    let caps = repo.ok_json(&["capabilities", "--json"]);
    assert_eq!(caps["data"]["profiles"], serde_json::json!(["acme"]));
    let capabilities = caps["data"]["capabilities"].as_array().unwrap();
    assert!(capabilities.iter().any(|c| c == "workflow"));
    assert!(capabilities.iter().any(|c| c == "profiles"));
}

#[test]
fn setup_rejects_a_profile_with_unmet_requirements() {
    let repo = TestRepo::new();
    repo.init();
    user_profile(
        &repo,
        "future",
        r#"
name = "future"
requires = ["workflow", "teleport"]
contract = "99.0.0"
"#,
    );

    let (status, value) = repo.json(&["setup", "future", "--json"]);
    assert_ne!(status, 0, "{value}");
    assert_eq!(value["ok"], false, "{value}");
    assert_eq!(value["error"]["code"], "PROFILE_UNMET", "{value}");
    let details = &value["error"]["details"];
    assert_eq!(
        details["missing_capabilities"],
        serde_json::json!(["teleport"])
    );
    assert_eq!(details["required_contract"], "99.0.0");
    // Nothing was written: no [[profiles]] record, default workflow kept.
    let config = board_config(&repo);
    assert!(!config.contains("[[profiles]]"), "{config}");
}

#[test]
fn doctor_reports_a_profile_with_unmet_requirements() {
    let repo = TestRepo::new();
    repo.init();
    // Hand-write an applied profile record the current binary cannot meet,
    // as if applied by a newer Orbit or edited by hand.
    let config_path = repo.board_path().join("config.toml");
    let mut config = std::fs::read_to_string(&config_path).unwrap();
    config.push_str(
        "\n[[profiles]]\nname = \"ghost\"\nrequires = [\"teleport\"]\ncontract = \"99.0.0\"\n",
    );
    std::fs::write(&config_path, config).unwrap();

    let (status, value) = repo.json(&["doctor", "--json"]);
    assert!(status == 0 || status == 1, "{value}");
    let issues = value["data"]["issues"].as_array().unwrap();
    let profile_issue = issues
        .iter()
        .find(|issue| issue["code"] == "PROFILE_UNMET")
        .unwrap_or_else(|| panic!("no PROFILE_UNMET issue in {value}"));
    assert!(profile_issue["message"].as_str().unwrap().contains("ghost"));
    assert!(
        profile_issue["message"]
            .as_str()
            .unwrap()
            .contains("teleport")
    );
    assert_eq!(value["data"]["healthy"], false, "{value}");
}

#[test]
fn doctor_is_quiet_when_profile_requirements_are_met() {
    let repo = TestRepo::new();
    repo.init();
    user_profile(
        &repo,
        "acme",
        "name = \"acme\"\nrequires = [\"workflow\"]\n",
    );
    repo.ok_json(&["setup", "acme", "--json"]);

    let value = repo.ok_json(&["doctor", "--json"]);
    let codes: Vec<&str> = value["data"]["issues"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|issue| issue["code"].as_str())
        .collect();
    assert!(!codes.contains(&"PROFILE_UNMET"), "{value}");
}

#[test]
fn setup_unknown_target_names_integrations_and_profiles() {
    let repo = TestRepo::new();
    repo.init();
    let (status, value) = repo.json(&["setup", "nosuchplugin", "--json"]);
    assert_ne!(status, 0, "{value}");
    assert_eq!(value["error"]["code"], "INVALID_ARGUMENT", "{value}");
    let message = value["error"]["message"].as_str().unwrap();
    assert!(message.contains("claude"), "{message}");
    assert!(message.contains("profiles"), "{message}");
}

#[test]
fn setup_check_and_print_preview_the_profile() {
    let repo = TestRepo::new();
    repo.init();
    user_profile(
        &repo,
        "acme",
        "name = \"acme\"\n[workflow]\ntemplate = \"agentic\"\n",
    );

    let (status, check) = repo.json(&["setup", "acme", "--check", "--json"]);
    assert_eq!(status, 1, "{check}");
    assert_eq!(check["data"]["status"], "stale", "{check}");

    let preview = repo.ok_json(&["setup", "acme", "--print", "--json"]);
    assert_eq!(preview["data"]["preview"], true, "{preview}");
    let files = preview["data"]["files"].as_array().unwrap();
    let config = files
        .iter()
        .find(|file| file["path"] == "config.toml")
        .unwrap_or_else(|| panic!("no config.toml preview in {preview}"));
    assert!(config["content"].as_str().unwrap().contains("agentic"));
    // The preview wrote nothing.
    assert!(!board_config(&repo).contains("[[profiles]]"));

    repo.ok_json(&["setup", "acme", "--json"]);
    let (status, check) = repo.json(&["setup", "acme", "--check", "--json"]);
    assert_eq!(status, 0, "{check}");
    assert_eq!(check["data"]["status"], "installed", "{check}");
}

#[test]
fn setup_accepts_an_explicit_profile_path() {
    let repo = TestRepo::new();
    repo.init();
    let profiles = repo.path().join("profiles");
    std::fs::create_dir_all(&profiles).unwrap();
    std::fs::write(
        profiles.join("local.toml"),
        "name = \"local\"\n[workflow]\ntemplate = \"minimal\"\n",
    )
    .unwrap();

    let applied = repo.ok_json(&["setup", "profiles/local.toml", "--json"]);
    assert_eq!(applied["data"]["profile"]["name"], "local");
    assert!(
        applied["data"]["profile"]["source"]
            .as_str()
            .unwrap()
            .starts_with("file:"),
        "{applied}"
    );
    assert!(board_config(&repo).contains("template = \"minimal\""));
}

#[test]
fn setup_applies_a_profile_alongside_a_known_integration() {
    let repo = TestRepo::new();
    repo.init();
    // A profile named after an agent integration composes: the harness
    // files install and the profile applies to the Board.
    user_profile(
        &repo,
        "mattpocock",
        "name = \"mattpocock\"\nrequires = [\"workflow\"]\n[workflow]\ntemplate = \"agentic\"\n",
    );

    let applied = repo.ok_json(&["setup", "mattpocock", "--json"]);
    assert_eq!(applied["data"]["profile"]["name"], "mattpocock");
    assert!(repo.path().join("docs/agents/issue-tracker.md").is_file());
    let config = board_config(&repo);
    assert!(config.contains("template = \"agentic\""), "{config}");
    assert!(config.contains("name = \"mattpocock\""), "{config}");
}

#[test]
fn setup_remove_rejects_a_profile_only_target() {
    let repo = TestRepo::new();
    repo.init();
    user_profile(&repo, "acme", "name = \"acme\"\n");
    let (status, value) = repo.json(&["setup", "acme", "--remove", "--json"]);
    assert_ne!(status, 0, "{value}");
    assert!(
        value["error"]["message"]
            .as_str()
            .unwrap()
            .contains("declarative")
    );
}

#[test]
fn profile_resolution_prefers_board_local_over_user() {
    let repo = TestRepo::new();
    repo.init();
    // Same profile name in the Board repository and the user home: the
    // Board-local copy wins.
    let board_profiles = repo.board_path().join("profiles");
    std::fs::create_dir_all(&board_profiles).unwrap();
    std::fs::write(
        board_profiles.join("acme.toml"),
        "name = \"acme\"\ndescription = \"board copy\"\n",
    )
    .unwrap();
    user_profile(
        &repo,
        "acme",
        "name = \"acme\"\ndescription = \"user copy\"\n",
    );

    let applied = repo.ok_json(&["setup", "acme", "--json"]);
    assert_eq!(applied["data"]["profile"]["source"], "board", "{applied}");
    let config = board_config(&repo);
    assert!(config.contains("board copy"), "{config}");
}

#[test]
fn shipped_workflow_templates_apply_through_profiles() {
    // minimal, standard, and agentic are the templates Orbit ships; each
    // must resolve through a profile's [workflow] section.
    for template in ["minimal", "standard", "agentic"] {
        let repo = TestRepo::new();
        repo.init();
        user_profile(
            &repo,
            "plug",
            &format!("name = \"plug\"\n[workflow]\ntemplate = \"{template}\"\n"),
        );
        repo.ok_json(&["setup", "plug", "--json"]);
        let config = board_config(&repo);
        assert!(
            config.contains(&format!("template = \"{template}\"")),
            "{template}: {config}"
        );
    }
}

#[test]
fn profile_without_a_board_fails_with_board_not_found() {
    let repo = TestRepo::new();
    // No `init`: profile resolution still finds the user profile, but
    // applying it needs a Board.
    user_profile(&repo, "acme", "name = \"acme\"\n");
    let (status, value) = repo.json(&["setup", "acme", "--json"]);
    assert_ne!(status, 0, "{value}");
    assert_eq!(value["error"]["code"], "BOARD_NOT_FOUND", "{value}");
}

#[test]
fn capabilities_lists_supported_capabilities() {
    let repo = TestRepo::new();
    repo.init();
    let caps = repo.ok_json(&["capabilities", "--json"]);
    let capabilities = caps["data"]["capabilities"].as_array().unwrap();
    for expected in [
        "workflow",
        "issue-types",
        "hierarchy",
        "priorities",
        "links",
        "components",
        "properties",
        "external-id",
        "assignee",
        "reporter",
        "lease",
        "keys",
        "idempotency",
        "batch",
        "remote",
        "workspace",
        "mcp",
        "setup",
        "profiles",
        "doctor",
    ] {
        assert!(
            capabilities.iter().any(|c| c == expected),
            "missing capability {expected} in {capabilities:?}"
        );
    }
}

#[test]
fn profile_path_outside_search_roots_is_not_found() {
    let repo = TestRepo::new();
    repo.init();
    let (status, value) = repo.json(&["setup", "missing.toml", "--json"]);
    assert_ne!(status, 0, "{value}");
    assert_eq!(value["error"]["code"], "NOT_FOUND", "{value}");
}

#[test]
fn orbit_dir_profile_is_found() {
    let repo = TestRepo::new();
    repo.init();
    // `.orbit/profiles/` sits beside the Board folder inside the Orbit
    // folder — shared by every checkout of the same anchor.
    let dir = repo.orbit_path().join("profiles");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(
        dir.join("anchored.toml"),
        "name = \"anchored\"\n[workflow]\ntemplate = \"minimal\"\n",
    )
    .unwrap();
    let applied = repo.ok_json(&["setup", "anchored", "--json"]);
    assert_eq!(applied["data"]["profile"]["source"], "orbit", "{applied}");
    assert!(board_config(&repo).contains("template = \"minimal\""));
}

#[test]
fn board_local_profile_directory_is_found() {
    let repo = TestRepo::new();
    repo.init();
    let dir = repo.board_path().join("profiles");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(
        dir.join("inboard.toml"),
        "name = \"inboard\"\n[workflow]\ntemplate = \"minimal\"\n",
    )
    .unwrap();
    let applied = repo.ok_json(&["setup", "inboard", "--json"]);
    assert_eq!(applied["data"]["profile"]["source"], "board", "{applied}");
}

#[test]
fn unmet_optional_capabilities_do_not_block() {
    let repo = TestRepo::new();
    repo.init();
    user_profile(
        &repo,
        "acme",
        "name = \"acme\"\nrequires = [\"workflow\"]\noptional = [\"teleport\", \"review\"]\n",
    );
    repo.ok_json(&["setup", "acme", "--json"]);
    let config = board_config(&repo);
    assert!(config.contains("teleport"), "{config}");
}

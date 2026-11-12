mod common;

use common::TestRepo;

#[test]
fn configured_github_board_loads_with_the_builtin_provider() {
    let repo = TestRepo::new();
    repo.init();
    let config = repo.board_path().join("config.toml");
    let mut contents = std::fs::read_to_string(&config).unwrap();
    contents.push_str("\n[github]\nmode = \"basic\"\nrepositories = [\"alice/widget\"]\n");
    std::fs::write(config, contents).unwrap();

    // GitHub is a built-in Remote Tracker adapter (ADR 0018): a configured
    // Board loads normally and `orbit remote` commands are always available.
    let (status, output) = repo.json(&["list", "--json"]);
    assert_eq!(status, 0, "{output}");
    assert_eq!(output["ok"], true);

    let status_output = repo.json(&["remote", "status", "--json"]);
    assert_eq!(status_output.0, 0, "{}", status_output.1);
}

#[test]
fn binary_help_exposes_remote_and_triage_commands() {
    let output = TestRepo::new().orbit(&["--help"]);
    assert!(output.status.success());
    let help = String::from_utf8_lossy(&output.stdout);
    assert!(help.contains("remote"), "{help}");
    assert!(help.contains("triage"), "{help}");
    assert!(!help.contains("github"), "{help}");

    // `orbit github` is gone for good: no alias, no compat flag.
    let rejected = TestRepo::new().orbit(&["github", "status"]);
    assert!(!rejected.status.success());
    let stderr = String::from_utf8_lossy(&rejected.stderr);
    assert!(stderr.contains("unrecognized subcommand"), "{stderr}");
}

#[test]
fn github_is_builtin_not_a_cargo_feature() {
    let manifest: toml::Value = toml::from_str(include_str!("../Cargo.toml")).unwrap();
    let features = manifest
        .get("features")
        .and_then(toml::Value::as_table)
        .cloned()
        .unwrap_or_default();
    assert!(
        !features.contains_key("github-provider"),
        "GitHub sync is built in; the github-provider feature gate is gone"
    );

    let dist: toml::Value = toml::from_str(include_str!("../dist-workspace.toml")).unwrap();
    let installers = dist["dist"]["installers"]
        .as_array()
        .expect("cargo-dist installers are configured");
    for installer in ["shell", "powershell", "npm"] {
        assert!(
            installers
                .iter()
                .any(|configured| configured.as_str() == Some(installer)),
            "release artifacts must include the {installer} installer"
        );
    }
}

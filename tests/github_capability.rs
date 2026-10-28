mod common;

use common::TestRepo;

#[test]
fn configured_github_board_requires_the_provider_capability() {
    let repo = TestRepo::new();
    repo.init();
    let config = repo.board_path().join("config.toml");
    let mut contents = std::fs::read_to_string(&config).unwrap();
    contents.push_str("\n[github]\nmode = \"basic\"\nrepositories = [\"alice/widget\"]\n");
    std::fs::write(config, contents).unwrap();

    let (status, output) = repo.json(&["list", "--json"]);

    if cfg!(feature = "github-provider") {
        assert_eq!(status, 0, "{output}");
        assert_eq!(output["ok"], true);
    } else {
        assert_eq!(status, 1, "{output}");
        assert_eq!(output["error"]["code"], "GITHUB_UNSUPPORTED");
        let message = output["error"]["message"].as_str().unwrap();
        assert!(message.contains("0.9.0"), "{message}");
        assert!(message.contains("upgrade"), "{message}");

        let human = repo.orbit(&["list"]);
        assert_eq!(human.status.code(), Some(1));
        let stderr = String::from_utf8_lossy(&human.stderr);
        assert!(stderr.contains("GITHUB_UNSUPPORTED"), "{stderr}");
        assert!(stderr.contains("0.9.0"), "{stderr}");
        assert!(stderr.contains("upgrade"), "{stderr}");
    }
}

#[test]
fn binary_help_exposes_github_and_triage_commands() {
    let output = TestRepo::new().orbit(&["--help"]);
    assert!(output.status.success());
    let help = String::from_utf8_lossy(&output.stdout);
    assert!(help.contains("github"), "{help}");
    assert!(help.contains("triage"), "{help}");
}

#[test]
fn release_metadata_keeps_the_provider_in_default_install_artifacts() {
    let manifest: toml::Value = toml::from_str(include_str!("../Cargo.toml")).unwrap();
    let default_features = manifest["features"]["default"]
        .as_array()
        .expect("Cargo package declares default features");
    assert!(
        default_features
            .iter()
            .any(|feature| feature.as_str() == Some("github-provider")),
        "default Cargo installs must include github-provider"
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

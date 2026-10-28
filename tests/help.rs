use clap::CommandFactory;
use orbit_tracker::cli::Cli;

fn collect_missing_examples(command: &clap::Command, path: &str, missing: &mut Vec<String>) {
    for subcommand in command.get_subcommands() {
        if subcommand.get_name() == "help" {
            continue;
        }

        let name = format!("{path} {}", subcommand.get_name());
        let mut subcommand = subcommand.clone();
        let help = subcommand.render_long_help().to_string();
        if !help.contains("Examples:") {
            missing.push(name.clone());
        }

        collect_missing_examples(&subcommand, &name, missing);
    }
}

#[test]
fn every_subcommand_documents_examples() {
    let mut root = Cli::command();
    root.build();

    let mut missing = Vec::new();
    collect_missing_examples(&root, "orbit", &mut missing);
    assert!(
        missing.is_empty(),
        "subcommands without an Examples block: {missing:?}"
    );
}

#[test]
fn init_help_describes_the_non_interactive_preview() {
    let output = std::process::Command::new(env!("CARGO_BIN_EXE_orbit"))
        .args(["init", "--help"])
        .output()
        .unwrap();
    assert!(output.status.success());
    let help = String::from_utf8(output.stdout).unwrap();
    let help = help.split_whitespace().collect::<Vec<_>>().join(" ");
    assert!(help.contains("--print"), "{help}");
    assert!(help.contains("never prompts"), "{help}");
    assert!(
        help.contains("without a terminal it writes no agent files"),
        "{help}"
    );
}

#[test]
fn root_help_mentions_the_agent_loop() {
    let help = Cli::command().render_long_help().to_string();
    assert!(help.contains("orbit ready --claim --by"));
    assert!(help.contains("orbit heartbeat"));
    assert!(help.contains("orbit done"));
}

#[test]
fn init_and_setup_explain_the_portable_mcp_choice() {
    let mut command = Cli::command();
    command.build();
    for (name, example) in [
        ("init", "orbit init --portable-mcp"),
        ("setup", "orbit setup claude --portable-mcp"),
    ] {
        let mut subcommand = command
            .find_subcommand(name)
            .unwrap_or_else(|| panic!("missing subcommand {name}"))
            .clone();
        let help = subcommand.render_long_help().to_string();

        assert!(help.contains("--portable-mcp"), "{name}: {help}");
        assert!(help.contains("absolute path"), "{name}: {help}");
        assert!(help.contains("PATH"), "{name}: {help}");
        assert!(help.contains(example), "{name}: {help}");
    }
}

#[test]
fn workspace_issue_import_help_explains_review_apply_and_existing_task_links() {
    let output = std::process::Command::new(env!("CARGO_BIN_EXE_orbit"))
        .args(["workspace", "issues", "import", "--help"])
        .output()
        .unwrap();
    assert!(output.status.success());
    let help = String::from_utf8(output.stdout).unwrap();
    assert!(help.contains("--apply"), "{help}");
    assert!(help.contains("--link"), "{help}");
    assert!(help.contains("default is a preview"), "{help}");
    assert!(help.contains("acme/api#42=orb-a7b2c9"), "{help}");
}

#[test]
fn workspace_setup_help_explains_selection_and_reconciliation() {
    let output = std::process::Command::new(env!("CARGO_BIN_EXE_orbit"))
        .args(["workspace", "setup", "--help"])
        .output()
        .unwrap();
    assert!(output.status.success());
    let help = String::from_utf8(output.stdout).unwrap();
    assert!(help.contains("--agents"), "{help}");
    assert!(help.contains("--print"), "{help}");
    assert!(help.contains("--remove"), "{help}");
    assert!(
        help.contains("Matt Pocock skills are detected separately"),
        "{help}"
    );
    assert!(
        help.contains("orbit workspace setup --agents all"),
        "{help}"
    );
}

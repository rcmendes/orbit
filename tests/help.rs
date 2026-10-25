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
    assert!(help.contains("does not prompt by default"), "{help}");
    assert!(help.contains("--agents ask"), "{help}");
}

#[test]
fn root_help_mentions_the_agent_loop() {
    let help = Cli::command().render_long_help().to_string();
    assert!(help.contains("orbit ready --claim --by"));
    assert!(help.contains("orbit heartbeat"));
    assert!(help.contains("orbit done"));
}

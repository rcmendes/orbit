//! `orbit capabilities`: the contract handshake (ADR 0022).
//!
//! Reports the contract version this binary speaks, the contract version the
//! Board declares, whether they are compatible, the optional modules the
//! Board has enabled, and the Board's workflow. It reads `config.toml`
//! leniently — without `Config::validate` — so it still answers on a Board
//! whose contract is newer than this binary's, which is exactly when a
//! consumer needs the handshake.

use std::path::Path;

use serde_json::{Value, json};

use crate::board::{Board, Config};
use crate::contract;
use crate::error::{OrbitError, Result};
use crate::output::CommandOutput;
use crate::workflow::Workflow;

/// `orbit capabilities [--json]`.
pub fn run(dir: Option<&Path>) -> Result<CommandOutput> {
    let mut discovery = Board::discovery(dir);
    if let Some(error) = discovery.error.take() {
        return Err(error);
    }
    let Some(root) = discovery.root.clone() else {
        return Err(OrbitError::board_not_found());
    };
    let config = Config::load_lenient(&root)?;

    let board_contract = config.contract.clone();
    let compatible = contract::is_compatible(&board_contract);
    let workflow = workflow_summary(&config);
    let modules = modules(&config);

    let data = json!({
        "orbit_version": env!("CARGO_PKG_VERSION"),
        "contract": contract::VERSION,
        "board": {
            "contract": board_contract,
            "compatible": compatible,
            "prefix": config.id.prefix,
        },
        "modules": modules,
        "workflow": workflow,
    });

    let human = human_summary(&data);
    let mut output = CommandOutput::new(data, human);
    if discovery.needs_migration {
        output.warnings.push(
            "the Board uses a pre-ADR-0019 layout; run `orbit migrate` to convert it".to_string(),
        );
    }
    Ok(output)
}

/// The Board's optional modules: configured subsystems outside the frozen
/// kernel. Each entry names the module and carries its key settings so a
/// consumer can negotiate before calling module commands.
fn modules(config: &Config) -> Value {
    let mut modules = Vec::new();
    // `[remote]` projects/adapter and the legacy `[github]` section are one
    // Remote Tracker surface; report it once as the `remote` module.
    let remote_enabled = !config.remote.projects.is_empty() || config.remote.github.is_some();
    let legacy_github = config.github != crate::github::GitHubConfig::default();
    if remote_enabled || legacy_github {
        let mut entry = json!({ "name": "remote" });
        if config.remote.auto_push {
            entry["auto_push"] = json!(true);
        }
        let projects: Vec<Value> = config
            .remote
            .projects
            .iter()
            .map(|project| json!({ "tracker": project.tracker, "project": project.project }))
            .collect();
        if !projects.is_empty() {
            entry["projects"] = json!(projects);
        }
        if config.remote.github.is_some() || legacy_github {
            entry["github_adapter"] = json!(true);
        }
        modules.push(entry);
    }
    if !config.workspace.is_empty() {
        let mut entry = json!({ "name": "workspace" });
        if !config.workspace.include.is_empty() {
            entry["include"] = json!(config.workspace.include);
        }
        if !config.workspace.exclude.is_empty() {
            entry["exclude"] = json!(config.workspace.exclude);
        }
        if !config.workspace.detached.is_empty() {
            entry["detached"] = json!(config.workspace.detached);
        }
        modules.push(entry);
    }
    Value::Array(modules)
}

/// The workflow summary: `null` when the Board's `[workflow]` section does
/// not resolve (for example a template name this binary does not know), so
/// capabilities still answers on a Board written for a newer contract.
fn workflow_summary(config: &Config) -> Value {
    let Ok(workflow) = Workflow::from_config(config) else {
        return Value::Null;
    };
    let source = if config.workflow.statuses.is_empty() {
        "template"
    } else {
        "custom"
    };
    let mut summary = json!({
        "source": source,
        "initial": workflow.initial_status(),
        "statuses": workflow.statuses,
        "transitions": workflow.transitions,
    });
    if source == "template" {
        summary["template"] = json!(config.workflow.template.as_deref().unwrap_or("standard"));
    }
    summary
}

fn human_summary(data: &Value) -> String {
    let mut out = String::new();
    out.push_str(&format!(
        "orbit {} (contract {})\n",
        data["orbit_version"].as_str().unwrap_or("?"),
        data["contract"].as_str().unwrap_or("?")
    ));
    let board = &data["board"];
    let compatible = if board["compatible"].as_bool().unwrap_or(false) {
        "compatible"
    } else {
        "INCOMPATIBLE — upgrade orbit"
    };
    out.push_str(&format!(
        "board contract {} ({compatible})\n",
        board["contract"].as_str().unwrap_or("?")
    ));
    match &data["modules"] {
        Value::Array(modules) if modules.is_empty() => out.push_str("modules: none\n"),
        Value::Array(modules) => {
            let names: Vec<&str> = modules
                .iter()
                .filter_map(|module| module["name"].as_str())
                .collect();
            out.push_str(&format!("modules: {}\n", names.join(", ")));
        }
        _ => {}
    }
    match &data["workflow"] {
        Value::Null => out.push_str("workflow: unresolvable on this binary\n"),
        workflow => {
            let template = workflow["template"].as_str();
            let label = template.unwrap_or("custom");
            let statuses = workflow["statuses"]
                .as_array()
                .map(|statuses| statuses.len())
                .unwrap_or(0);
            let transitions = workflow["transitions"]
                .as_array()
                .map(|transitions| transitions.len())
                .unwrap_or(0);
            out.push_str(&format!(
                "workflow: {label} (initial {}, {statuses} statuses, {transitions} transitions)\n",
                workflow["initial"].as_str().unwrap_or("?")
            ));
        }
    }
    out
}

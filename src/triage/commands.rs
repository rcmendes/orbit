use std::fs;
use std::io::{BufRead, IsTerminal, Read, Write};
use std::path::PathBuf;

use chrono::Utc;
use serde_json::json;

use crate::atomic;
use crate::board::Board;
use crate::cli::{TriageApplyArgs, TriageArgs, TriageCommand, TriageReviewArgs};
use crate::error::{OrbitError, Result, code};
use crate::graph::{Graph, LoadMode};
use crate::task::{self, Task, TaskArea};
use crate::triage::guidance::{guidance_missing_note, read_project_guidance};
use crate::triage::labels::CANONICAL_TRIAGE_LABELS;
use crate::triage::proposal::TriageProposal;

pub fn run(
    board: &Board,
    args: TriageArgs,
    json_mode: bool,
) -> Result<crate::output::CommandOutput> {
    match args.command {
        TriageCommand::Context { task } => cmd_context(board, &task),
        TriageCommand::Review(args) => cmd_review(board, args, json_mode),
        TriageCommand::Apply(args) => cmd_apply(board, args),
    }
}

fn cmd_context(board: &Board, id: &str) -> Result<crate::output::CommandOutput> {
    let (graph, _) = Graph::load(board, LoadMode::Strict)?;
    let task = graph.get(id)?;
    let link = require_github_link(&task)?;

    let (guidance, guidance_path) = read_project_guidance(board);
    let mut warnings = Vec::new();
    if guidance.is_none() {
        if let Some(ref path) = guidance_path {
            warnings.push(guidance_missing_note(path));
        }
    }

    let guidance_path_json = guidance_path
        .as_ref()
        .map(|path| path.display().to_string());

    let areas: Vec<&str> = TaskArea::all().iter().map(|area| area.as_str()).collect();
    let data = json!({
        "id": task.id,
        "github": link,
        "current": {
            "area": task.front.area.map(|area| area.as_str()),
            "verification": task.front.verification,
            "labels": task.front.labels,
        },
        "project_guidance": guidance,
        "project_guidance_path": guidance_path_json,
        "allowed_areas": areas,
        "canonical_labels": CANONICAL_TRIAGE_LABELS,
        "metadata_capabilities": {
            "area": !board.config.github.mode.is_basic(),
            "verification": !board.config.github.mode.is_basic(),
            "labels": true,
        },
    });
    let mut output =
        crate::output::CommandOutput::new(data, format!("Triage context for {id} (GitHub-linked)"));
    output.warnings.extend(warnings);
    Ok(output)
}

fn cmd_review(
    board: &Board,
    args: TriageReviewArgs,
    json_mode: bool,
) -> Result<crate::output::CommandOutput> {
    let proposal = load_proposal(&args.proposal_file)?;
    let (graph, _) = Graph::load(board, LoadMode::Strict)?;
    let task = graph.get(&args.task)?;
    require_github_link(&task)?;

    let (changes, apply_ready) = proposal.build_preview(&task)?;
    reject_basic_structured_changes(board, &changes)?;
    let preview = render_preview(&args.task, &proposal, &changes);
    let data = json!({
        "id": task.id,
        "proposal": {
            "rationale": proposal.rationale,
            "uncertainty": proposal.uncertainty,
        },
        "changes": changes,
        "apply_ready": apply_ready,
        "sync_note": if board.config.github.mode.is_basic() {
            "In basic mode, only labels can sync to GitHub; area and verification require full mode with organization Issue Fields."
        } else {
            "After approval, run `orbit github sync <ID> --direction push --apply` to publish Orbit-owned fields and labels to GitHub."
        },
    });

    let has_changes = changes
        .as_object()
        .is_some_and(|changes| !changes.is_empty());
    if is_interactive_review(json_mode, &args) && apply_ready && has_changes {
        let mut prompt = std::io::stdout().lock();
        write!(prompt, "{preview}")
            .map_err(|error| OrbitError::io("write triage review preview", error))?;
        prompt
            .flush()
            .map_err(|error| OrbitError::io("flush triage review preview", error))?;
        if prompt_for_approval(&mut std::io::stdin().lock(), &mut prompt)? {
            return apply_proposal(board, &args.task, &proposal, task.clone(), changes);
        }
        return Ok(crate::output::CommandOutput::new(
            data,
            format!(
                "Triage review cancelled; no changes applied to {}",
                args.task
            ),
        ));
    }

    Ok(crate::output::CommandOutput::new(data, preview))
}

fn cmd_apply(board: &Board, args: TriageApplyArgs) -> Result<crate::output::CommandOutput> {
    if !args.approve {
        return Err(OrbitError::new(
            code::TRIAGE_APPROVAL_REQUIRED,
            "triage apply requires explicit approval; pass --approve after reviewing the proposal",
        ));
    }

    let proposal = load_proposal(&args.proposal_file)?;
    let (graph, _) = Graph::load(board, LoadMode::Strict)?;
    let task = graph.get(&args.task)?;
    require_github_link(&task)?;

    let (changes, apply_ready) = validate_proposal_for_apply(board, &proposal, &task)?;
    if !apply_ready {
        return Err(OrbitError::new(
            code::TRIAGE_INVALID_PROPOSAL,
            "proposal has no applicable metadata changes",
        ));
    }

    apply_proposal(board, &args.task, &proposal, task.clone(), changes)
}

fn validate_proposal_for_apply(
    board: &Board,
    proposal: &TriageProposal,
    task: &Task,
) -> Result<(serde_json::Value, bool)> {
    if proposal.uncertainty.is_some() && !proposal.has_metadata_changes() {
        return Err(OrbitError::new(
            code::TRIAGE_INVALID_PROPOSAL,
            "uncertainty-only proposals must not be applied; leave metadata unchanged",
        ));
    }
    let (changes, apply_ready) = proposal.build_preview(task)?;
    reject_basic_structured_changes(board, &changes)?;
    Ok((changes, apply_ready))
}

fn apply_proposal(
    board: &Board,
    id: &str,
    proposal: &TriageProposal,
    task: Task,
    changes: serde_json::Value,
) -> Result<crate::output::CommandOutput> {
    let mut task = task;
    let changed = proposal.apply_to(&mut task)?;
    if !changed {
        return Ok(crate::output::CommandOutput::new(
            json!({ "id": id, "applied": false, "changes": changes }),
            format!("No triage changes applied to {id}"),
        ));
    }

    task.front.updated_at = Utc::now();
    persist_task(board, &task, &format!("orbit: triage apply {id}"))?;

    Ok(crate::output::CommandOutput::new(
        json!({
            "id": id,
            "applied": true,
            "changes": changes,
            "sync_required": true,
            "sync_command": format!(
                "orbit github sync {} --direction push --apply",
                id
            ),
        }),
        format!("Applied triage metadata to {id}; publish with `orbit github sync`"),
    ))
}

fn is_interactive_review(json_mode: bool, args: &TriageReviewArgs) -> bool {
    !json_mode
        && args.proposal_file.as_os_str() != "-"
        && std::io::stdin().is_terminal()
        && std::io::stdout().is_terminal()
}

fn render_preview(id: &str, proposal: &TriageProposal, changes: &serde_json::Value) -> String {
    let mut lines = vec![format!("Triage proposal for {id}")];
    if let Some(rationale) = &proposal.rationale {
        lines.push(format!("Rationale: {rationale}"));
    }
    if let Some(uncertainty) = &proposal.uncertainty {
        lines.push(format!("Uncertainty: {uncertainty}"));
    }

    let change_map = changes.as_object();
    if let Some(area) = change_map.and_then(|changes| changes.get("area")) {
        lines.push(format!(
            "Area: {} -> {}",
            display_diff_value(&area["from"]),
            display_diff_value(&area["to"])
        ));
    }
    if let Some(verification) = change_map.and_then(|changes| changes.get("verification")) {
        lines.push(format!(
            "Verification: {} -> {}",
            display_diff_value(&verification["from"]),
            display_diff_value(&verification["to"])
        ));
    }
    if let Some(labels) = change_map.and_then(|changes| changes.get("labels")) {
        append_label_change(&mut lines, "Labels added", &labels["add"]);
        append_label_change(&mut lines, "Labels removed", &labels["remove"]);
    }
    if change_map.is_none_or(|changes| changes.is_empty()) {
        if proposal.uncertainty.is_some() {
            lines.push("No metadata changes proposed; leaving the Task unchanged.".into());
        } else {
            lines.push("No metadata changes: the proposal is already satisfied.".into());
        }
    }
    lines.push("".into());
    lines.join("\n")
}

fn display_diff_value(value: &serde_json::Value) -> String {
    value
        .as_str()
        .map(str::to_owned)
        .unwrap_or_else(|| "(unset)".into())
}

fn append_label_change(lines: &mut Vec<String>, heading: &str, labels: &serde_json::Value) {
    if let Some(labels) = labels.as_array()
        && !labels.is_empty()
    {
        let labels = labels
            .iter()
            .filter_map(serde_json::Value::as_str)
            .collect::<Vec<_>>()
            .join(", ");
        lines.push(format!("{heading}: {labels}"));
    }
}

fn prompt_for_approval(input: &mut impl BufRead, output: &mut impl Write) -> Result<bool> {
    loop {
        write!(output, "Apply these Orbit changes? [y/N] ")
            .map_err(|error| OrbitError::io("write triage approval prompt", error))?;
        output
            .flush()
            .map_err(|error| OrbitError::io("flush triage approval prompt", error))?;
        let mut answer = String::new();
        if input
            .read_line(&mut answer)
            .map_err(|error| OrbitError::io("read triage approval", error))?
            == 0
        {
            return Ok(false);
        }
        match answer.trim().to_ascii_lowercase().as_str() {
            "y" | "yes" => return Ok(true),
            "" | "n" | "no" | "cancel" => return Ok(false),
            _ => writeln!(output, "Enter y to apply or n to cancel.")
                .map_err(|error| OrbitError::io("write triage approval guidance", error))?,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{prompt_for_approval, render_preview};
    use crate::triage::proposal::TriageProposal;
    use std::io::Cursor;

    #[test]
    fn triage_review_prompt_cancels_unless_yes_is_explicit() {
        for answer in ["", "\n", "n\n", "no\n", "cancel\n"] {
            let mut input = Cursor::new(answer.as_bytes());
            let mut output = Vec::new();
            assert!(!prompt_for_approval(&mut input, &mut output).unwrap());
        }
        let mut eof = Cursor::new(Vec::<u8>::new());
        assert!(!prompt_for_approval(&mut eof, &mut Vec::new()).unwrap());
    }

    #[test]
    fn triage_review_prompt_accepts_yes_and_reprompts_for_unknown_input() {
        for answer in ["y\n", "yes\n", "maybe\ny\n"] {
            let mut input = Cursor::new(answer.as_bytes());
            let mut output = Vec::new();
            assert!(prompt_for_approval(&mut input, &mut output).unwrap());
            let output = String::from_utf8(output).unwrap();
            assert!(output.contains("Apply these Orbit changes? [y/N]"));
            if answer.starts_with("maybe") {
                assert!(output.contains("Enter y to apply or n to cancel."));
            }
        }
    }

    #[test]
    fn triage_review_preview_explains_uncertainty_and_noop_proposals() {
        let mut uncertain = TriageProposal::parse_json(
            r#"{"uncertainty":"The issue has no reproduction details."}"#,
        )
        .unwrap();
        uncertain.normalize().unwrap();
        let uncertainty_preview = render_preview("orb-123", &uncertain, &serde_json::json!({}));
        assert!(
            uncertainty_preview.contains("Uncertainty: The issue has no reproduction details.")
        );
        assert!(
            uncertainty_preview
                .contains("No metadata changes proposed; leaving the Task unchanged.")
        );

        let mut satisfied = TriageProposal::parse_json(r#"{"area":"cli"}"#).unwrap();
        satisfied.normalize().unwrap();
        let noop_preview = render_preview("orb-123", &satisfied, &serde_json::json!({}));
        assert!(noop_preview.contains("No metadata changes: the proposal is already satisfied."));
    }
}

fn reject_basic_structured_changes(board: &Board, changes: &serde_json::Value) -> Result<()> {
    if !board.config.github.mode.is_basic() {
        return Ok(());
    }
    let has_structured_field = changes.as_object().is_some_and(|changes| {
        changes.contains_key("area") || changes.contains_key("verification")
    });
    if has_structured_field {
        return Err(OrbitError::invalid_argument(
            "GitHub basic mode Triage supports labels only; area and verification require mode = `full` with organization Issue Fields",
        ));
    }
    Ok(())
}

fn load_proposal(path: &PathBuf) -> Result<TriageProposal> {
    let raw = if path.as_os_str() == "-" {
        let mut raw = String::new();
        std::io::stdin()
            .read_to_string(&mut raw)
            .map_err(|error| OrbitError::io("read proposal from stdin", error))?;
        raw
    } else {
        fs::read_to_string(path).map_err(|error| {
            OrbitError::io(format!("read proposal file {}", path.display()), error)
        })?
    };
    let mut proposal = TriageProposal::parse_json(&raw)?;
    proposal.normalize()?;
    Ok(proposal)
}

fn require_github_link(task: &Task) -> Result<serde_json::Value> {
    task.github_link()
        .map(|link| json!({ "repo": link.repo, "number": link.number }))
        .ok_or_else(|| {
            OrbitError::new(
                code::TRIAGE_NOT_LINKED,
                format!(
                    "task {} is not linked to a GitHub Issue; adopt explicitly with `orbit github adopt` before triage",
                    task.id
                ),
            )
        })
}

fn persist_task(board: &Board, task: &Task, message: &str) -> Result<()> {
    let rendered = task::render_task(task)?;
    atomic::write_atomic(&board.task_path(&task.id), rendered.as_bytes(), true)?;
    board.commit(message, false)?;
    Ok(())
}

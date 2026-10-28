use std::fs;
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

pub fn run(board: &Board, args: TriageArgs) -> Result<crate::output::CommandOutput> {
    match args.command {
        TriageCommand::Context { task } => cmd_context(board, &task),
        TriageCommand::Review(args) => cmd_review(board, args),
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

    let guidance_path_json = guidance_path.as_ref().map(|path| path.display().to_string());

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
    });
    let mut output = crate::output::CommandOutput::new(
        data,
        format!("Triage context for {id} (GitHub-linked)"),
    );
    output.warnings.extend(warnings);
    Ok(output)
}

fn cmd_review(board: &Board, args: TriageReviewArgs) -> Result<crate::output::CommandOutput> {
    let proposal = load_proposal(&args.proposal_file)?;
    let (graph, _) = Graph::load(board, LoadMode::Strict)?;
    let task = graph.get(&args.task)?;
    require_github_link(&task)?;

    let (changes, apply_ready) = proposal.build_preview(&task)?;
    let data = json!({
        "id": task.id,
        "proposal": {
            "rationale": proposal.rationale,
            "uncertainty": proposal.uncertainty,
        },
        "changes": changes,
        "apply_ready": apply_ready,
        "sync_note": "After approval, run `orbit github sync <ID> --direction push --apply` to publish Orbit-owned fields and labels to GitHub.",
    });
    Ok(crate::output::CommandOutput::new(
        data,
        if apply_ready {
            format!("Reviewed triage proposal for {}", args.task)
        } else {
            format!(
                "Triage left {} unchanged due to uncertainty",
                args.task
            )
        },
    ))
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

    if proposal.uncertainty.is_some() && !proposal.has_metadata_changes() {
        return Err(OrbitError::new(
            code::TRIAGE_INVALID_PROPOSAL,
            "uncertainty-only proposals must not be applied; leave metadata unchanged",
        ));
    }

    let (changes, apply_ready) = proposal.build_preview(&task)?;
    if !apply_ready {
        return Err(OrbitError::new(
            code::TRIAGE_INVALID_PROPOSAL,
            "proposal has no applicable metadata changes",
        ));
    }

    let mut task = task.clone();
    let changed = proposal.apply_to(&mut task)?;
    if !changed {
        return Ok(crate::output::CommandOutput::new(
            json!({ "id": args.task, "applied": false, "changes": changes }),
            format!("No triage changes applied to {}", args.task),
        ));
    }

    task.front.updated_at = Utc::now();
    persist_task(board, &task, &format!("orbit: triage apply {}", args.task))?;

    Ok(crate::output::CommandOutput::new(
        json!({
            "id": args.task,
            "applied": true,
            "changes": changes,
            "sync_required": true,
            "sync_command": format!(
                "orbit github sync {} --direction push --apply",
                args.task
            ),
        }),
        format!(
            "Applied triage metadata to {}; publish with `orbit github sync`",
            args.task
        ),
    ))
}

fn load_proposal(path: &PathBuf) -> Result<TriageProposal> {
    let raw = fs::read_to_string(path).map_err(|error| {
        OrbitError::io(format!("read proposal file {}", path.display()), error)
    })?;
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

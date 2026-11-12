use std::fs;
use std::io::{BufRead, IsTerminal, Read, Write};
use std::path::Path;

use chrono::Utc;
use serde_json::json;

use crate::board::Board;
use crate::cli::{TriageApplyArgs, TriageArgs, TriageCommand, TriageReviewArgs};
use crate::error::{OrbitError, Result, code};
use crate::graph::{Graph, LoadMode};
use crate::remote::link::{RemoteLinkView, active_link, github_compat_json, link_json};
use crate::remote::model::MappedFields;
use crate::remote::port::{BoardResolver, RemoteResolver};
use crate::task::Task;
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
    let publish = publishability(board, task);

    let (guidance, guidance_path) = read_project_guidance(board);
    let mut warnings = Vec::new();
    if guidance.is_none()
        && let Some(ref path) = guidance_path
    {
        warnings.push(guidance_missing_note(path));
    }
    if let Publishability::Unavailable { tracker } = &publish {
        warnings.push(unavailable_tracker_note(id, tracker));
    }

    let guidance_path_json = guidance_path
        .as_ref()
        .map(|path| path.display().to_string());

    // An empty `board.areas` means the Board defines no Areas, so any
    // lowercase slug is allowed; `null` tells the agent there is no list.
    let allowed_areas = if board.config.board.areas.is_empty() {
        serde_json::Value::Null
    } else {
        json!(board.config.board.areas)
    };
    // For a Linked Task only the Remote Project's Mapped fields can be
    // published, so they are the only fields Triage may change; a Local
    // Task (or an unresolvable tracker) has no publish step and every
    // field stays available.
    let capabilities = match &publish {
        Publishability::Remote { mapped, .. } => json!({
            "area": mapped.area,
            "verification": mapped.verification,
            "labels": mapped.labels,
        }),
        _ => json!({
            "area": true,
            "verification": true,
            "labels": true,
        }),
    };
    let data = json!({
        "id": task.id,
        "github": github_compat_json(task),
        "remote": link_json(task),
        "current": {
            "area": task.front.area.as_deref(),
            "verification": task.front.verification,
            "labels": task.front.labels,
        },
        "project_guidance": guidance,
        "project_guidance_path": guidance_path_json,
        "allowed_areas": allowed_areas,
        "canonical_labels": CANONICAL_TRIAGE_LABELS,
        "metadata_capabilities": capabilities,
    });
    let mut output = crate::output::CommandOutput::new(data, format!("Triage context for {id}"));
    output.warnings.extend(warnings);
    Ok(output)
}

fn cmd_review(
    board: &Board,
    args: TriageReviewArgs,
    json_mode: bool,
) -> Result<crate::output::CommandOutput> {
    let proposal = load_proposal(board, &args.proposal_file)
        .map_err(|error| with_proposal_context(error, &args.task, &args.proposal_file))?;
    let (graph, _) = Graph::load(board, LoadMode::Strict)?;
    let task = graph.get(&args.task)?;
    let publish = publishability(board, task);

    let (changes, apply_ready) = proposal
        .build_preview(task)
        .map_err(|error| with_proposal_context(error, &args.task, &args.proposal_file))?;
    reject_unmapped_changes(&publish, task, &changes)?;
    let preview = render_preview(&args.task, &proposal, &changes);
    let data = json!({
        "id": task.id,
        "proposal": {
            "rationale": proposal.rationale,
            "uncertainty": proposal.uncertainty,
        },
        "changes": changes,
        "apply_ready": apply_ready,
        "sync_note": publish_note(&publish, &args.task),
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
        )
        .with_details(json!({
            "task": args.task,
            "proposal_file": args.proposal_file.display().to_string(),
        })));
    }

    let proposal = load_proposal(board, &args.proposal_file)
        .map_err(|error| with_proposal_context(error, &args.task, &args.proposal_file))?;
    let (graph, _) = Graph::load(board, LoadMode::Strict)?;
    let task = graph.get(&args.task)?;

    let (changes, apply_ready) = validate_proposal_for_apply(board, &proposal, task)
        .map_err(|error| with_proposal_context(error, &args.task, &args.proposal_file))?;
    if !apply_ready {
        return Err(OrbitError::new(
            code::TRIAGE_INVALID_PROPOSAL,
            "proposal has no applicable metadata changes",
        )
        .with_details(proposal_context(&args.task, &args.proposal_file)));
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
    reject_unmapped_changes(&publishability(board, task), task, &changes)?;
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

    task.touch(Utc::now());
    let outcome =
        crate::mutation::persist_task(board, &task, &format!("orbit: triage apply {id}"))?;

    let publish = publishability(board, &task);
    let (data, message) = match &publish {
        Publishability::Remote { link, .. } => {
            let command = publish_command(link, id);
            (
                json!({
                    "id": id,
                    "applied": true,
                    "changes": changes,
                    "sync_required": true,
                    "sync_command": command,
                }),
                format!("Applied triage metadata to {id}; publish with `{command}`"),
            )
        }
        _ => (
            json!({
                "id": id,
                "applied": true,
                "changes": changes,
                "sync_required": false,
                "sync_command": serde_json::Value::Null,
            }),
            format!("Applied triage metadata to {id}"),
        ),
    };
    let mut output = crate::output::CommandOutput::new(data, message);
    if let Publishability::Unavailable { tracker } = &publish {
        output.warnings.push(unavailable_tracker_note(id, tracker));
    }
    if let Some(warning) = crate::mutation::commit_warning(outcome) {
        output.warnings.push(warning);
    }
    Ok(output)
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

/// How a Task's triage changes can be published, resolved once per
/// command. Triage itself is local-first: `Local` and `Unavailable` both
/// mean nothing is published, and only `Remote` restricts which fields a
/// proposal may change to the Remote Project's Mapped fields.
enum Publishability {
    /// No active Remote Issue link.
    Local,
    /// Actively linked and the tracker's adapter resolved.
    Remote {
        link: Box<RemoteLinkView>,
        mapped: MappedFields,
    },
    /// Linked to a tracker whose adapter cannot be resolved.
    Unavailable { tracker: String },
}

fn publishability(board: &Board, task: &Task) -> Publishability {
    let Some(link) = active_link(task) else {
        return Publishability::Local;
    };
    match BoardResolver::new(board).adapter(&link.tracker) {
        Ok(adapter) => Publishability::Remote {
            mapped: adapter.mapped_fields(&link.remote_project()),
            link: Box::new(link),
        },
        Err(_) => Publishability::Unavailable {
            tracker: link.tracker.clone(),
        },
    }
}

/// The command that publishes a Linked Task's mapped fields through the
/// provider-neutral Remote Tracker surface.
fn publish_command(_link: &RemoteLinkView, task: &str) -> String {
    format!("orbit remote sync {task} --direction push --apply")
}

fn publish_note(publish: &Publishability, task: &str) -> String {
    match publish {
        Publishability::Remote { link, .. } => format!(
            "After approval, run `{}` to publish mapped Orbit fields and labels to the Remote Issue.",
            publish_command(link, task)
        ),
        Publishability::Unavailable { tracker } => format!(
            "Task is linked to tracker `{tracker}`, which has no installed adapter; triage changes stay local."
        ),
        Publishability::Local => {
            "Local Task: triage changes stay on the Orbit Board; there is no Remote Issue to publish to.".into()
        }
    }
}

fn unavailable_tracker_note(task: &str, tracker: &str) -> String {
    format!(
        "Task {task} links to tracker `{tracker}`, which has no installed adapter; triage changes stay local and cannot be published"
    )
}

/// For a Linked Task, reject proposal changes the Remote Project cannot
/// publish: the adapter's Mapped fields are the only fields sync carries,
/// so proposing an unmappable field would promise a publish that can
/// never happen (and, for `area`/`verification`, would break the next
/// push). Local Tasks and unresolvable trackers have no publish step, so
/// nothing is rejected.
fn reject_unmapped_changes(
    publish: &Publishability,
    task: &Task,
    changes: &serde_json::Value,
) -> Result<()> {
    let Publishability::Remote { link, mapped } = publish else {
        return Ok(());
    };
    let unmapped: Vec<&str> = [
        ("area", mapped.area),
        ("verification", mapped.verification),
        ("labels", mapped.labels),
    ]
    .into_iter()
    .filter(|(field, mapped)| changes.get(field).is_some() && !mapped)
    .map(|(field, _)| field)
    .collect();
    if unmapped.is_empty() {
        return Ok(());
    }
    Err(OrbitError::invalid_argument(format!(
        "Task {} is linked to Remote Project `{}` on tracker `{}`, which does not map field(s): {}; Triage may only change fields the Remote Tracker publishes",
        task.id,
        link.project,
        link.tracker,
        unmapped.join(", ")
    )))
}

fn load_proposal(board: &Board, path: &Path) -> Result<TriageProposal> {
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
    proposal.normalize(&board.config.board.areas)?;
    Ok(proposal)
}

fn proposal_context(task: &str, path: &Path) -> serde_json::Value {
    json!({
        "task": task,
        "proposal_file": path.display().to_string(),
    })
}

fn with_proposal_context(mut error: OrbitError, task: &str, path: &Path) -> OrbitError {
    if error.code == code::TRIAGE_INVALID_PROPOSAL {
        let mut details = match error.details.take() {
            Some(serde_json::Value::Object(details)) => details,
            Some(value) => serde_json::Map::from_iter([("value".to_string(), value)]),
            None => serde_json::Map::new(),
        };
        details.extend(
            proposal_context(task, path)
                .as_object()
                .expect("proposal context is an object")
                .clone(),
        );
        error.details = Some(serde_json::Value::Object(details));
    }
    error
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
        uncertain.normalize(&[]).unwrap();
        let uncertainty_preview = render_preview("orb-123", &uncertain, &serde_json::json!({}));
        assert!(
            uncertainty_preview.contains("Uncertainty: The issue has no reproduction details.")
        );
        assert!(
            uncertainty_preview
                .contains("No metadata changes proposed; leaving the Task unchanged.")
        );

        let mut satisfied = TriageProposal::parse_json(r#"{"area":"cli"}"#).unwrap();
        satisfied.normalize(&[]).unwrap();
        let noop_preview = render_preview("orb-123", &satisfied, &serde_json::json!({}));
        assert!(noop_preview.contains("No metadata changes: the proposal is already satisfied."));
    }
}

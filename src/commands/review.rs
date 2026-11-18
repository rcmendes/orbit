//! `orbit submit`, `orbit review`, `orbit ask`, and `orbit answer` — the
//! review, evidence, and escalation commands (ADR 0022). All four run
//! through [`crate::ops`] so CLI, batch, and MCP share the same workflow
//! transitions and `[policy]` enforcement.

use chrono::Utc;
use serde_json::json;

use crate::cli::{AnswerArgs, AskArgs, ReviewArgs, ReviewVerdictArg, SubmitArgs};
use crate::review::Verdict;

use super::*;

fn op_output<T>(
    result: crate::mutation::OpResult<T>,
    human: String,
) -> crate::output::CommandOutput {
    let mut output =
        crate::output::CommandOutput::new(graph::task_detail(&result.graph, &result.task), human);
    output.warnings = result.warnings;
    output
}

/// `orbit submit <id>`: evidence flags plus the `submit` transition into
/// In Review.
pub(super) fn cmd_submit(board: &Board, args: SubmitArgs) -> Result<crate::output::CommandOutput> {
    let agent = resolve_agent(args.by)?;
    let evidence = crate::review::Evidence {
        commands: args.command,
        exit_code: args.exit_code,
        commit: args.commit,
        files: args.file,
        note: args.note,
    };
    let id = args.id.clone();
    let result = crate::mutation::run_op(board, &args.id, "submit", |graph| {
        crate::ops::submit(
            graph,
            &args.id,
            &agent,
            args.kind,
            &evidence,
            args.force,
            Utc::now(),
        )
        .map(|changed| ((), changed))
    })?;
    let human = if result.changed {
        format!("submitted {} for review", result.task.id)
    } else {
        format!("{} was already in review", result.task.id)
    };
    let mut output = op_output(result, human);
    if let Some(data) = output.data.as_object_mut() {
        data.insert("id".to_string(), json!(id));
    }
    Ok(output)
}

/// `orbit review <id> --verdict approve|request-changes`.
pub(super) fn cmd_review(board: &Board, args: ReviewArgs) -> Result<crate::output::CommandOutput> {
    let agent = resolve_agent(args.by)?;
    let verdict = match args.verdict {
        ReviewVerdictArg::Approve => Verdict::Approve,
        ReviewVerdictArg::RequestChanges => Verdict::RequestChanges,
    };
    let reason = args.reason.clone();
    let result = crate::mutation::run_op(board, &args.id, "review", |graph| {
        let outcome = crate::ops::review(
            graph,
            &args.id,
            verdict,
            reason.as_deref(),
            &agent,
            args.kind,
            args.force,
            Utc::now(),
        )?;
        let changed = outcome.changed;
        Ok((outcome, changed))
    })?;
    let outcome = &result.outcome;
    let human = if !result.changed {
        format!("{} review verdict already recorded", result.task.id)
    } else if outcome.escalated {
        format!(
            "review of {} requested changes; attempt limit reached, escalated to {}",
            result.task.id, outcome.status
        )
    } else {
        match outcome.verdict {
            Verdict::Approve => format!("approved {}", result.task.id),
            Verdict::RequestChanges => format!(
                "requested changes on {} (back to {})",
                result.task.id, outcome.status
            ),
        }
    };
    let verdict_str = result.outcome.verdict.as_str();
    let escalated = result.outcome.escalated;
    let mut output = op_output(result, human);
    if let Some(data) = output.data.as_object_mut() {
        data.insert(
            "review".to_string(),
            json!({"verdict": verdict_str, "escalated": escalated}),
        );
    }
    Ok(output)
}

/// `orbit ask <id> "<question>"`.
pub(super) fn cmd_ask(board: &Board, args: AskArgs) -> Result<crate::output::CommandOutput> {
    let agent = resolve_agent(args.by)?;
    let question = args.question.clone();
    let result = crate::mutation::run_op(board, &args.id, "ask", |graph| {
        let outcome = crate::ops::ask(graph, &args.id, &question, &agent, Utc::now())?;
        let changed = outcome.changed;
        Ok((outcome, changed))
    })?;
    let human = if result.outcome.status_changed {
        format!(
            "asked on {}: moved to {} — blocked until `orbit answer`",
            result.task.id, result.outcome.status
        )
    } else {
        format!("asked on {} — blocked until `orbit answer`", result.task.id)
    };
    Ok(op_output(result, human))
}

/// `orbit answer <id> "<answer>"`.
pub(super) fn cmd_answer(board: &Board, args: AnswerArgs) -> Result<crate::output::CommandOutput> {
    let agent = resolve_agent(args.by)?;
    let answer = args.answer.clone();
    let result = crate::mutation::run_op(board, &args.id, "answer", |graph| {
        let outcome = crate::ops::answer(graph, &args.id, &answer, &agent, Utc::now())?;
        let changed = outcome.changed;
        Ok((outcome, changed))
    })?;
    let human = match &result.outcome.returned_to {
        Some(target) => format!("answered {} — back to {target}", result.task.id),
        None => format!("answered {}", result.task.id),
    };
    Ok(op_output(result, human))
}

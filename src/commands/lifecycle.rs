use chrono::Utc;
use serde_json::json;

use crate::cli::{
    CancelArgs, CheckpointArgs, ClaimArgs, DoneArgs, HeartbeatArgs, ReleaseArgs, ReopenArgs,
};
use crate::comment;
use crate::transition::{CheckpointUpdate, FieldUpdate};

use super::*;

pub(super) fn cmd_claim(board: &Board, args: ClaimArgs) -> Result<crate::output::CommandOutput> {
    let agent = resolve_agent(args.by)?;
    let result = crate::mutation::execute(
        board,
        &args.id,
        crate::mutation::Lifecycle::Claim {
            agent: &agent,
            steal: args.steal,
        },
        Utc::now(),
    )?;
    let lease = result
        .task
        .lease_until()
        .map(|value| value.to_rfc3339())
        .unwrap_or_default();
    Ok(mutation_output(
        result,
        format!("claimed {} by {agent} until {lease}", args.id),
    ))
}

pub(super) fn cmd_release(
    board: &Board,
    args: ReleaseArgs,
) -> Result<crate::output::CommandOutput> {
    let caller = optional_agent(args.by);
    let result = crate::mutation::execute(
        board,
        &args.id,
        crate::mutation::Lifecycle::Release {
            caller: caller.as_deref(),
            force: args.force,
        },
        Utc::now(),
    )?;
    if !result.changed {
        return Ok(crate::output::CommandOutput::new(
            json!({"id": args.id, "changed": false}),
            format!("{} was not claimed", args.id),
        ));
    }
    Ok(mutation_output(result, format!("released {}", args.id)))
}

pub(super) fn cmd_heartbeat(
    board: &Board,
    args: HeartbeatArgs,
) -> Result<crate::output::CommandOutput> {
    let agent = resolve_agent(args.by)?;
    let result = crate::mutation::execute(
        board,
        &args.id,
        crate::mutation::Lifecycle::Heartbeat { agent: &agent },
        Utc::now(),
    )?;
    let lease = result.task.lease_until().expect("heartbeat sets lease");
    Ok(crate::output::CommandOutput::new(
        json!({"id": args.id, "lease_until": lease}),
        format!("heartbeat {} extended to {}", args.id, lease.to_rfc3339()),
    )
    .with_meta(json!({"committed": false})))
}

pub(super) fn cmd_checkpoint(
    board: &Board,
    args: CheckpointArgs,
) -> Result<crate::output::CommandOutput> {
    let agent = resolve_agent(args.by)?;
    let update = CheckpointUpdate {
        progress: args.progress,
        next: checkpoint_field(args.next, args.clear_next),
        blocker: checkpoint_field(args.blocker, args.clear_blocker),
    };
    let result = crate::mutation::execute(
        board,
        &args.id,
        crate::mutation::Lifecycle::Checkpoint {
            agent: &agent,
            update,
        },
        Utc::now(),
    )?;
    Ok(mutation_output(
        result,
        format!("checkpoint recorded for {} by {agent}", args.id),
    ))
}

fn checkpoint_field(value: Option<String>, clear: bool) -> FieldUpdate<String> {
    if clear {
        FieldUpdate::Clear
    } else if let Some(value) = value {
        FieldUpdate::Set(value)
    } else {
        FieldUpdate::Keep
    }
}

pub(super) fn cmd_done(board: &Board, args: DoneArgs) -> Result<crate::output::CommandOutput> {
    terminal_command(
        board,
        args.id,
        args.by,
        args.force,
        args.comment,
        args.comment_file,
        crate::mutation::TerminalKind::Done,
    )
}

pub(super) fn cmd_cancel(board: &Board, args: CancelArgs) -> Result<crate::output::CommandOutput> {
    terminal_command(
        board,
        args.id,
        args.by,
        args.force,
        args.comment,
        args.comment_file,
        crate::mutation::TerminalKind::Cancelled,
    )
}

fn terminal_command(
    board: &Board,
    id: String,
    by: Option<String>,
    force: bool,
    comment: Option<String>,
    comment_file: Option<std::path::PathBuf>,
    kind: crate::mutation::TerminalKind,
) -> Result<crate::output::CommandOutput> {
    let comment = resolve_optional_comment(comment, comment_file)?;
    let author = if comment.is_some() {
        Some(resolve_agent(by.clone())?)
    } else {
        None
    };
    let result = crate::mutation::finish(
        board,
        &id,
        optional_agent(by).as_deref(),
        force,
        author.zip(comment),
        kind,
        Utc::now(),
    )?;
    let past = kind.past();
    let human = if !result.changed && result.comments.is_none() {
        format!("{id} is already {past}")
    } else if result.comments.is_some() {
        format!("{past} {id} (comment added)")
    } else {
        format!("{past} {id}")
    };
    let data = match &result.comments {
        Some(comments) => graph::task_detail_with_comments(&result.graph, &result.task, comments),
        None => graph::task_detail(&result.graph, &result.task),
    };
    let mut output = crate::output::CommandOutput::new(data, human);
    if !result.changed && result.comments.is_none() {
        output.meta = Some(json!({"changed": false}));
    }
    output.warnings = result.warnings;
    if result.comment_failed {
        output = output.with_exit_code(1);
    }
    Ok(output)
}

pub(super) fn cmd_reopen(board: &Board, args: ReopenArgs) -> Result<crate::output::CommandOutput> {
    let result = crate::mutation::reopen(board, &args.id, args.force, Utc::now())?;
    Ok(mutation_output(result, format!("reopened {}", args.id)))
}

fn resolve_optional_comment(
    comment: Option<String>,
    comment_file: Option<std::path::PathBuf>,
) -> Result<Option<String>> {
    if comment.is_none() && comment_file.is_none() {
        return Ok(None);
    }
    let body = resolve_body(comment, comment_file)?;
    comment::validate_body(&body)?;
    Ok(Some(body))
}

use chrono::Utc;
use serde_json::json;

use crate::cli::{
    AssignArgs, CancelArgs, CheckpointArgs, ClaimArgs, DoneArgs, HeartbeatArgs, ReleaseArgs,
    ReopenArgs, TransitionArgs,
};
use crate::comment;
use crate::transition::{CheckpointUpdate, FieldUpdate};

use super::*;

pub(super) fn cmd_claim(board: &Board, args: ClaimArgs) -> Result<crate::output::CommandOutput> {
    let agent = resolve_agent(args.by)?;
    let worktree = if args.worktree {
        Some(plan_worktree(board, &args.id, args.base.as_deref())?)
    } else {
        None
    };
    let result = crate::mutation::execute(
        board,
        &args.id,
        crate::mutation::Lifecycle::Claim {
            agent: &agent,
            kind: args.kind,
            steal: args.steal,
        },
        Utc::now(),
    )?;
    let lease = result
        .task
        .lease_until()
        .map(|value| value.to_rfc3339())
        .unwrap_or_default();
    let mut output = mutation_output(
        result,
        format!("claimed {} by {agent} until {lease}", args.id),
    );
    if let Some(plan) = worktree {
        let (created, warning) = plan.create().map_err(|cause| {
            OrbitError::new(
                code::CLAIM_WORKTREE_FAILED,
                format!(
                    "{}; claim remains active, but worktree preparation failed: {}. Fix the cause and retry `orbit claim {} --by {agent} --worktree`; no claim was released",
                    output.human, cause.message, args.id
                ),
            )
            .with_details(json!({
                "claim": output.data,
                "worktree": {
                    "repository": plan.repository,
                    "path": plan.path,
                    "branch": plan.branch,
                },
                "cause": {
                    "code": cause.code,
                    "message": cause.message,
                    "retryable": cause.retryable,
                    "details": cause.details,
                },
            }))
        })?;
        if let Some(data) = output.data.as_object_mut() {
            data.insert(
                "worktree".to_string(),
                json!({
                    "repository": plan.repository,
                    "path": crate::repository::canonical_lenient(&plan.path),
                    "branch": plan.branch,
                    "created": created,
                }),
            );
        }
        output.human.push_str(&format!(
            "\nworktree {} on {} ({})",
            plan.path.display(),
            plan.branch,
            plan.repository
        ));
        if let Some(warning) = warning {
            output.warnings.push(warning);
        }
    }
    Ok(output)
}

/// The `.worktrees/<id>` worktree `orbit claim --worktree` prepares in the
/// Task's member Repository on branch `ticket/<id>`.
struct WorktreePlan {
    repository: String,
    repo_path: std::path::PathBuf,
    path: std::path::PathBuf,
    branch: String,
    existing: bool,
    base: Option<String>,
    base_commit: Option<String>,
}

/// Resolves the member Repository a claim worktree belongs to and validates
/// the target before the claim is taken: the Task's `repo` field wins, else
/// the member Repository containing the current directory. `--base` must
/// resolve to a commit in that Repository so an unknown ref fails before any
/// lease is taken.
fn plan_worktree(board: &Board, id: &str, base: Option<&str>) -> Result<WorktreePlan> {
    let (graph, _) = crate::graph::Graph::load(board, crate::graph::LoadMode::Strict)?;
    let task = graph.get(id)?;
    let member = match task.front.repository.as_deref() {
        Some(name) => crate::repository::members(board)?
            .into_iter()
            .find(|member| member.name == name)
            .ok_or_else(|| {
                OrbitError::invalid_argument(format!(
                    "{id} names Repository `{name}`, which is not a member of this Board"
                ))
            })?,
        None => crate::repository::current_member(board)?.ok_or_else(|| {
            OrbitError::new(
                code::NOT_A_REPOSITORY,
                format!(
                    "{id} has no Repository; run `orbit claim {id} --worktree` inside a member Repository"
                ),
            )
        })?,
    };
    // Worktree paths and branch names key off the immutable Task ID, never
    // a project key the user typed.
    let task_id = task.id.clone();
    let path = member.path.join(".worktrees").join(&task_id);
    let branch = format!("ticket/{task_id}");
    let records = crate::git::worktree_records(&member.path)?;
    let target = crate::repository::canonical_lenient(&path);
    if let Some(record) = records.iter().find(|record| {
        record.branch.as_deref() == Some(branch.as_str())
            && crate::repository::canonical_lenient(&record.path) != target
    }) {
        return Err(OrbitError::git(format!(
            "{branch} is already checked out at {}; claim unchanged",
            record.path.display()
        )));
    }
    let mut existing = false;
    if path.exists() {
        let matches = records.iter().any(|record| {
            !record.missing
                && crate::repository::canonical_lenient(&record.path) == target
                && record.branch.as_deref() == Some(branch.as_str())
        });
        if !matches {
            return Err(OrbitError::git(format!(
                "{} already exists and is not the {branch} worktree",
                path.display()
            )));
        }
        existing = true;
    }
    let base_commit = match base {
        Some(reference) => Some(crate::git::rev_parse(&member.path, reference)?.ok_or_else(
            || {
                OrbitError::invalid_argument(format!(
                    "--base {reference} does not resolve to a commit in Repository `{}`",
                    member.name
                ))
            },
        )?),
        None => None,
    };
    Ok(WorktreePlan {
        repository: member.name,
        repo_path: member.path,
        path,
        branch,
        existing,
        base: base.map(str::to_string),
        base_commit,
    })
}

impl WorktreePlan {
    /// Creates the worktree unless it already exists, hiding `.worktrees/`
    /// through the Repository's local exclude file. Returns whether a new
    /// worktree was created plus a warning when an existing `ticket/<id>`
    /// branch is reused without containing the requested `--base`.
    fn create(&self) -> Result<(bool, Option<String>)> {
        crate::git::ensure_exclude_entry(&self.repo_path, &self.repo_path.join(".worktrees"))?;
        if self.existing {
            return Ok((false, self.base_warning()?));
        }
        if crate::git::branch_exists(&self.repo_path, &self.branch)? {
            crate::git::worktree_add(&self.repo_path, &self.path, &self.branch)?;
            return Ok((true, self.base_warning()?));
        }
        let base = match &self.base_commit {
            Some(commit) => commit.clone(),
            None => crate::git::default_branch(&self.repo_path)?,
        };
        crate::git::worktree_add_new(&self.repo_path, &self.path, &self.branch, &base)?;
        Ok((true, None))
    }

    /// Warns when `ticket/<id>` already exists but does not contain the
    /// `--base` commit: the branch is reused as-is and never rebased.
    fn base_warning(&self) -> Result<Option<String>> {
        let (Some(reference), Some(base_commit)) = (&self.base, &self.base_commit) else {
            return Ok(None);
        };
        let merge_base = crate::git::merge_base(&self.repo_path, &self.branch, base_commit)?;
        if merge_base.as_deref() == Some(base_commit.as_str()) {
            return Ok(None);
        }
        Ok(Some(format!(
            "{} already exists and does not contain {reference}; the existing branch was kept unchanged",
            self.branch
        )))
    }
}

/// `orbit assign`: set the assignee without a lease — how humans take work.
pub(super) fn cmd_assign(board: &Board, args: AssignArgs) -> Result<crate::output::CommandOutput> {
    let result = crate::mutation::execute(
        board,
        &args.id,
        crate::mutation::Lifecycle::Assign {
            name: &args.to,
            kind: args.kind,
            force: args.force,
        },
        Utc::now(),
    )?;
    if !result.changed {
        return Ok(crate::output::CommandOutput::new(
            json!({"id": args.id, "assignee": args.to, "changed": false}),
            format!("{} was already assigned to {}", args.id, args.to),
        ));
    }
    Ok(mutation_output(
        result,
        format!("assigned {} to {}", args.id, args.to),
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

pub(super) fn cmd_transition(
    board: &Board,
    args: TransitionArgs,
) -> Result<crate::output::CommandOutput> {
    let caller = optional_agent(args.by);
    let result = crate::mutation::execute(
        board,
        &args.id,
        crate::mutation::Lifecycle::Transition {
            name: &args.name,
            caller: caller.as_deref(),
            force: args.force,
        },
        Utc::now(),
    )?;
    if !result.changed {
        return Ok(crate::output::CommandOutput::new(
            json!({"id": args.id, "transition": args.name, "changed": false}),
            format!("{} was already at the `{}` target", args.id, args.name),
        ));
    }
    Ok(mutation_output(
        result,
        format!("transitioned {} via `{}`", args.id, args.name),
    ))
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

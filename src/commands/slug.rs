use serde_json::{Value, json};

use crate::cli::{SlugArgs, SlugMigrateArgs};
use crate::graph::LoadMode;
use crate::migrate::{self, Plan};
use crate::slug;

use super::*;

pub(super) fn cmd_slug(board: &Board, args: SlugArgs) -> Result<crate::output::CommandOutput> {
    match args.command {
        SlugCommand::Show => cmd_slug_show(board),
        SlugCommand::Migrate(args) => cmd_slug_migrate(board, args),
    }
}

fn cmd_slug_show(board: &Board) -> Result<crate::output::CommandOutput> {
    let ids = board.task_ids()?;
    let subtasks = ids.iter().filter(|id| id.contains('.')).count();
    let comment_files = board.comment_ids()?.len();
    let slug = slug::slug_of_prefix(&board.config.id.prefix);

    let human = format!(
        "slug     {slug}\nprefix   {}\ntasks    {} ({})\ncomments {}\n",
        board.config.id.prefix,
        ids.len(),
        plural(subtasks, "SubTask", "SubTasks"),
        plural(comment_files, "file", "files"),
    );
    Ok(crate::output::CommandOutput::new(
        json!({
            "slug": slug,
            "prefix": board.config.id.prefix,
            "tasks": ids.len(),
            "subtasks": subtasks,
            "comment_files": comment_files,
        }),
        human,
    ))
}

fn cmd_slug_migrate(board: &Board, args: SlugMigrateArgs) -> Result<crate::output::CommandOutput> {
    let (graph, _) = Graph::load(board, LoadMode::Strict)?;
    let issues = migrate::health_issues(board, &graph)?;
    if !issues.is_empty() {
        return Err(migrate::blocked_by_issues(issues));
    }

    let plan = migrate::plan(board, &graph, &args.slug)?;
    if !plan.collisions.is_empty() {
        return Err(migrate::blocked_by_collisions(
            &plan.collisions,
            &plan.new_prefix,
        ));
    }
    if !args.dry_run && plan.changed && !plan.claims.is_empty() && !args.force {
        return Err(migrate::blocked_by_claims(&plan.claims));
    }

    if args.dry_run || !plan.changed {
        let mut output = crate::output::CommandOutput::new(
            slug_plan_json(&plan, args.dry_run, false, None),
            slug_plan_human(&plan, args.dry_run),
        );
        if !plan.claims.is_empty() {
            output.warnings.push(format!(
                "{} live claim(s) would change ID; apply requires --force",
                plan.claims.len()
            ));
        }
        return Ok(output);
    }

    let applied = migrate::apply(board, &graph, &plan)?;
    let mut output = crate::output::CommandOutput::new(
        slug_plan_json(&plan, false, true, Some(&applied.commit)),
        slug_applied_human(&plan, &applied.commit),
    );
    output.warnings = applied.warnings;
    if !plan.claims.is_empty() {
        output.warnings.push(format!(
            "{} live claim(s) changed ID; holders must re-read the Board",
            plan.claims.len()
        ));
    }
    Ok(output)
}

fn slug_plan_json(plan: &Plan, dry_run: bool, applied: bool, commit: Option<&str>) -> Value {
    json!({
        "current": {
            "slug": &plan.old_slug,
            "prefix": &plan.old_prefix,
        },
        "target": {
            "slug": &plan.new_slug,
            "prefix": &plan.new_prefix,
        },
        "changed": plan.changed,
        "tasks": plan.task_count,
        "subtasks": plan.subtask_count,
        "comment_files": plan.comment_files.len(),
        "references": plan.references,
        "mappings": &plan.mappings,
        "claims": &plan.claims,
        "collisions": &plan.collisions,
        "dry_run": dry_run,
        "applied": applied,
        "commit": commit,
    })
}

fn slug_plan_human(plan: &Plan, dry_run: bool) -> String {
    if !plan.changed {
        return format!(
            "board already uses slug `{}` (prefix `{}`); nothing to migrate\n",
            plan.old_slug, plan.old_prefix
        );
    }

    let mut human = String::from(if dry_run {
        "slug migration plan\n"
    } else {
        "slug migration\n"
    });
    human.push_str(&format!(
        "prefix     {} -> {}\n",
        plan.old_prefix, plan.new_prefix
    ));
    human.push_str(&format!(
        "tasks      {} ({})\n",
        plan.task_count,
        plural(plan.subtask_count, "SubTask", "SubTasks")
    ));
    human.push_str(&format!(
        "comments   {}\n",
        plural(plan.comment_files.len(), "file", "files")
    ));
    human.push_str(&format!("references {}\n", plan.references));
    append_claim_lines(&mut human, plan);
    if dry_run {
        human.push_str("dry run: no files written\n");
    }
    human
}

fn slug_applied_human(plan: &Plan, commit: &str) -> String {
    let mut human = format!(
        "migrated {}, {}, and {}\n",
        plural(plan.task_count, "task", "tasks"),
        plural(plan.subtask_count, "SubTask", "SubTasks"),
        plural(plan.comment_files.len(), "comment file", "comment files"),
    );
    human.push_str(&format!(
        "prefix     {} -> {}\n",
        plan.old_prefix, plan.new_prefix
    ));
    human.push_str(&format!("commit     {}\n", short_commit(commit)));
    append_claim_lines(&mut human, plan);
    human
}

fn append_claim_lines(human: &mut String, plan: &Plan) {
    if plan.claims.is_empty() {
        return;
    }
    human.push_str("claims:\n");
    for claim in &plan.claims {
        let lease = claim
            .lease_until
            .map(|until| format!(", lease until {}", until.to_rfc3339()))
            .unwrap_or_default();
        human.push_str(&format!(
            "  {} -> {} ({}{})\n",
            claim.from, claim.to, claim.holder, lease
        ));
    }
}

fn plural(count: usize, singular: &str, plural: &str) -> String {
    format!("{count} {}", if count == 1 { singular } else { plural })
}

fn short_commit(commit: &str) -> &str {
    commit.get(..7).unwrap_or(commit)
}

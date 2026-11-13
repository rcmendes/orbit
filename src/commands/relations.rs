use chrono::Utc;
use serde_json::{Value, json};
use std::collections::BTreeMap;

use crate::cli::{
    CommentAddArgs, CommentArgs, CommentListArgs, DependArgs, LabelArgs, LabelMutationArgs,
    UndependArgs,
};
use crate::comment::Comment;
use crate::graph::LoadMode;

use super::*;

pub(super) fn cmd_label(board: &Board, args: LabelArgs) -> Result<crate::output::CommandOutput> {
    match args.command {
        LabelCommand::Add(args) => mutate_labels(board, args, true),
        LabelCommand::Remove(args) => mutate_labels(board, args, false),
        LabelCommand::List => list_labels(board),
    }
}

fn mutate_labels(
    board: &Board,
    args: LabelMutationArgs,
    add: bool,
) -> Result<crate::output::CommandOutput> {
    let labels = normalize_labels(&args.labels)?;
    let (mut graph, _) = Graph::load(board, LoadMode::Strict)?;
    let (to_add, to_remove): (&[String], &[String]) =
        if add { (&labels, &[]) } else { (&[], &labels) };
    let changed = crate::ops::label(&mut graph, &args.id, to_add, to_remove, Utc::now())?;
    let task = graph.get(&args.id)?.clone();

    if !changed {
        return Ok(crate::output::CommandOutput::new(
            json!({ "id": args.id, "labels": task.front.labels, "changed": false }),
            format!("{} labels unchanged", args.id),
        ));
    }

    let warning = crate::mutation::save(
        board,
        &task,
        &format!(
            "orbit: label {} {}",
            if add { "add" } else { "remove" },
            args.id
        ),
    )?;
    let mut output = crate::output::CommandOutput::new(
        json!({ "id": args.id, "labels": task.front.labels, "changed": true }),
        format!(
            "{} {}: {}",
            if add { "added to" } else { "removed from" },
            args.id,
            labels.join(", ")
        ),
    );
    output.warnings.extend(warning);
    Ok(output)
}

fn list_labels(board: &Board) -> Result<crate::output::CommandOutput> {
    let (graph, _) = Graph::load(board, LoadMode::Strict)?;
    let mut counts: BTreeMap<String, usize> = BTreeMap::new();
    for task in graph.tasks.values() {
        for label in &task.front.labels {
            *counts.entry(label.clone()).or_default() += 1;
        }
    }

    let labels: Vec<Value> = counts
        .iter()
        .map(|(label, count)| json!({ "label": label, "count": count }))
        .collect();
    let human = if labels.is_empty() {
        "no labels".to_string()
    } else {
        labels
            .iter()
            .map(|value| {
                format!(
                    "{}  {}",
                    value["label"].as_str().unwrap_or("?"),
                    value["count"].as_u64().unwrap_or(0)
                )
            })
            .collect::<Vec<_>>()
            .join("\n")
    };

    Ok(
        crate::output::CommandOutput::new(json!({ "labels": labels }), human)
            .with_meta(json!({ "count": counts.len() })),
    )
}

pub(super) fn cmd_comment(
    board: &Board,
    args: CommentArgs,
) -> Result<crate::output::CommandOutput> {
    match args.command {
        CommentCommand::Add(args) => cmd_comment_add(board, args),
        CommentCommand::List(args) => cmd_comment_list(board, args),
    }
}

fn cmd_comment_add(board: &Board, args: CommentAddArgs) -> Result<crate::output::CommandOutput> {
    let (mut graph, _) = Graph::load(board, LoadMode::Strict)?;
    graph.get(&args.id)?;
    let author = resolve_agent(args.by)?;
    let body = resolve_body(args.body, args.body_file)?;

    let now = Utc::now();
    let comments = crate::mutation::append_comment(board, &args.id, &author, &body, now)?;
    let comment = comments.last().expect("a comment was appended").clone();
    let task = {
        let task = graph.get_mut(&args.id)?;
        task.touch(now);
        task.clone()
    };
    let warning = crate::mutation::save(board, &task, &format!("orbit: comment {}", args.id))?;
    let mut output = crate::output::CommandOutput::new(
        json!({
            "id": args.id,
            "comment": comment,
            "count": comments.len()
        }),
        format!("commented {} as {author}", args.id),
    );
    output.warnings.extend(warning);
    Ok(output)
}

fn cmd_comment_list(board: &Board, args: CommentListArgs) -> Result<crate::output::CommandOutput> {
    let (graph, _) = Graph::load(board, LoadMode::Strict)?;
    graph.get(&args.id)?;
    let comments = board.read_comments(&args.id)?;
    let human = if comments.is_empty() {
        format!("{} has no comments", args.id)
    } else {
        comments
            .iter()
            .map(format_comment)
            .collect::<Vec<_>>()
            .join("\n\n")
    };

    Ok(
        crate::output::CommandOutput::new(json!({ "id": args.id, "comments": comments }), human)
            .with_meta(json!({ "count": comments.len() })),
    )
}

fn format_comment(comment: &Comment) -> String {
    format!(
        "{}  {}\n{}",
        comment.created_at.to_rfc3339(),
        comment.author,
        comment.body
    )
}

pub(super) fn cmd_depend(board: &Board, args: DependArgs) -> Result<crate::output::CommandOutput> {
    let (mut graph, _) = Graph::load(board, LoadMode::Strict)?;
    if !crate::ops::depend(&mut graph, &args.id, &args.dep_id, Utc::now())? {
        let task = graph.get(&args.id)?;
        return Ok(crate::output::CommandOutput::new(
            graph::task_detail(&graph, task),
            format!("{} already depends on {}", args.id, args.dep_id),
        )
        .with_meta(json!({ "changed": false })));
    }
    let task = graph.get(&args.id)?.clone();
    let warning = crate::mutation::save(board, &task, &format!("orbit: depend {}", args.id))?;
    Ok(task_output(
        &graph,
        &task,
        format!("{} now depends on {}", args.id, args.dep_id),
        warning,
    ))
}

pub(super) fn cmd_undepend(
    board: &Board,
    args: UndependArgs,
) -> Result<crate::output::CommandOutput> {
    let (mut graph, _) = Graph::load(board, LoadMode::Lenient)?;
    if !crate::ops::undepend(&mut graph, &args.id, &args.dep_id, Utc::now())? {
        let task = graph.get(&args.id)?;
        return Ok(crate::output::CommandOutput::new(
            graph::task_detail(&graph, task),
            format!("{} does not depend on {}", args.id, args.dep_id),
        )
        .with_meta(json!({ "changed": false })));
    }
    let task = graph.get(&args.id)?.clone();
    let warning = crate::mutation::save(board, &task, &format!("orbit: undepend {}", args.id))?;
    Ok(task_output(
        &graph,
        &task,
        format!("{} no longer depends on {}", args.id, args.dep_id),
        warning,
    ))
}

use chrono::Utc;
use serde_json::{Value, json};
use std::collections::BTreeMap;

use crate::cli::{
    CommentAddArgs, CommentArgs, CommentListArgs, DependArgs, LabelArgs, LabelMutationArgs,
    LinkAddArgs, LinkCommand, LinkRemoveArgs, LinkTargetArgs, UndependArgs,
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
    // The comment sidecar is named by the immutable internal ID.
    let id = graph.resolve(&args.id)?;
    let author = resolve_agent(args.by)?;
    let body = resolve_body(args.body, args.body_file)?;

    let now = Utc::now();
    let comments = crate::mutation::append_comment(board, &id, &author, &body, now)?;
    let comment = comments.last().expect("a comment was appended").clone();
    let task = {
        let task = graph.get_mut(&id)?;
        task.touch(now);
        task.clone()
    };
    let warning = crate::mutation::save_as(
        board,
        &task,
        &format!("orbit: comment {id}"),
        Some(author.as_str()),
    )?;
    let mut output = crate::output::CommandOutput::new(
        json!({
            "id": id,
            "comment": comment,
            "count": comments.len()
        }),
        format!("commented {id} as {author}"),
    );
    output.warnings.extend(warning);
    Ok(output)
}

fn cmd_comment_list(board: &Board, args: CommentListArgs) -> Result<crate::output::CommandOutput> {
    let (graph, _) = Graph::load(board, LoadMode::Strict)?;
    let id = graph.resolve(&args.id)?;
    let comments = board.read_comments(&id)?;
    let human = if comments.is_empty() {
        format!("{id} has no comments")
    } else {
        comments
            .iter()
            .map(format_comment)
            .collect::<Vec<_>>()
            .join("\n\n")
    };

    Ok(
        crate::output::CommandOutput::new(json!({ "id": id, "comments": comments }), human)
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

pub(super) fn cmd_link(
    board: &Board,
    args: crate::cli::LinkArgs,
) -> Result<crate::output::CommandOutput> {
    match args.command {
        LinkCommand::Add(args) => cmd_link_add(board, args),
        LinkCommand::Remove(args) => cmd_link_remove(board, args),
        LinkCommand::List(args) => cmd_link_list(board, args),
    }
}

fn link_target(
    to: Option<&str>,
    external: Option<&str>,
    url: Option<String>,
) -> Result<crate::task::LinkTarget> {
    let mut target = crate::ops::link_target(to, external)?;
    if let (crate::task::LinkTarget::External(reference), Some(url)) = (&mut target, url) {
        reference.url = Some(url);
    }
    Ok(target)
}

fn cmd_link_add(board: &Board, args: LinkAddArgs) -> Result<crate::output::CommandOutput> {
    let kind = crate::task::normalize_link_kind(&args.kind)?;
    let target = link_target(args.to.as_deref(), args.external.as_deref(), args.url)?;
    let blocking = if args.no_blocking {
        Some(false)
    } else if args.blocking {
        Some(true)
    } else {
        None
    };
    let attributes = crate::ops::link_attributes(&args.attribute)?;
    let link = crate::task::Link {
        kind,
        target,
        blocking,
        attributes,
    };
    let (mut graph, _) = Graph::load(board, LoadMode::Strict)?;
    if !crate::ops::link(&mut graph, &args.id, link, Utc::now())? {
        let task = graph.get(&args.id)?;
        return Ok(crate::output::CommandOutput::new(
            graph::task_detail(&graph, task),
            format!("{} already has that link", args.id),
        )
        .with_meta(json!({ "changed": false })));
    }
    let task = graph.get(&args.id)?.clone();
    let warning = crate::mutation::save(board, &task, &format!("orbit: link {}", args.id))?;
    Ok(task_output(
        &graph,
        &task,
        format!("linked {}", args.id),
        warning,
    ))
}

fn cmd_link_remove(board: &Board, args: LinkRemoveArgs) -> Result<crate::output::CommandOutput> {
    let kind = args
        .kind
        .as_deref()
        .map(crate::task::normalize_link_kind)
        .transpose()?;
    let target = link_target(args.to.as_deref(), args.external.as_deref(), None)?;
    let (mut graph, _) = Graph::load(board, LoadMode::Lenient)?;
    if !crate::ops::unlink(&mut graph, &args.id, kind.as_deref(), &target, Utc::now())? {
        let task = graph.get(&args.id)?;
        return Ok(crate::output::CommandOutput::new(
            graph::task_detail(&graph, task),
            format!("{} has no matching link", args.id),
        )
        .with_meta(json!({ "changed": false })));
    }
    let task = graph.get(&args.id)?.clone();
    let warning = crate::mutation::save(board, &task, &format!("orbit: unlink {}", args.id))?;
    Ok(task_output(
        &graph,
        &task,
        format!("unlinked {}", args.id),
        warning,
    ))
}

fn cmd_link_list(board: &Board, args: LinkTargetArgs) -> Result<crate::output::CommandOutput> {
    let (graph, _) = Graph::load(board, LoadMode::Strict)?;
    let task = graph.get(&args.id)?;
    let links: Vec<Value> = task
        .front
        .links
        .iter()
        .map(crate::task::Link::to_json)
        .collect();
    let human = if task.front.links.is_empty() {
        format!("{} has no links", args.id)
    } else {
        task.front
            .links
            .iter()
            .map(|link| {
                let target = match &link.target {
                    crate::task::LinkTarget::Task(id) => graph.display_id(id),
                    other => other.label(),
                };
                let mut line = format!("{} {} ({})", link.inward_name(), target, link.kind);
                if link.is_blocking() {
                    line.push_str(" [blocking]");
                }
                line
            })
            .collect::<Vec<_>>()
            .join("\n")
    };
    Ok(
        crate::output::CommandOutput::new(json!({ "id": args.id, "links": links }), human)
            .with_meta(json!({ "count": links.len() })),
    )
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

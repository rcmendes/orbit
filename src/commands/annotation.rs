use chrono::Utc;
use serde_json::json;

use crate::cli::{AnnotationArgs, AnnotationCommand};
use crate::graph::LoadMode;

use super::*;

pub(super) fn cmd_annotation(
    board: &Board,
    args: AnnotationArgs,
) -> Result<crate::output::CommandOutput> {
    let (id, owner, value) = match args.command {
        AnnotationCommand::Set(args) => {
            let value: serde_json::Value = serde_json::from_str(&args.value).map_err(|error| {
                OrbitError::new(
                    code::INVALID_ANNOTATION,
                    format!("annotation value is not valid JSON: {error}"),
                )
            })?;
            (args.id, args.owner, Some(value))
        }
        AnnotationCommand::Remove(args) => (args.id, args.owner, None),
    };
    let (mut graph, _) = Graph::load(board, LoadMode::Strict)?;
    let changed = crate::ops::annotate(&mut graph, &id, &owner, value, Utc::now())?;
    let task = graph.get(&id)?.clone();
    let annotation = task.front.annotations.get(&owner).cloned();
    let mut output = crate::output::CommandOutput::new(
        json!({ "id": id, "owner": owner, "annotation": annotation, "changed": changed }),
        if changed {
            format!("{id}: annotation {owner} updated")
        } else {
            format!("{id}: annotation {owner} unchanged")
        },
    );
    if changed {
        let warning =
            crate::mutation::save(board, &task, &format!("orbit: annotate {id} {owner}"))?;
        output.warnings.extend(warning);
    }
    Ok(output)
}

use chrono::Utc;
use serde_json::json;

use crate::cli::{PropertyArgs, PropertyCommand};
use crate::graph::LoadMode;

use super::*;

pub(super) fn cmd_property(
    board: &Board,
    args: PropertyArgs,
) -> Result<crate::output::CommandOutput> {
    let (id, owner, value) = match args.command {
        PropertyCommand::Set(args) => {
            let value: serde_json::Value = serde_json::from_str(&args.value).map_err(|error| {
                OrbitError::new(
                    code::INVALID_PROPERTY,
                    format!("property value is not valid JSON: {error}"),
                )
            })?;
            (args.id, args.owner, Some(value))
        }
        PropertyCommand::Remove(args) => (args.id, args.owner, None),
    };
    let (mut graph, _) = Graph::load(board, LoadMode::Strict)?;
    let changed = crate::ops::property(&mut graph, &id, &owner, value, Utc::now())?;
    let task = graph.get(&id)?.clone();
    let property = task.front.properties.get(&owner).cloned();
    let mut output = crate::output::CommandOutput::new(
        json!({ "id": id, "owner": owner, "property": property, "changed": changed }),
        if changed {
            format!("{id}: property {owner} updated")
        } else {
            format!("{id}: property {owner} unchanged")
        },
    );
    if changed {
        let warning =
            crate::mutation::save(board, &task, &format!("orbit: property {id} {owner}"))?;
        output.warnings.extend(warning);
    }
    Ok(output)
}

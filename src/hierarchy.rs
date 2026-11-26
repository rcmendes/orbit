//! Issue types and the configurable parent hierarchy (ADR 0022).
//!
//! A Board declares its Issue types under `[[board.types]]` and orders them
//! into hierarchy levels under `[[board.hierarchy]]`, top level first. A
//! Task may carry `parent` only when its type sits on a strictly deeper level
//! than the parent's type; the number of levels is the maximum depth. `type`
//! is no longer structural metadata: it decides where in the hierarchy a Task
//! may sit, while `parent` is an ordinary field that never changes the ID.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::board::BoardConfig;
use crate::error::{OrbitError, Result, code};
use crate::task::{self, Task};

/// The fallback Issue type when `board.default_type` is absent.
pub fn default_issue_type() -> String {
    "task".to_string()
}

/// The default Issue type vocabulary, in configuration order.
pub fn default_issue_types() -> Vec<IssueTypeConfig> {
    ["epic", "story", "task", "bug", "sub-task"]
        .iter()
        .map(|name| IssueTypeConfig {
            name: name.to_string(),
            required: Vec::new(),
        })
        .collect()
}

/// The default hierarchy: `epic` above the working types above `sub-task`.
pub fn default_hierarchy() -> Vec<HierarchyLevelConfig> {
    vec![
        HierarchyLevelConfig {
            types: vec!["epic".to_string()],
        },
        HierarchyLevelConfig {
            types: vec!["story".to_string(), "task".to_string(), "bug".to_string()],
        },
        HierarchyLevelConfig {
            types: vec!["sub-task".to_string()],
        },
    ]
}

/// One configured Issue type (`[[board.types]]`). `name` is the value stored
/// in the Task's `type:` frontmatter; `required` lists frontmatter fields the
/// type demands — see [`required_field_present`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct IssueTypeConfig {
    pub name: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub required: Vec<String>,
}

/// One level of the hierarchy (`[[board.hierarchy]]`): the Issue types that
/// may sit at this depth. Levels are ordered top to bottom, so a Task's level
/// index is its depth from the top of the hierarchy.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HierarchyLevelConfig {
    pub types: Vec<String>,
}

/// The resolved per-Board Issue model: the type vocabulary, each type's
/// required fields, and the ordered hierarchy levels. `Graph` carries one so
/// validation, readiness, and the ops layer all agree on the rules.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Hierarchy {
    types: Vec<IssueTypeConfig>,
    levels: Vec<Vec<String>>,
    level_of: BTreeMap<String, usize>,
    default_type: String,
}

impl Default for Hierarchy {
    fn default() -> Self {
        Self::from_parts(
            default_issue_types(),
            default_issue_type(),
            default_hierarchy(),
        )
        .expect("the default Issue model is valid")
    }
}

impl Hierarchy {
    /// Builds the resolved model from raw config pieces, validating that the
    /// vocabulary and the hierarchy agree: every type is named once, every
    /// level is non-empty, and every level names a configured type.
    pub fn from_parts(
        types: Vec<IssueTypeConfig>,
        default_type: String,
        levels: Vec<HierarchyLevelConfig>,
    ) -> Result<Self> {
        if types.is_empty() {
            return Err(OrbitError::config("board.types must not be empty"));
        }
        let mut seen = std::collections::BTreeSet::new();
        for issue_type in &types {
            if !task::is_valid_label(&issue_type.name) {
                return Err(OrbitError::config(format!(
                    "board.types contains invalid type name `{}`",
                    issue_type.name
                )));
            }
            if !seen.insert(issue_type.name.as_str()) {
                return Err(OrbitError::config(format!(
                    "board.types contains duplicate type `{}`",
                    issue_type.name
                )));
            }
            for field in &issue_type.required {
                if !task::is_valid_label(field) {
                    return Err(OrbitError::config(format!(
                        "board.types `{}` has invalid required field `{field}`",
                        issue_type.name
                    )));
                }
            }
        }
        if !seen.contains(default_type.as_str()) {
            return Err(OrbitError::config(format!(
                "board.default_type `{default_type}` is not in board.types"
            )));
        }
        if levels.is_empty() {
            return Err(OrbitError::config("board.hierarchy must not be empty"));
        }
        let mut level_of = BTreeMap::new();
        let mut resolved_levels = Vec::with_capacity(levels.len());
        for (index, level) in levels.iter().enumerate() {
            if level.types.is_empty() {
                return Err(OrbitError::config(format!(
                    "board.hierarchy level {} must name at least one type",
                    index + 1
                )));
            }
            let mut resolved = Vec::with_capacity(level.types.len());
            for name in &level.types {
                let name = name.trim().to_lowercase();
                if !seen.contains(name.as_str()) {
                    return Err(OrbitError::config(format!(
                        "board.hierarchy level {} names unknown type `{name}`",
                        index + 1
                    )));
                }
                if level_of.insert(name.clone(), index).is_some() {
                    return Err(OrbitError::config(format!(
                        "board.hierarchy places type `{name}` on more than one level"
                    )));
                }
                resolved.push(name);
            }
            resolved_levels.push(resolved);
        }
        for issue_type in &types {
            if !level_of.contains_key(issue_type.name.as_str()) {
                return Err(OrbitError::config(format!(
                    "board.hierarchy does not place type `{}` on any level",
                    issue_type.name
                )));
            }
        }
        Ok(Self {
            types,
            levels: resolved_levels,
            level_of,
            default_type,
        })
    }

    /// The resolved model for a Board's `[board]` config section.
    pub fn from_config(config: &BoardConfig) -> Result<Self> {
        Self::from_parts(
            config.types.clone(),
            config.default_type.clone(),
            config.hierarchy.clone(),
        )
    }

    /// The configured Issue types, in `board.types` order.
    pub fn types(&self) -> &[IssueTypeConfig] {
        &self.types
    }

    /// The type a new Task gets when neither `--type` nor `--parent` applies.
    pub fn default_type(&self) -> &str {
        &self.default_type
    }

    /// The type a `--parent` add falls back to when `--type` is absent: the
    /// first type on the deepest level, the leaf role of the hierarchy.
    pub fn default_child_type(&self) -> &str {
        self.levels
            .last()
            .and_then(|level| level.first())
            .map(String::as_str)
            .unwrap_or(&self.default_type)
    }

    /// Whether `name` is a configured Issue type.
    pub fn is_known_type(&self, name: &str) -> bool {
        self.level_of.contains_key(name)
    }

    /// The type's configuration, when `name` is configured.
    pub fn type_config(&self, name: &str) -> Option<&IssueTypeConfig> {
        self.types.iter().find(|issue_type| issue_type.name == name)
    }

    /// The type's level index: 0 is the top of the hierarchy.
    pub fn level_of(&self, name: &str) -> Option<usize> {
        self.level_of.get(name).copied()
    }

    /// The maximum number of parent hops the hierarchy permits.
    pub fn max_depth(&self) -> usize {
        self.levels.len()
    }

    /// Whether a Task of type `child` may carry `parent` of type `parent`:
    /// both types must be configured and the child must sit strictly deeper.
    pub fn allows_parent(&self, child: &str, parent: &str) -> bool {
        match (self.level_of(child), self.level_of(parent)) {
            (Some(child), Some(parent)) => child > parent,
            _ => false,
        }
    }

    /// Why `child` may not sit under `parent`, phrased for an error message.
    /// Returns `None` when the hierarchy permits the edge (or a type is
    /// unknown — unknown types are reported separately).
    pub fn parent_rule_violation(&self, child: &str, parent: &str) -> Option<String> {
        let child_level = self.level_of(child)?;
        let parent_level = self.level_of(parent)?;
        if child_level > parent_level {
            return None;
        }
        Some(format!(
            "type `{child}` cannot be a child of type `{parent}`; the Board hierarchy orders `{parent}` at level {} and `{child}` at level {}",
            parent_level + 1,
            child_level + 1
        ))
    }

    /// The configured required fields the Task does not satisfy.
    pub fn missing_required_fields(&self, task: &Task) -> Vec<String> {
        self.type_config(&task.front.task_type)
            .map(|issue_type| {
                issue_type
                    .required
                    .iter()
                    .filter(|field| !required_field_present(task, field))
                    .cloned()
                    .collect()
            })
            .unwrap_or_default()
    }

    /// The human-readable type names for error messages.
    pub fn type_names(&self) -> Vec<&str> {
        self.types
            .iter()
            .map(|issue_type| issue_type.name.as_str())
            .collect()
    }
}

/// Whether one required field is present on a Task. Known names map onto the
/// typed frontmatter fields; any other name is looked up in the frontmatter's
/// `extra` map so Boards can require their own custom fields. Shared with the
/// `[policy]` rule validators (`require_fields`).
pub(crate) fn required_field_present(task: &Task, field: &str) -> bool {
    let front = &task.front;
    match field {
        "title" => !front.title.trim().is_empty(),
        "status" | "created_at" | "updated_at" | "type" => true,
        "priority" => !front.priority.trim().is_empty(),
        "component" => front
            .component
            .as_deref()
            .is_some_and(|v| !v.trim().is_empty()),
        "verification" => front
            .verification
            .as_deref()
            .is_some_and(|v| !v.trim().is_empty()),
        "repository" => front
            .repository
            .as_deref()
            .is_some_and(|v| !v.trim().is_empty()),
        "labels" => !front.labels.is_empty(),
        "links" => !front.links.is_empty(),
        "parent" => front.parent.is_some(),
        "checkpoint" => front.checkpoint.is_some(),
        "assignee" => front.assignee.is_some(),
        "source_key" => front.source_key.is_some(),
        "remote" => front.remote.is_some(),
        "github" => front.github.is_some(),
        "properties" => !front.properties.is_empty(),
        "external_id" => front
            .external_id
            .as_deref()
            .is_some_and(|v| !v.trim().is_empty()),
        "body" => !task.body.trim().is_empty(),
        other => front.extra.get(other).is_some_and(|value| match value {
            serde_yaml_ng::Value::Null => false,
            serde_yaml_ng::Value::String(text) => !text.trim().is_empty(),
            _ => true,
        }),
    }
}

/// Normalizes a type name the way the config stores it: trimmed lowercase.
pub fn normalize_type_name(input: &str) -> String {
    input.trim().to_lowercase()
}

/// The error for an unconfigured type name.
pub fn unknown_type_error(hierarchy: &Hierarchy, name: &str) -> OrbitError {
    OrbitError::new(
        code::INVALID_TYPE,
        format!(
            "unknown type `{name}`; the Board configures: {}",
            hierarchy.type_names().join(", ")
        ),
    )
}

/// The error for a required field the Task lacks.
pub fn missing_field_error(id: &str, fields: &[String]) -> OrbitError {
    OrbitError::new(
        code::MISSING_REQUIRED_FIELD,
        format!("{id} is missing required field(s): {}", fields.join(", ")),
    )
    .with_details(serde_json::json!({ "issue": id, "fields": fields }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::task::{TaskFrontmatter, default_priority};
    use chrono::{TimeZone, Utc};
    use std::collections::BTreeMap;

    fn task(id: &str, ty: &str) -> Task {
        let now = Utc.with_ymd_and_hms(2026, 11, 16, 12, 0, 0).unwrap();
        Task {
            id: id.to_string(),
            front: TaskFrontmatter {
                title: id.to_string(),
                status: "todo".to_string(),
                key: None,
                key_aliases: Vec::new(),
                resolution: None,
                resolved_at: None,
                task_type: ty.to_string(),
                priority: default_priority(),
                labels: Vec::new(),
                links: Vec::new(),
                parent: None,
                checkpoint: None,
                component: None,
                verification: None,
                github: None,
                remote: None,
                repository: None,
                external_id: None,
                assignee: None,
                lease: None,
                reporter: None,
                source_key: None,
                properties: BTreeMap::new(),
                created_at: now,
                updated_at: now,
                extra: BTreeMap::new(),
            },
            body: String::new(),
        }
    }

    #[test]
    fn default_model_matches_the_documented_shape() {
        let hierarchy = Hierarchy::default();
        assert_eq!(hierarchy.default_type(), "task");
        assert_eq!(hierarchy.default_child_type(), "sub-task");
        assert_eq!(hierarchy.max_depth(), 3);
        assert!(hierarchy.allows_parent("task", "epic"));
        assert!(hierarchy.allows_parent("sub-task", "task"));
        assert!(hierarchy.allows_parent("sub-task", "epic"));
        assert!(!hierarchy.allows_parent("task", "task"));
        assert!(!hierarchy.allows_parent("epic", "task"));
        assert!(!hierarchy.allows_parent("sub-task", "sub-task"));
        assert!(hierarchy.parent_rule_violation("task", "task").is_some());
        assert!(
            hierarchy
                .parent_rule_violation("sub-task", "epic")
                .is_none()
        );
    }

    #[test]
    fn from_parts_rejects_inconsistent_models() {
        let types = default_issue_types();
        // A type missing from the hierarchy.
        let levels = vec![
            HierarchyLevelConfig {
                types: vec!["epic".to_string()],
            },
            HierarchyLevelConfig {
                types: vec!["task".to_string()],
            },
        ];
        assert!(Hierarchy::from_parts(types.clone(), "task".to_string(), levels).is_err());

        // A level naming an unknown type.
        let levels = vec![
            HierarchyLevelConfig {
                types: vec!["epic".to_string(), "mystery".to_string()],
            },
            HierarchyLevelConfig {
                types: vec!["story".to_string(), "task".to_string(), "bug".to_string()],
            },
            HierarchyLevelConfig {
                types: vec!["sub-task".to_string()],
            },
        ];
        assert!(Hierarchy::from_parts(types, "task".to_string(), levels).is_err());
    }

    #[test]
    fn required_fields_cover_known_and_extra_names() {
        let types = vec![IssueTypeConfig {
            name: "task".to_string(),
            required: vec!["component".to_string(), "cycle".to_string()],
        }];
        let levels = vec![HierarchyLevelConfig {
            types: vec!["task".to_string()],
        }];
        let hierarchy = Hierarchy::from_parts(types, "task".to_string(), levels).unwrap();

        let mut task = task("orb-aaaaaa", "task");
        assert_eq!(
            hierarchy.missing_required_fields(&task),
            vec!["component".to_string(), "cycle".to_string()]
        );

        task.front.component = Some("backend".to_string());
        task.front.extra.insert(
            "cycle".to_string(),
            serde_yaml_ng::Value::String("Q3".to_string()),
        );
        assert!(hierarchy.missing_required_fields(&task).is_empty());
    }
}

//! The Board-defined workflow (ADR 0022): named statuses carrying a fixed
//! category, named transitions between them, and a resolution recorded when
//! a transition closes a Task.
//!
//! A [`Workflow`] is pure data resolved once from `config.toml`: either a
//! shipped template (`minimal`, `standard`, `agentic`) or a fully custom
//! `[workflow]` section. Everything that needs status semantics — readiness,
//! terminal checks, transition legality — asks the workflow instead of
//! hard-coding status names.

use serde::{Deserialize, Serialize};

use crate::board::Config;
use crate::error::{OrbitError, Result};

/// The fixed lifecycle bucket a status belongs to. Categories never change;
/// Boards name the statuses inside them.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StatusCategory {
    /// Not started; may or may not be the Ready status.
    Todo,
    /// Claimed or otherwise being worked.
    Active,
    /// Successfully closed.
    Done,
    /// Closed without completing.
    Canceled,
}

impl StatusCategory {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Todo => "todo",
            Self::Active => "active",
            Self::Done => "done",
            Self::Canceled => "canceled",
        }
    }

    /// Closed categories end the lifecycle; `reopen`-style transitions leave them.
    pub fn is_terminal(self) -> bool {
        matches!(self, Self::Done | Self::Canceled)
    }
}

/// One named status in the workflow. Exactly one status per workflow may set
/// `ready`, marking the status `orbit ready` lists.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StatusDef {
    pub name: String,
    pub category: StatusCategory,
    /// The Ready status: `orbit ready` requires this status plus the derived
    /// non-blocked conditions (unclaimed, not waiting, no hold, no open
    /// blocking links or SubTasks).
    #[serde(default)]
    pub ready: bool,
}

/// A named edge between statuses. `from` lists the statuses the transition
/// starts from; `"*"` matches every status. `resolution` is recorded on the
/// Task when the transition targets a closed (done/canceled) status and is
/// required to be absent otherwise.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TransitionDef {
    pub name: String,
    #[serde(default)]
    pub from: Vec<String>,
    pub to: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub resolution: Option<String>,
}

impl TransitionDef {
    /// Whether this transition may run from `status`.
    pub fn allows_from(&self, status: &str) -> bool {
        self.from.iter().any(|from| from == "*" || from == status)
    }
}

/// A resolved workflow: the Board's statuses and transitions plus the
/// conventions commands rely on (`claim`, `release`, `done`, `cancel`,
/// `reopen` are looked up by name).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Workflow {
    pub statuses: Vec<StatusDef>,
    pub transitions: Vec<TransitionDef>,
    /// Status new Tasks start in.
    pub initial: String,
}

/// The `[workflow]` section of `config.toml`. When `statuses` is empty the
/// `template` name selects a shipped workflow; otherwise the section fully
/// defines the workflow and `template` is ignored.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct WorkflowConfig {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub template: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub initial: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub statuses: Vec<StatusDef>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub transitions: Vec<TransitionDef>,
}

impl WorkflowConfig {
    /// True when no `[workflow]` section was configured; keeps `config.toml`
    /// clean for Boards on the default template.
    pub fn is_default(&self) -> bool {
        self.template.is_none()
            && self.initial.is_none()
            && self.statuses.is_empty()
            && self.transitions.is_empty()
    }

    /// Cheap syntactic check run by `Config::validate`; the full semantic
    /// validation (references, categories, resolution placement) runs in
    /// [`Workflow::from_config`].
    pub fn validate(&self) -> Result<()> {
        if let Some(template) = &self.template
            && template_from_name(template).is_none()
            && self.statuses.is_empty()
        {
            return Err(OrbitError::config(format!(
                "workflow.template `{template}` is unknown; expected minimal, standard, or agentic"
            )));
        }
        Ok(())
    }
}

/// Names of the transitions the built-in lifecycle commands look up.
pub const TRANSITION_CLAIM: &str = "claim";
pub const TRANSITION_RELEASE: &str = "release";
pub const TRANSITION_DONE: &str = "done";
pub const TRANSITION_CANCEL: &str = "cancel";
pub const TRANSITION_REOPEN: &str = "reopen";
/// Review lifecycle (ADR 0022): `submit` moves work into the In Review
/// status, `approve` and `request_changes` are the reviewer verdicts, `ask`
/// escalates a question to `needs_human`, and `escalate` is the generic
/// escalation edge attempt limits and other policy guards reuse.
pub const TRANSITION_SUBMIT: &str = "submit";
pub const TRANSITION_APPROVE: &str = "approve";
pub const TRANSITION_REQUEST_CHANGES: &str = "request_changes";
pub const TRANSITION_ASK: &str = "ask";
pub const TRANSITION_ESCALATE: &str = "escalate";
/// Fallback edge out of the escalation hold; `orbit answer` prefers
/// restoring the Ask's recorded `return_to` status and only uses this
/// transition when that status is gone.
pub const TRANSITION_ANSWER: &str = "answer";

/// Status names the shipped templates and remote adapters prefer when a
/// category has several candidates.
const OPEN_STATUS: &str = "todo";
const ACTIVE_STATUS: &str = "in_progress";
const DONE_STATUS: &str = "done";
const CANCELLED_STATUS: &str = "cancelled";

impl Workflow {
    /// Resolve the workflow for a Board config: a custom `[workflow]`
    /// definition when `statuses` are configured, else the named template
    /// (default `standard`).
    pub fn from_config(config: &Config) -> Result<Self> {
        let section = &config.workflow;
        let mut workflow = if section.statuses.is_empty() {
            let name = section.template.as_deref().unwrap_or("standard");
            template_from_name(name).ok_or_else(|| {
                OrbitError::config(format!(
                    "workflow.template `{name}` is unknown; expected minimal, standard, or agentic"
                ))
            })?
        } else {
            Workflow {
                statuses: section.statuses.clone(),
                transitions: section.transitions.clone(),
                initial: String::new(),
            }
        };
        if let Some(initial) = &section.initial {
            workflow.initial = initial.clone();
        } else if workflow.initial.is_empty() {
            // A custom `[workflow]` without `initial` starts Tasks in the
            // Ready status, else the first defined status.
            workflow.initial = workflow
                .statuses
                .iter()
                .find(|status| status.ready)
                .or_else(|| workflow.statuses.first())
                .map(|status| status.name.clone())
                .unwrap_or_default();
        }
        workflow.validate()?;
        Ok(workflow)
    }

    /// The minimal lifecycle: Ready, In Progress, Done, Cancelled.
    pub fn minimal() -> Self {
        Self::new(
            vec![
                status("ready", StatusCategory::Todo, true),
                status("in_progress", StatusCategory::Active, false),
                status("done", StatusCategory::Done, false),
                status("cancelled", StatusCategory::Canceled, false),
            ],
            vec![
                transition("claim", &["ready", "in_progress"], "in_progress", None),
                transition("release", &["in_progress"], "ready", None),
                transition("done", &["ready", "in_progress"], "done", Some("completed")),
                transition(
                    "cancel",
                    &["ready", "in_progress"],
                    "cancelled",
                    Some("cancelled"),
                ),
                transition("reopen", &["done", "cancelled"], "ready", None),
            ],
            "ready",
        )
    }

    /// The default workflow: minimal plus a `todo` backlog status Tasks can
    /// be parked in and promoted back from, an `in_review` status for the
    /// submit/review loop, and a `needs_human` hold `ask`/`escalate` land on.
    pub fn standard() -> Self {
        Self::new(
            vec![
                status("ready", StatusCategory::Todo, true),
                status("todo", StatusCategory::Todo, false),
                status("needs_human", StatusCategory::Todo, false),
                status("in_progress", StatusCategory::Active, false),
                status("in_review", StatusCategory::Active, false),
                status("done", StatusCategory::Done, false),
                status("cancelled", StatusCategory::Canceled, false),
            ],
            vec![
                transition("claim", &["ready", "in_progress"], "in_progress", None),
                transition("release", &["in_progress"], "ready", None),
                transition("promote", &["todo"], "ready", None),
                transition("defer", &["ready"], "todo", None),
                transition("submit", &["in_progress"], "in_review", None),
                transition(
                    TRANSITION_APPROVE,
                    &["in_review"],
                    "done",
                    Some("completed"),
                ),
                transition(
                    TRANSITION_REQUEST_CHANGES,
                    &["in_review"],
                    "in_progress",
                    None,
                ),
                transition(
                    TRANSITION_ASK,
                    &["in_progress", "in_review"],
                    "needs_human",
                    None,
                ),
                transition(
                    TRANSITION_ESCALATE,
                    &["in_progress", "in_review"],
                    "needs_human",
                    None,
                ),
                transition(TRANSITION_ANSWER, &["needs_human"], "ready", None),
                transition(
                    "done",
                    &["ready", "todo", "in_progress", "in_review", "needs_human"],
                    "done",
                    Some("completed"),
                ),
                transition(
                    "cancel",
                    &["ready", "todo", "in_progress", "in_review", "needs_human"],
                    "cancelled",
                    Some("cancelled"),
                ),
                transition("reopen", &["done", "cancelled"], "ready", None),
            ],
            "ready",
        )
    }

    /// The agent-oriented workflow: standard plus `blocked` and
    /// `needs_decision` hold statuses an agent can flag and resume from.
    /// `needs_decision` stays the agent's `escalate` target; `ask` and the
    /// attempt-limit path land on `needs_human`, answered by `orbit answer`.
    pub fn agentic() -> Self {
        Self::new(
            vec![
                status("ready", StatusCategory::Todo, true),
                status("todo", StatusCategory::Todo, false),
                status("blocked", StatusCategory::Todo, false),
                status("needs_decision", StatusCategory::Todo, false),
                status("needs_human", StatusCategory::Todo, false),
                status("in_progress", StatusCategory::Active, false),
                status("in_review", StatusCategory::Active, false),
                status("done", StatusCategory::Done, false),
                status("cancelled", StatusCategory::Canceled, false),
            ],
            vec![
                transition("claim", &["ready", "in_progress"], "in_progress", None),
                transition("release", &["in_progress"], "ready", None),
                transition("promote", &["todo"], "ready", None),
                transition("defer", &["ready"], "todo", None),
                transition("block", &["in_progress"], "blocked", None),
                transition(
                    "escalate",
                    &["in_progress", "in_review"],
                    "needs_decision",
                    None,
                ),
                transition(
                    "resume",
                    &["blocked", "needs_decision"],
                    "in_progress",
                    None,
                ),
                transition("submit", &["in_progress"], "in_review", None),
                transition(
                    TRANSITION_APPROVE,
                    &["in_review"],
                    "done",
                    Some("completed"),
                ),
                transition(
                    TRANSITION_REQUEST_CHANGES,
                    &["in_review"],
                    "in_progress",
                    None,
                ),
                transition(
                    TRANSITION_ASK,
                    &["in_progress", "in_review", "blocked", "needs_decision"],
                    "needs_human",
                    None,
                ),
                transition(TRANSITION_ANSWER, &["needs_human"], "ready", None),
                transition(
                    "done",
                    &[
                        "ready",
                        "todo",
                        "blocked",
                        "needs_decision",
                        "needs_human",
                        "in_progress",
                        "in_review",
                    ],
                    "done",
                    Some("completed"),
                ),
                transition(
                    "cancel",
                    &[
                        "ready",
                        "todo",
                        "blocked",
                        "needs_decision",
                        "needs_human",
                        "in_progress",
                        "in_review",
                    ],
                    "cancelled",
                    Some("cancelled"),
                ),
                transition("reopen", &["done", "cancelled"], "ready", None),
            ],
            "ready",
        )
    }

    fn new(statuses: Vec<StatusDef>, transitions: Vec<TransitionDef>, initial: &str) -> Self {
        Self {
            statuses,
            transitions,
            initial: initial.to_string(),
        }
    }

    /// Semantic validation: unique names, resolvable references, at most one
    /// Ready status, and resolutions only on transitions into closed
    /// categories.
    pub fn validate(&self) -> Result<()> {
        let invalid = |message: String| OrbitError::config(format!("workflow: {message}"));
        let mut names = std::collections::BTreeSet::new();
        let mut ready_count = 0;
        for status in &self.statuses {
            if status.name.trim().is_empty() {
                return Err(invalid("status names must not be empty".to_string()));
            }
            if !names.insert(status.name.as_str()) {
                return Err(invalid(format!(
                    "status `{}` is defined more than once",
                    status.name
                )));
            }
            if status.ready {
                ready_count += 1;
            }
        }
        if ready_count > 1 {
            return Err(invalid(
                "at most one status may be marked ready".to_string(),
            ));
        }
        if self.status(&self.initial).is_none() {
            return Err(invalid(format!(
                "initial status `{}` is not defined",
                self.initial
            )));
        }
        let mut transition_names = std::collections::BTreeSet::new();
        for transition in &self.transitions {
            if transition.name.trim().is_empty() {
                return Err(invalid("transition names must not be empty".to_string()));
            }
            if !transition_names.insert(transition.name.as_str()) {
                return Err(invalid(format!(
                    "transition `{}` is defined more than once",
                    transition.name
                )));
            }
            for from in &transition.from {
                if from != "*" && self.status(from).is_none() {
                    return Err(invalid(format!(
                        "transition `{}` starts from unknown status `{from}`",
                        transition.name
                    )));
                }
            }
            let Some(target) = self.status(&transition.to) else {
                return Err(invalid(format!(
                    "transition `{}` targets unknown status `{}`",
                    transition.name, transition.to
                )));
            };
            if transition.resolution.is_some() && !target.category.is_terminal() {
                return Err(invalid(format!(
                    "transition `{}` carries a resolution but `{}` is not a closed status",
                    transition.name, transition.to
                )));
            }
        }
        Ok(())
    }

    /// The status definition named `name`, if the workflow defines it.
    pub fn status(&self, name: &str) -> Option<&StatusDef> {
        self.statuses.iter().find(|status| status.name == name)
    }

    /// The category of `name`; `None` for statuses the workflow does not
    /// define (callers treat unknown names as non-terminal but report them).
    pub fn category_of(&self, name: &str) -> Option<StatusCategory> {
        self.status(name).map(|status| status.category)
    }

    /// Whether `name` is a closed (done/canceled-category) status.
    pub fn is_terminal(&self, name: &str) -> bool {
        self.category_of(name)
            .is_some_and(StatusCategory::is_terminal)
    }

    /// Whether `name` is the workflow's Ready status.
    pub fn is_ready_status(&self, name: &str) -> bool {
        self.status(name).is_some_and(|status| status.ready)
    }

    /// The status new Tasks start in.
    pub fn initial_status(&self) -> &str {
        &self.initial
    }

    /// The Ready status name, when the workflow defines one.
    pub fn ready_status(&self) -> Option<&str> {
        self.statuses
            .iter()
            .find(|status| status.ready)
            .map(|status| status.name.as_str())
    }

    /// The preferred "open, not started" status name: the Ready status when
    /// defined (a Remote-open Issue is workable, matching the historical
    /// `todo` mapping), else `todo`, else the first todo-category status.
    pub fn open_status(&self) -> &str {
        if let Some(ready) = self.ready_status() {
            return ready;
        }
        self.preferred(OPEN_STATUS, StatusCategory::Todo)
    }

    /// The preferred "being worked" status name: `in_progress` when defined,
    /// else the first active-category status.
    pub fn active_status(&self) -> &str {
        self.preferred(ACTIVE_STATUS, StatusCategory::Active)
    }

    /// The preferred done-category status name.
    pub fn done_status(&self) -> &str {
        self.preferred(DONE_STATUS, StatusCategory::Done)
    }

    /// The preferred canceled-category status name.
    pub fn cancelled_status(&self) -> &str {
        self.preferred(CANCELLED_STATUS, StatusCategory::Canceled)
    }

    fn preferred(&self, canonical: &'static str, category: StatusCategory) -> &str {
        if self
            .status(canonical)
            .is_some_and(|s| s.category == category)
        {
            return canonical;
        }
        self.statuses
            .iter()
            .find(|status| status.category == category)
            .map(|status| status.name.as_str())
            .unwrap_or(canonical)
    }

    /// The transition named `name`, if defined.
    pub fn transition_named(&self, name: &str) -> Option<&TransitionDef> {
        self.transitions.iter().find(|t| t.name == name)
    }

    /// The resolution a Task takes on when it lands on closed `status`: the one
    /// a transition into it records, else the category default. `None` for open
    /// statuses.
    pub fn resolution_for(&self, status: &str) -> Option<String> {
        let category = self.category_of(status)?;
        if !category.is_terminal() {
            return None;
        }
        self.transitions
            .iter()
            .find(|t| t.to == status && t.resolution.is_some())
            .and_then(|t| t.resolution.clone())
            .or_else(|| {
                Some(match category {
                    StatusCategory::Done => "completed".to_string(),
                    _ => "cancelled".to_string(),
                })
            })
    }

    /// Transitions that may run from `status`, in definition order.
    pub fn transitions_from(&self, status: &str) -> Vec<&TransitionDef> {
        self.transitions
            .iter()
            .filter(|transition| transition.allows_from(status))
            .collect()
    }

    /// Every defined transition name, for error messages.
    pub fn transition_names(&self) -> Vec<&str> {
        self.transitions
            .iter()
            .map(|transition| transition.name.as_str())
            .collect()
    }

    /// Every defined status name, for error messages.
    pub fn status_names(&self) -> Vec<&str> {
        self.statuses
            .iter()
            .map(|status| status.name.as_str())
            .collect()
    }
}

fn template_from_name(name: &str) -> Option<Workflow> {
    match name {
        "minimal" => Some(Workflow::minimal()),
        "standard" => Some(Workflow::standard()),
        "agentic" => Some(Workflow::agentic()),
        _ => None,
    }
}

fn status(name: &str, category: StatusCategory, ready: bool) -> StatusDef {
    StatusDef {
        name: name.to_string(),
        category,
        ready,
    }
}

fn transition(name: &str, from: &[&str], to: &str, resolution: Option<&str>) -> TransitionDef {
    TransitionDef {
        name: name.to_string(),
        from: from.iter().map(|from| from.to_string()).collect(),
        to: to.to_string(),
        resolution: resolution.map(str::to_string),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn templates_validate_and_cover_the_lifecycle() {
        for workflow in [
            Workflow::minimal(),
            Workflow::standard(),
            Workflow::agentic(),
        ] {
            workflow.validate().unwrap();
            assert!(workflow.ready_status().is_some());
            for name in [
                TRANSITION_CLAIM,
                TRANSITION_RELEASE,
                TRANSITION_DONE,
                TRANSITION_CANCEL,
                TRANSITION_REOPEN,
            ] {
                assert!(workflow.transition_named(name).is_some(), "{name}");
            }
            assert!(workflow.is_terminal(workflow.done_status()));
            assert!(workflow.is_terminal(workflow.cancelled_status()));
        }
    }

    #[test]
    fn transitions_from_filters_by_source() {
        let workflow = Workflow::standard();
        let names: Vec<&str> = workflow
            .transitions_from("ready")
            .iter()
            .map(|t| t.name.as_str())
            .collect();
        assert_eq!(names, ["claim", "defer", "done", "cancel"]);
        let names: Vec<&str> = workflow
            .transitions_from("done")
            .iter()
            .map(|t| t.name.as_str())
            .collect();
        assert_eq!(names, ["reopen"]);
    }

    #[test]
    fn custom_config_builds_a_workflow() {
        let config: Config = toml::from_str(
            r#"
[workflow]
template = "minimal"
"#,
        )
        .unwrap();
        let workflow = Workflow::from_config(&config).unwrap();
        assert_eq!(workflow.initial_status(), "ready");

        let custom: Config = toml::from_str(
            r#"
[[workflow.statuses]]
name = "new"
category = "todo"
ready = true

[[workflow.statuses]]
name = "finished"
category = "done"

[[workflow.transitions]]
name = "finish"
from = ["new"]
to = "finished"
resolution = "shipped"
"#,
        )
        .unwrap();
        let workflow = Workflow::from_config(&custom).unwrap();
        assert_eq!(workflow.initial_status(), "new");
        assert_eq!(workflow.done_status(), "finished");
        assert_eq!(
            workflow
                .transition_named("finish")
                .unwrap()
                .resolution
                .as_deref(),
            Some("shipped")
        );
    }

    #[test]
    fn rejects_resolution_on_open_targets_and_duplicate_statuses() {
        let config: Config = toml::from_str(
            r#"
[[workflow.statuses]]
name = "a"
category = "todo"

[[workflow.statuses]]
name = "a"
category = "todo"

[[workflow.transitions]]
name = "x"
from = ["a"]
to = "a"
resolution = "bad"
"#,
        )
        .unwrap();
        assert!(Workflow::from_config(&config).is_err());
    }
}

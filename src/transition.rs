//! Task lifecycle rules. A Task owns its transitions: callers pass the actor,
//! the clock, and the Board's [`Workflow`], and get back the changed Task or a
//! typed [`TransitionError`]. No I/O happens here. Context that lives in the
//! Graph (blocking dependencies, hold labels, done dependents) comes in as
//! arguments.
//!
//! Status changes go through the workflow's named transitions: `claim`,
//! `release`, `done`, `cancel`, and `reopen` are looked up by name, and any
//! transition into a closed (done/canceled) category records the transition's
//! `resolution` on the Task.

use chrono::{DateTime, Duration, Utc};
use serde_json::json;

use crate::error::{OrbitError, code};
use crate::task::{Checkpoint, Claim, Task};
use crate::workflow::{
    StatusCategory, TRANSITION_CANCEL, TRANSITION_CLAIM, TRANSITION_DONE, TRANSITION_RELEASE,
    TRANSITION_REOPEN, TransitionDef, Workflow,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Outcome {
    Changed,
    /// Already in the target state; nothing was modified.
    Unchanged,
}

/// Why a transition is not legal from the Task's current state (`INVALID_TRANSITION`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum InvalidReason {
    /// The workflow defines no transition with this name.
    UnknownTransition {
        name: String,
        defined: Vec<String>,
    },
    /// The transition exists but does not start from the current status.
    NotAllowed {
        name: String,
        status: String,
        valid: Vec<String>,
    },
    HeartbeatNotActive(String),
    HeartbeatNoClaim,
    CheckpointClosed(String),
    CheckpointNoClaim,
    ClaimClosed(String),
    CompleteCanceled(String),
    CancelDone(String),
    ReopenActive(String),
    ReopenDoneDependents(Vec<String>),
}

impl InvalidReason {
    fn into_error(self, id: &str) -> OrbitError {
        let (message, details) = match &self {
            Self::UnknownTransition { name, defined } => (
                format!(
                    "{id}: the workflow has no `{name}` transition{}",
                    if defined.is_empty() {
                        String::new()
                    } else {
                        format!("; defined transitions: {}", defined.join(", "))
                    }
                ),
                json!({ "task": id, "transition": name, "defined": defined }),
            ),
            Self::NotAllowed {
                name,
                status,
                valid,
            } => (
                format!(
                    "{id} is `{status}`; `{name}` is not allowed from here{}",
                    if valid.is_empty() {
                        "; no transitions are allowed from this status".to_string()
                    } else {
                        format!("; valid transitions: {}", valid.join(", "))
                    }
                ),
                json!({
                    "task": id,
                    "transition": name,
                    "status": status,
                    "valid_transitions": valid,
                }),
            ),
            Self::HeartbeatNotActive(status) => (
                format!("{id} is `{status}`; only active-category statuses can be heartbeated"),
                json!({ "task": id, "status": status }),
            ),
            Self::HeartbeatNoClaim => (format!("{id} has no claim"), json!({ "task": id })),
            Self::CheckpointClosed(status) => (
                format!(
                    "{id} is `{status}`; checkpoints only apply to open (non-terminal) statuses"
                ),
                json!({ "task": id, "status": status }),
            ),
            Self::CheckpointNoClaim => (
                format!("{id} has no claim; claim it before recording a checkpoint"),
                json!({ "task": id }),
            ),
            Self::ClaimClosed(status) => (
                format!("{id} is `{status}`; it cannot be claimed"),
                json!({ "task": id, "status": status }),
            ),
            Self::CompleteCanceled(status) => (
                format!("{id} is `{status}`; reopen it before completing it"),
                json!({ "task": id, "status": status }),
            ),
            Self::CancelDone(status) => (
                format!("{id} is `{status}`; reopen it before cancelling it"),
                json!({ "task": id, "status": status }),
            ),
            Self::ReopenActive(status) => (
                format!("{id} is `{status}`; only closed (done/canceled) statuses can be reopened"),
                json!({ "task": id, "status": status }),
            ),
            Self::ReopenDoneDependents(dependents) => (
                format!("{id} has done dependents: {}", dependents.join(", ")),
                json!({ "task": id, "done_dependents": dependents }),
            ),
        };
        OrbitError::new(code::INVALID_TRANSITION, message).with_details(details)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TransitionError {
    InvalidTransition {
        id: String,
        reason: InvalidReason,
    },
    NotClaimHolder {
        id: String,
        holder: String,
    },
    InvalidCheckpoint(String),
    AlreadyClaimed {
        id: String,
        holder: String,
        lease_until: Option<DateTime<Utc>>,
    },
    ClaimStale {
        id: String,
        holder: String,
        lease_until: Option<DateTime<Utc>>,
    },
    DependencyBlocked {
        id: String,
        blocking: Vec<String>,
    },
    SubTasksOpen {
        id: String,
        subtasks: Vec<String>,
    },
    WaitingExternal {
        id: String,
        waiting_on: Option<String>,
    },
    LabelBlocked {
        id: String,
        labels: Vec<String>,
    },
}

/// One optional text field in a checkpoint update.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum FieldUpdate<T> {
    #[default]
    Keep,
    Clear,
    Set(T),
}

/// A partial checkpoint write. `progress` omitted means keep the current value;
/// `next` and `blocker` can be kept, cleared, or replaced.
#[derive(Debug, Clone, Default)]
pub struct CheckpointUpdate {
    pub progress: Option<String>,
    pub next: FieldUpdate<String>,
    pub blocker: FieldUpdate<String>,
}

impl CheckpointUpdate {
    pub fn is_empty(&self) -> bool {
        self.progress.is_none()
            && matches!(self.next, FieldUpdate::Keep)
            && matches!(self.blocker, FieldUpdate::Keep)
    }
}

/// Readiness context for `claim`, derived by the Graph.
#[derive(Debug, Clone, Default)]
pub struct ClaimGate {
    pub hold_labels: Vec<String>,
    pub blocking: Vec<String>,
    pub open_subtasks: Vec<String>,
}

/// The transition named `name` when the workflow defines it and it starts
/// from `status`; otherwise the matching [`InvalidReason`].
fn require_transition<'w>(
    workflow: &'w Workflow,
    name: &str,
    status: &str,
    id: &str,
) -> Result<&'w TransitionDef, TransitionError> {
    let Some(transition) = workflow.transition_named(name) else {
        return Err(TransitionError::InvalidTransition {
            id: id.to_string(),
            reason: InvalidReason::UnknownTransition {
                name: name.to_string(),
                defined: workflow
                    .transition_names()
                    .iter()
                    .map(|n| n.to_string())
                    .collect(),
            },
        });
    };
    if !transition.allows_from(status) {
        return Err(TransitionError::InvalidTransition {
            id: id.to_string(),
            reason: InvalidReason::NotAllowed {
                name: name.to_string(),
                status: status.to_string(),
                valid: workflow
                    .transitions_from(status)
                    .iter()
                    .map(|t| t.name.clone())
                    .collect(),
            },
        });
    }
    Ok(transition)
}

impl Task {
    pub fn holder(&self) -> Option<&str> {
        self.front.claim.as_ref().map(|claim| claim.holder.as_str())
    }

    pub fn claimed_at(&self) -> Option<DateTime<Utc>> {
        self.front.claim.as_ref().and_then(|claim| claim.claimed_at)
    }

    pub fn lease_until(&self) -> Option<DateTime<Utc>> {
        self.front
            .claim
            .as_ref()
            .and_then(|claim| claim.lease_until)
    }

    pub fn is_stale(&self, now: DateTime<Utc>) -> bool {
        self.front
            .claim
            .as_ref()
            .is_some_and(|claim| claim.is_stale(now))
    }

    /// A live Claim held by someone else blocks the caller unless forced; a
    /// Stale one does not.
    pub fn authorize_live(
        &self,
        caller: Option<&str>,
        force: bool,
        now: DateTime<Utc>,
    ) -> Result<(), TransitionError> {
        match &self.front.claim {
            Some(claim) if claim.is_live(now) && !force => self.require_holder(claim, caller),
            _ => Ok(()),
        }
    }

    fn require_holder(&self, claim: &Claim, caller: Option<&str>) -> Result<(), TransitionError> {
        if caller == Some(claim.holder.as_str()) {
            return Ok(());
        }
        Err(TransitionError::NotClaimHolder {
            id: self.id.clone(),
            holder: claim.holder.clone(),
        })
    }

    fn invalid(&self, reason: InvalidReason) -> TransitionError {
        TransitionError::InvalidTransition {
            id: self.id.clone(),
            reason,
        }
    }

    fn clear_claim(&mut self) {
        self.front.claim = None;
        self.front.stray_claim_times = None;
    }

    /// Applies a workflow transition: sets the target status, records the
    /// resolution when the target is a closed category, clears the claim
    /// when leaving the active bucket or closing, and clears the resolution
    /// whenever the target is open. Links and external blockers survive
    /// every transition; only explicit link commands change them.
    fn apply_transition(
        &mut self,
        transition: &TransitionDef,
        workflow: &Workflow,
        now: DateTime<Utc>,
    ) -> Outcome {
        if transition.to == self.front.status {
            return Outcome::Unchanged;
        }
        let closed = workflow
            .category_of(&transition.to)
            .is_some_and(StatusCategory::is_terminal);
        // Leaving the active bucket hands the work back: `release` and any
        // other transition out of an active status drop the Claim.
        let was_active = workflow.category_of(&self.front.status) == Some(StatusCategory::Active);
        if closed {
            self.front.resolution = Some(
                transition
                    .resolution
                    .clone()
                    .unwrap_or_else(|| transition.name.clone()),
            );
            self.clear_claim();
        } else {
            self.front.resolution = None;
            if was_active {
                self.clear_claim();
            }
        }
        self.front.status = transition.to.clone();
        self.touch(now);
        Outcome::Changed
    }

    /// Release keeps links on open-to-open edges; only edges touching a
    /// closed status clear the resolution (see [`Self::apply_transition`]).
    /// Links and external blockers survive every transition; only explicit
    /// link commands change them. An unclaimed Task outside the `release`
    /// transition's sources is `Unchanged`, which preserves the historical
    /// "was not claimed" no-op.
    pub fn release(
        &mut self,
        caller: Option<&str>,
        force: bool,
        now: DateTime<Utc>,
        workflow: &Workflow,
    ) -> Result<Outcome, TransitionError> {
        let status = self.front.status.clone();
        let Some(transition) = workflow.transition_named(TRANSITION_RELEASE) else {
            return Err(self.invalid(InvalidReason::UnknownTransition {
                name: TRANSITION_RELEASE.to_string(),
                defined: workflow
                    .transition_names()
                    .iter()
                    .map(|name| name.to_string())
                    .collect(),
            }));
        };
        if !transition.allows_from(&status) {
            if self.front.claim.is_none() && !workflow.is_terminal(&status) {
                return Ok(Outcome::Unchanged);
            }
            return Err(self.invalid(InvalidReason::NotAllowed {
                name: TRANSITION_RELEASE.to_string(),
                status,
                valid: workflow
                    .transitions_from(&self.front.status)
                    .iter()
                    .map(|t| t.name.clone())
                    .collect(),
            }));
        }
        if let Some(claim) = &self.front.claim
            && !force
        {
            self.require_holder(claim, caller)?;
        }
        if self.front.claim.is_none() && self.front.status == transition.to {
            return Ok(Outcome::Unchanged);
        }
        Ok(self.apply_transition(transition, workflow, now))
    }

    /// Extends the lease and returns the new expiry. Requires an active-category
    /// status — the workflow's "being worked" bucket.
    pub fn heartbeat(
        &mut self,
        agent: &str,
        lease: Duration,
        now: DateTime<Utc>,
        workflow: &Workflow,
    ) -> Result<DateTime<Utc>, TransitionError> {
        if workflow.category_of(&self.front.status) != Some(StatusCategory::Active) {
            return Err(self.invalid(InvalidReason::HeartbeatNotActive(self.front.status.clone())));
        }
        let Some(claim) = &self.front.claim else {
            return Err(self.invalid(InvalidReason::HeartbeatNoClaim));
        };
        self.require_holder(claim, Some(agent))?;
        let lease_until = now + lease;
        self.front
            .claim
            .as_mut()
            .expect("checked above")
            .lease_until = Some(lease_until);
        self.touch(now);
        Ok(lease_until)
    }

    /// Replaces the active work checkpoint. The caller must hold the claim; a
    /// stale lease still counts so the holder can hand off after a long run.
    pub fn record_checkpoint(
        &mut self,
        agent: &str,
        update: CheckpointUpdate,
        now: DateTime<Utc>,
        workflow: &Workflow,
    ) -> Result<Outcome, TransitionError> {
        if workflow.is_terminal(&self.front.status) {
            return Err(self.invalid(InvalidReason::CheckpointClosed(self.front.status.clone())));
        }
        let Some(claim) = &self.front.claim else {
            return Err(self.invalid(InvalidReason::CheckpointNoClaim));
        };
        self.require_holder(claim, Some(agent))?;
        if update.is_empty() {
            return Err(TransitionError::InvalidCheckpoint(
                "nothing to record in the checkpoint".to_string(),
            ));
        }

        let existing = self.front.checkpoint.clone();
        let progress = update
            .progress
            .or_else(|| {
                existing
                    .as_ref()
                    .map(|checkpoint| checkpoint.progress.clone())
            })
            .ok_or_else(|| {
                TransitionError::InvalidCheckpoint(
                    "--progress is required to record the first checkpoint".to_string(),
                )
            })?;
        let next = match update.next {
            FieldUpdate::Keep => existing
                .as_ref()
                .and_then(|checkpoint| checkpoint.next.clone()),
            FieldUpdate::Clear => None,
            FieldUpdate::Set(value) => Some(value.trim().to_string()),
        };
        let blocker = match update.blocker {
            FieldUpdate::Keep => existing
                .as_ref()
                .and_then(|checkpoint| checkpoint.blocker.clone()),
            FieldUpdate::Clear => None,
            FieldUpdate::Set(value) => Some(value.trim().to_string()),
        };

        let checkpoint = Checkpoint {
            by: agent.to_string(),
            at: now,
            progress: progress.trim().to_string(),
            next,
            blocker,
        };
        if let Err(reason) = checkpoint.validate() {
            return Err(TransitionError::InvalidCheckpoint(reason));
        }
        self.front.checkpoint = Some(checkpoint);
        self.touch(now);
        Ok(Outcome::Changed)
    }

    /// Readiness gate first (so `steal` never bypasses it), then the existing
    /// Claim. The `claim` transition decides the target status.
    pub fn claim(
        &mut self,
        agent: &str,
        steal: bool,
        lease: Duration,
        gate: &ClaimGate,
        now: DateTime<Utc>,
        workflow: &Workflow,
    ) -> Result<(), TransitionError> {
        let id = self.id.clone();
        if workflow.is_terminal(&self.front.status) {
            return Err(self.invalid(InvalidReason::ClaimClosed(self.front.status.clone())));
        }
        let transition = require_transition(workflow, TRANSITION_CLAIM, &self.front.status, &id)?;
        let target = transition.to.clone();
        let waiting_labels = self.waiting_labels();
        if !waiting_labels.is_empty() {
            return Err(TransitionError::WaitingExternal {
                id,
                waiting_on: Some(waiting_labels.join(", ")),
            });
        }
        if !gate.hold_labels.is_empty() {
            return Err(TransitionError::LabelBlocked {
                id,
                labels: gate.hold_labels.clone(),
            });
        }
        if !gate.blocking.is_empty() {
            return Err(TransitionError::DependencyBlocked {
                id,
                blocking: gate.blocking.clone(),
            });
        }
        if !gate.open_subtasks.is_empty() {
            return Err(TransitionError::SubTasksOpen {
                id,
                subtasks: gate.open_subtasks.clone(),
            });
        }
        // The current holder renews its own Claim, live or Stale; this makes a
        // repeated claim (for example a retried Batch) a safe no-op.
        if let Some(claim) = &mut self.front.claim
            && claim.holder == agent
        {
            claim.lease_until = Some(now + lease);
            if self.front.status != target {
                self.front.status = target;
            }
            self.touch(now);
            return Ok(());
        }
        if let Some(claim) = &self.front.claim
            && !steal
        {
            let (holder, lease_until) = (claim.holder.clone(), claim.lease_until);
            return Err(if claim.is_live(now) {
                TransitionError::AlreadyClaimed {
                    id,
                    holder,
                    lease_until,
                }
            } else {
                TransitionError::ClaimStale {
                    id,
                    holder,
                    lease_until,
                }
            });
        }
        self.front.status = target;
        self.front.stray_claim_times = None;
        self.front.claim = Some(Claim {
            holder: agent.to_string(),
            claimed_at: Some(now),
            lease_until: Some(now + lease),
        });
        self.touch(now);
        Ok(())
    }

    /// `Unchanged` means already done: the handler may still record a comment
    /// and `touch`. The `done` transition decides the target status and
    /// resolution.
    pub fn complete(
        &mut self,
        caller: Option<&str>,
        force: bool,
        blocking: &[String],
        open_subtasks: &[String],
        now: DateTime<Utc>,
        workflow: &Workflow,
    ) -> Result<Outcome, TransitionError> {
        match workflow.category_of(&self.front.status) {
            Some(StatusCategory::Done) => return Ok(Outcome::Unchanged),
            Some(StatusCategory::Canceled) => {
                return Err(
                    self.invalid(InvalidReason::CompleteCanceled(self.front.status.clone()))
                );
            }
            _ => {}
        }
        let id = self.id.clone();
        let transition = require_transition(workflow, TRANSITION_DONE, &self.front.status, &id)?;
        if !open_subtasks.is_empty() {
            return Err(TransitionError::SubTasksOpen {
                id,
                subtasks: open_subtasks.to_vec(),
            });
        }
        if !blocking.is_empty() {
            return Err(TransitionError::DependencyBlocked {
                id,
                blocking: blocking.to_vec(),
            });
        }
        self.authorize_live(caller, force, now)?;
        Ok(self.apply_transition(transition, workflow, now))
    }

    /// `Unchanged` means already cancelled: the handler may still record a
    /// comment and `touch`. The `cancel` transition decides the target status
    /// and resolution.
    pub fn cancel(
        &mut self,
        caller: Option<&str>,
        force: bool,
        now: DateTime<Utc>,
        workflow: &Workflow,
    ) -> Result<Outcome, TransitionError> {
        match workflow.category_of(&self.front.status) {
            Some(StatusCategory::Canceled) => return Ok(Outcome::Unchanged),
            Some(StatusCategory::Done) => {
                return Err(self.invalid(InvalidReason::CancelDone(self.front.status.clone())));
            }
            _ => {}
        }
        let id = self.id.clone();
        let transition = require_transition(workflow, TRANSITION_CANCEL, &self.front.status, &id)?;
        self.authorize_live(caller, force, now)?;
        Ok(self.apply_transition(transition, workflow, now))
    }

    pub fn reopen(
        &mut self,
        force: bool,
        done_dependents: &[String],
        now: DateTime<Utc>,
        workflow: &Workflow,
    ) -> Result<(), TransitionError> {
        if !workflow.is_terminal(&self.front.status) {
            return Err(self.invalid(InvalidReason::ReopenActive(self.front.status.clone())));
        }
        if !done_dependents.is_empty() && !force {
            return Err(self.invalid(InvalidReason::ReopenDoneDependents(
                done_dependents.to_vec(),
            )));
        }
        let id = self.id.clone();
        let transition = require_transition(workflow, TRANSITION_REOPEN, &self.front.status, &id)?;
        self.apply_transition(transition, workflow, now);
        Ok(())
    }

    /// Runs any named workflow transition the Board defines. Unlike the
    /// dedicated commands this applies no gate checks (blocking dependencies,
    /// open SubTasks); those stay with `done`/`cancel`. A live Claim held by
    /// someone else still requires `force`.
    pub fn transition_to(
        &mut self,
        name: &str,
        caller: Option<&str>,
        force: bool,
        now: DateTime<Utc>,
        workflow: &Workflow,
    ) -> Result<Outcome, TransitionError> {
        let id = self.id.clone();
        let transition = require_transition(workflow, name, &self.front.status, &id)?;
        self.authorize_live(caller, force, now)?;
        Ok(self.apply_transition(transition, workflow, now))
    }

    /// The workflow transitions allowed from the current status, for
    /// `allowed_transitions` output.
    pub fn allowed_transitions<'w>(&self, workflow: &'w Workflow) -> Vec<&'w TransitionDef> {
        workflow.transitions_from(&self.front.status)
    }
}

impl From<TransitionError> for OrbitError {
    fn from(error: TransitionError) -> Self {
        use TransitionError::*;
        match error {
            InvalidTransition { id, reason } => reason.into_error(&id),
            InvalidCheckpoint(message) => OrbitError::invalid_argument(message),
            NotClaimHolder { id, holder } => OrbitError::new(
                code::NOT_CLAIM_HOLDER,
                format!("{id} is claimed by {holder}"),
            )
            .with_details(json!({ "task": id, "holder": holder })),
            AlreadyClaimed {
                id,
                holder,
                lease_until,
            } => OrbitError::new(
                code::ALREADY_CLAIMED,
                format!("{id} is claimed by {holder}"),
            )
            .with_details(json!({ "task": id, "holder": holder, "lease_until": lease_until })),
            ClaimStale {
                id,
                holder,
                lease_until,
            } => OrbitError::new(
                code::CLAIM_STALE,
                format!("{id} has a stale claim by {holder}; use `orbit claim {id} --steal`"),
            )
            .with_details(json!({ "task": id, "holder": holder, "lease_until": lease_until })),
            DependencyBlocked { id, blocking } => OrbitError::new(
                code::DEPENDENCY_BLOCKED,
                format!("{id} has unfinished dependencies"),
            )
            .with_details(json!({ "task": id, "blocking": blocking })),
            SubTasksOpen { id, subtasks } => OrbitError::new(
                code::SUBTASKS_OPEN,
                format!("{id} has open SubTasks: {}", subtasks.join(", ")),
            )
            .with_details(json!({ "task": id, "subtasks": subtasks })),
            WaitingExternal { id, waiting_on } => OrbitError::new(
                code::WAITING_EXTERNAL,
                format!("{id} is waiting on an external blocker"),
            )
            .with_details(json!({ "task": id, "waiting_on": waiting_on })),
            LabelBlocked { id, labels } => OrbitError::new(
                code::LABEL_BLOCKED,
                format!("{id} has hold labels: {}", labels.join(", ")),
            )
            .with_details(json!({ "task": id, "labels": labels })),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;
    use std::collections::BTreeMap;

    use crate::task::TaskFrontmatter;
    use crate::workflow::Workflow;

    const ID: &str = "orb-a7b2c9";

    fn now() -> DateTime<Utc> {
        Utc.with_ymd_and_hms(2026, 10, 24, 12, 0, 0).unwrap()
    }

    #[derive(Clone, Copy, Debug)]
    enum Held {
        No,
        Live,
    }

    /// A task whose Claim (if any) is held by `alice`.
    fn task(status: &str, held: Held) -> Task {
        let lease_until = match held {
            Held::No => None,
            Held::Live => Some(now() + Duration::minutes(10)),
        };
        Task {
            id: ID.to_string(),
            front: TaskFrontmatter {
                title: "t".to_string(),
                status: status.to_string(),
                resolution: None,
                task_type: "task".to_string(),
                priority: "medium".to_string(),
                labels: Vec::new(),
                links: vec![crate::task::Link::waiting_on("vendor")],
                parent: None,
                checkpoint: None,
                component: None,
                verification: None,
                github: None,
                remote: None,
                repository: None,
                external_id: None,
                stray_claim_times: None,
                claim: lease_until.map(|lease_until| Claim {
                    holder: "alice".to_string(),
                    claimed_at: Some(now() - Duration::minutes(20)),
                    lease_until: Some(lease_until),
                }),
                created_at: now(),
                updated_at: now() - Duration::hours(1),
                source_key: None,
                properties: BTreeMap::new(),
                extra: BTreeMap::new(),
            },
            body: String::new(),
        }
    }

    fn workflow() -> Workflow {
        Workflow::standard()
    }

    fn gate() -> ClaimGate {
        ClaimGate::default()
    }

    #[test]
    fn claim_moves_ready_to_in_progress_and_sets_lease() {
        let mut t = task("ready", Held::No);
        t.front.links.clear();
        t.claim(
            "bob",
            false,
            Duration::minutes(30),
            &gate(),
            now(),
            &workflow(),
        )
        .unwrap();
        assert_eq!(t.front.status, "in_progress");
        assert_eq!(t.holder(), Some("bob"));
        assert_eq!(t.lease_until(), Some(now() + Duration::minutes(30)));
    }

    #[test]
    fn claim_rejects_closed_and_unready_statuses_with_valid_transitions() {
        let mut closed = task("done", Held::No);
        let error = closed
            .claim(
                "bob",
                false,
                Duration::minutes(30),
                &gate(),
                now(),
                &workflow(),
            )
            .unwrap_err();
        assert!(matches!(
            error,
            TransitionError::InvalidTransition {
                reason: InvalidReason::ClaimClosed(_),
                ..
            }
        ));

        // `todo` is a backlog status in the standard workflow: claim is not
        // allowed and the error names the valid transitions.
        let mut backlog = task("todo", Held::No);
        backlog.front.links.clear();
        let error = backlog
            .claim(
                "bob",
                false,
                Duration::minutes(30),
                &gate(),
                now(),
                &workflow(),
            )
            .unwrap_err();
        match &error {
            TransitionError::InvalidTransition {
                reason: InvalidReason::NotAllowed { name, valid, .. },
                ..
            } => {
                assert_eq!(name, "claim");
                assert!(valid.contains(&"promote".to_string()));
                assert!(valid.contains(&"done".to_string()));
                assert!(!valid.contains(&"claim".to_string()));
            }
            other => panic!("unexpected error: {other:?}"),
        }
        let orbit = OrbitError::from(error);
        assert!(orbit.message.contains("valid transitions"));
    }

    #[test]
    fn claim_gates_still_apply() {
        let mut waiting = task("ready", Held::No);
        // fixture has waiting_on set
        let error = waiting
            .claim(
                "bob",
                false,
                Duration::minutes(30),
                &gate(),
                now(),
                &workflow(),
            )
            .unwrap_err();
        assert!(matches!(error, TransitionError::WaitingExternal { .. }));

        let mut held = task("ready", Held::Live);
        held.front.links.clear();
        let error = held
            .claim(
                "bob",
                false,
                Duration::minutes(30),
                &gate(),
                now(),
                &workflow(),
            )
            .unwrap_err();
        assert!(matches!(error, TransitionError::AlreadyClaimed { .. }));

        let mut blocked = task("ready", Held::No);
        blocked.front.links.clear();
        let gate = ClaimGate {
            blocking: vec!["orb-dead".to_string()],
            ..ClaimGate::default()
        };
        let error = blocked
            .claim(
                "bob",
                false,
                Duration::minutes(30),
                &gate,
                now(),
                &workflow(),
            )
            .unwrap_err();
        assert!(matches!(error, TransitionError::DependencyBlocked { .. }));
    }

    #[test]
    fn release_returns_to_the_transition_target() {
        let mut t = task("in_progress", Held::Live);
        t.release(Some("alice"), false, now(), &workflow()).unwrap();
        assert_eq!(t.front.status, "ready");
        assert!(t.front.claim.is_none());
        // release keeps waiting_on on open-to-open edges
        assert_eq!(t.waiting_labels(), vec!["vendor".to_string()]);
    }

    #[test]
    fn release_on_unclaimed_open_task_is_unchanged() {
        let mut t = task("ready", Held::No);
        let outcome = t.release(None, false, now(), &workflow()).unwrap();
        assert_eq!(outcome, Outcome::Unchanged);

        // An unclaimed backlog task is also a no-op, not an error.
        let mut t = task("todo", Held::No);
        let outcome = t.release(None, false, now(), &workflow()).unwrap();
        assert_eq!(outcome, Outcome::Unchanged);
        assert_eq!(t.front.status, "todo");
    }

    #[test]
    fn release_on_closed_task_names_valid_transitions() {
        let mut t = task("done", Held::No);
        let error = t.release(None, false, now(), &workflow()).unwrap_err();
        match error {
            TransitionError::InvalidTransition {
                reason: InvalidReason::NotAllowed { name, valid, .. },
                ..
            } => {
                assert_eq!(name, "release");
                assert_eq!(valid, vec!["reopen".to_string()]);
            }
            other => panic!("unexpected error: {other:?}"),
        }
    }

    #[test]
    fn heartbeat_requires_active_category_and_claim() {
        let mut t = task("in_progress", Held::Live);
        let lease = t
            .heartbeat("alice", Duration::minutes(30), now(), &workflow())
            .unwrap();
        assert_eq!(lease, now() + Duration::minutes(30));

        let mut not_active = task("ready", Held::Live);
        let error = not_active
            .heartbeat("alice", Duration::minutes(30), now(), &workflow())
            .unwrap_err();
        assert!(matches!(
            error,
            TransitionError::InvalidTransition {
                reason: InvalidReason::HeartbeatNotActive(_),
                ..
            }
        ));

        let mut no_claim = task("in_progress", Held::No);
        let error = no_claim
            .heartbeat("alice", Duration::minutes(30), now(), &workflow())
            .unwrap_err();
        assert!(matches!(
            error,
            TransitionError::InvalidTransition {
                reason: InvalidReason::HeartbeatNoClaim,
                ..
            }
        ));
    }

    #[test]
    fn checkpoint_rejects_closed_statuses() {
        let mut t = task("done", Held::No);
        let update = CheckpointUpdate {
            progress: Some("x".to_string()),
            ..CheckpointUpdate::default()
        };
        let error = t
            .record_checkpoint("alice", update, now(), &workflow())
            .unwrap_err();
        assert!(matches!(
            error,
            TransitionError::InvalidTransition {
                reason: InvalidReason::CheckpointClosed(_),
                ..
            }
        ));
    }

    #[test]
    fn complete_records_resolution_and_clears_claim() {
        let mut t = task("in_progress", Held::Live);
        let outcome = t
            .complete(Some("alice"), false, &[], &[], now(), &workflow())
            .unwrap();
        assert_eq!(outcome, Outcome::Changed);
        assert_eq!(t.front.status, "done");
        assert_eq!(t.front.resolution.as_deref(), Some("completed"));
        assert!(t.front.claim.is_none());
        // Links and external blockers survive transitions.
        assert_eq!(t.waiting_labels(), vec!["vendor".to_string()]);
    }

    #[test]
    fn complete_twice_is_unchanged_and_cancelled_cannot_complete() {
        let mut done = task("done", Held::No);
        let outcome = done
            .complete(None, false, &[], &[], now(), &workflow())
            .unwrap();
        assert_eq!(outcome, Outcome::Unchanged);

        let mut cancelled = task("cancelled", Held::No);
        let error = cancelled
            .complete(None, false, &[], &[], now(), &workflow())
            .unwrap_err();
        assert!(matches!(
            error,
            TransitionError::InvalidTransition {
                reason: InvalidReason::CompleteCanceled(_),
                ..
            }
        ));
    }

    #[test]
    fn complete_checks_open_subtasks_and_blocking_deps() {
        let mut t = task("ready", Held::No);
        t.front.links.clear();
        let error = t
            .complete(
                None,
                false,
                &[],
                &["orb-kid".to_string()],
                now(),
                &workflow(),
            )
            .unwrap_err();
        assert!(matches!(error, TransitionError::SubTasksOpen { .. }));

        let error = t
            .complete(
                None,
                false,
                &["orb-dep".to_string()],
                &[],
                now(),
                &workflow(),
            )
            .unwrap_err();
        assert!(matches!(error, TransitionError::DependencyBlocked { .. }));
    }

    #[test]
    fn cancel_records_its_own_resolution() {
        let mut t = task("ready", Held::No);
        t.front.links.clear();
        t.cancel(None, false, now(), &workflow()).unwrap();
        assert_eq!(t.front.status, "cancelled");
        assert_eq!(t.front.resolution.as_deref(), Some("cancelled"));

        let mut done = task("done", Held::No);
        let error = done.cancel(None, false, now(), &workflow()).unwrap_err();
        assert!(matches!(
            error,
            TransitionError::InvalidTransition {
                reason: InvalidReason::CancelDone(_),
                ..
            }
        ));
    }

    #[test]
    fn reopen_returns_to_the_transition_target_and_clears_resolution() {
        let mut t = task("done", Held::No);
        t.front.resolution = Some("completed".to_string());
        t.reopen(false, &[], now(), &workflow()).unwrap();
        assert_eq!(t.front.status, "ready");
        assert!(t.front.resolution.is_none());
        // Links and external blockers survive reopen.
        assert_eq!(t.waiting_labels(), vec!["vendor".to_string()]);
    }

    #[test]
    fn reopen_rejects_open_statuses() {
        let mut t = task("ready", Held::No);
        let error = t.reopen(false, &[], now(), &workflow()).unwrap_err();
        assert!(matches!(
            error,
            TransitionError::InvalidTransition {
                reason: InvalidReason::ReopenActive(_),
                ..
            }
        ));
    }

    #[test]
    fn transition_to_runs_named_transitions_and_guards_claims() {
        let workflow = Workflow::standard();
        let mut t = task("ready", Held::No);
        t.front.links.clear();
        // `defer` is a standard-workflow transition ready -> todo.
        let outcome = t
            .transition_to("defer", None, false, now(), &workflow)
            .unwrap();
        assert_eq!(outcome, Outcome::Changed);
        assert_eq!(t.front.status, "todo");

        // `promote` is not allowed from `in_progress`; the error names the
        // transitions that are.
        let mut t = task("in_progress", Held::Live);
        t.front.links.clear();
        let error = t
            .transition_to("promote", Some("alice"), false, now(), &workflow)
            .unwrap_err();
        match error {
            TransitionError::InvalidTransition {
                reason: InvalidReason::NotAllowed { name, valid, .. },
                ..
            } => {
                assert_eq!(name, "promote");
                assert!(valid.contains(&"release".to_string()));
                assert!(valid.contains(&"done".to_string()));
            }
            other => panic!("unexpected error: {other:?}"),
        }

        // Unknown transition names list what the workflow defines.
        let error = t
            .transition_to("fly", None, false, now(), &workflow)
            .unwrap_err();
        match error {
            TransitionError::InvalidTransition {
                reason: InvalidReason::UnknownTransition { name, defined },
                ..
            } => {
                assert_eq!(name, "fly");
                assert!(defined.contains(&"claim".to_string()));
            }
            other => panic!("unexpected error: {other:?}"),
        }
    }

    #[test]
    fn transition_to_a_closed_status_records_resolution() {
        let workflow = Workflow::minimal();
        let mut t = task("in_progress", Held::Live);
        t.front.links.clear();
        // minimal has no `cancel` from in_progress? it does: [ready,in_progress]
        t.transition_to("cancel", Some("alice"), false, now(), &workflow)
            .unwrap();
        assert_eq!(t.front.status, "cancelled");
        assert_eq!(t.front.resolution.as_deref(), Some("cancelled"));
        assert!(t.front.claim.is_none());
    }

    #[test]
    fn invalid_transition_error_names_valid_ones() {
        let mut t = task("done", Held::No);
        let error = t
            .transition_to("claim", None, false, now(), &workflow())
            .unwrap_err();
        let orbit = OrbitError::from(error);
        assert_eq!(orbit.code, code::INVALID_TRANSITION);
        assert!(orbit.message.contains("reopen"));
    }
}

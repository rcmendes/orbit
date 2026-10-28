//! Task lifecycle rules. A Task owns its transitions: callers pass the actor and the
//! clock, and get back the changed Task or a typed [`TransitionError`]. No I/O happens
//! here. Context that lives in the Graph (blocking dependencies, hold labels, done
//! dependents) comes in as arguments.

use chrono::{DateTime, Duration, Utc};
use serde_json::json;

use crate::error::{OrbitError, code};
#[cfg(test)]
use crate::task::TaskType;
use crate::task::{Checkpoint, Claim, Status, Task};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Outcome {
    Changed,
    /// Already in the target state; nothing was modified.
    Unchanged,
}

/// Why a transition is not legal from the Task's current state (`INVALID_TRANSITION`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum InvalidReason {
    ReleaseTerminal(Status),
    HeartbeatNotInProgress(Status),
    HeartbeatNoClaim,
    CheckpointTerminal(Status),
    CheckpointNoClaim,
    ClaimTerminal(Status),
    CompleteCancelled,
    CancelDone,
    ReopenActive(Status),
    ReopenDoneDependents(Vec<String>),
}

impl InvalidReason {
    fn into_error(self, id: &str) -> OrbitError {
        let message = match &self {
            Self::ReleaseTerminal(status) => {
                format!(
                    "{id} is {}; release only applies to active tasks",
                    status.as_str()
                )
            }
            Self::HeartbeatNotInProgress(status) => {
                format!(
                    "{id} is {}; only in_progress tasks can be heartbeated",
                    status.as_str()
                )
            }
            Self::HeartbeatNoClaim => format!("{id} has no claim"),
            Self::CheckpointTerminal(status) => {
                format!(
                    "{id} is {}; checkpoints only apply to active tasks",
                    status.as_str()
                )
            }
            Self::CheckpointNoClaim => {
                format!("{id} has no claim; claim it before recording a checkpoint")
            }
            Self::ClaimTerminal(status) => {
                format!("{id} is {}; it cannot be claimed", status.as_str())
            }
            Self::CompleteCancelled => {
                format!("{id} is cancelled; reopen it before completing it")
            }
            Self::CancelDone => {
                format!("{id} is done; reopen it before cancelling it")
            }
            Self::ReopenActive(status) => {
                format!(
                    "{id} is {}; only terminal tasks can be reopened",
                    status.as_str()
                )
            }
            Self::ReopenDoneDependents(dependents) => {
                format!("{id} has done dependents: {}", dependents.join(", "))
            }
        };
        let error = OrbitError::new(code::INVALID_TRANSITION, message);
        match self {
            Self::ReopenDoneDependents(dependents) => {
                error.with_details(json!({ "done_dependents": dependents }))
            }
            _ => error,
        }
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

    pub fn touch(&mut self, now: DateTime<Utc>) {
        self.front.updated_at = now;
    }

    /// A live Claim held by someone else blocks the caller unless forced; a Stale one does not.
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

    /// Every changing transition that ends a Claim goes through here.
    fn settle(&mut self, status: Status, now: DateTime<Utc>) -> Outcome {
        self.front.status = status;
        self.clear_claim();
        self.touch(now);
        Outcome::Changed
    }

    /// Terminal transitions and reopen also clear `waiting_on`.
    fn finish(&mut self, status: Status, now: DateTime<Utc>) -> Outcome {
        self.front.waiting_on = None;
        self.settle(status, now)
    }

    /// Release keeps `waiting_on`; only terminal transitions clear it.
    pub fn release(
        &mut self,
        caller: Option<&str>,
        force: bool,
        now: DateTime<Utc>,
    ) -> Result<Outcome, TransitionError> {
        if self.front.status.is_terminal() {
            return Err(self.invalid(InvalidReason::ReleaseTerminal(self.front.status)));
        }
        if let Some(claim) = &self.front.claim
            && !force
        {
            self.require_holder(claim, caller)?;
        }
        if self.front.claim.is_none() && self.front.status != Status::InProgress {
            return Ok(Outcome::Unchanged);
        }
        Ok(self.settle(Status::Todo, now))
    }

    /// Extends the lease and returns the new expiry.
    pub fn heartbeat(
        &mut self,
        agent: &str,
        lease: Duration,
        now: DateTime<Utc>,
    ) -> Result<DateTime<Utc>, TransitionError> {
        if self.front.status != Status::InProgress {
            return Err(self.invalid(InvalidReason::HeartbeatNotInProgress(self.front.status)));
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
    ) -> Result<Outcome, TransitionError> {
        if self.front.status.is_terminal() {
            return Err(self.invalid(InvalidReason::CheckpointTerminal(self.front.status)));
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

    /// Readiness gate first (so `steal` never bypasses it), then the existing Claim.
    pub fn claim(
        &mut self,
        agent: &str,
        steal: bool,
        lease: Duration,
        gate: &ClaimGate,
        now: DateTime<Utc>,
    ) -> Result<(), TransitionError> {
        let id = self.id.clone();
        if self.front.status.is_terminal() {
            return Err(self.invalid(InvalidReason::ClaimTerminal(self.front.status)));
        }
        if self.front.waiting_on.is_some() {
            return Err(TransitionError::WaitingExternal {
                id,
                waiting_on: self.front.waiting_on.clone(),
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
        self.front.status = Status::InProgress;
        self.front.stray_claim_times = None;
        self.front.claim = Some(Claim {
            holder: agent.to_string(),
            claimed_at: Some(now),
            lease_until: Some(now + lease),
        });
        self.touch(now);
        Ok(())
    }

    /// `Unchanged` means already done: the handler may still record a comment and `touch`.
    pub fn complete(
        &mut self,
        caller: Option<&str>,
        force: bool,
        blocking: &[String],
        open_subtasks: &[String],
        now: DateTime<Utc>,
    ) -> Result<Outcome, TransitionError> {
        match self.front.status {
            Status::Done => return Ok(Outcome::Unchanged),
            Status::Cancelled => return Err(self.invalid(InvalidReason::CompleteCancelled)),
            _ => {}
        }
        if !open_subtasks.is_empty() {
            return Err(TransitionError::SubTasksOpen {
                id: self.id.clone(),
                subtasks: open_subtasks.to_vec(),
            });
        }
        if !blocking.is_empty() {
            return Err(TransitionError::DependencyBlocked {
                id: self.id.clone(),
                blocking: blocking.to_vec(),
            });
        }
        self.authorize_live(caller, force, now)?;
        Ok(self.finish(Status::Done, now))
    }

    /// `Unchanged` means already cancelled: the handler may still record a comment and `touch`.
    pub fn cancel(
        &mut self,
        caller: Option<&str>,
        force: bool,
        now: DateTime<Utc>,
    ) -> Result<Outcome, TransitionError> {
        match self.front.status {
            Status::Cancelled => return Ok(Outcome::Unchanged),
            Status::Done => return Err(self.invalid(InvalidReason::CancelDone)),
            _ => {}
        }
        self.authorize_live(caller, force, now)?;
        Ok(self.finish(Status::Cancelled, now))
    }

    pub fn reopen(
        &mut self,
        force: bool,
        done_dependents: &[String],
        now: DateTime<Utc>,
    ) -> Result<(), TransitionError> {
        if !self.front.status.is_terminal() {
            return Err(self.invalid(InvalidReason::ReopenActive(self.front.status)));
        }
        if !done_dependents.is_empty() && !force {
            return Err(self.invalid(InvalidReason::ReopenDoneDependents(
                done_dependents.to_vec(),
            )));
        }
        self.finish(Status::Todo, now);
        Ok(())
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

    const ID: &str = "orb-a7b2c9";

    fn now() -> DateTime<Utc> {
        Utc.with_ymd_and_hms(2026, 10, 24, 12, 0, 0).unwrap()
    }

    #[derive(Clone, Copy, Debug)]
    enum Held {
        No,
        Live,
        Stale,
    }

    /// A task whose Claim (if any) is held by `alice`.
    fn task(status: Status, held: Held) -> Task {
        let lease_until = match held {
            Held::No => None,
            Held::Live => Some(now() + Duration::minutes(10)),
            Held::Stale => Some(now() - Duration::minutes(10)),
        };
        Task {
            id: ID.to_string(),
            front: TaskFrontmatter {
                title: "t".to_string(),
                status,
                task_type: TaskType::Task,
                priority: 2,
                labels: Vec::new(),
                depends_on: Vec::new(),
                parent: None,
                waiting_on: Some("vendor".to_string()),
                checkpoint: None,
                area: None,
                verification: None,
                github: None,
                stray_claim_times: None,
                claim: lease_until.map(|lease_until| Claim {
                    holder: "alice".to_string(),
                    claimed_at: Some(now() - Duration::minutes(20)),
                    lease_until: Some(lease_until),
                }),
                created_at: now(),
                updated_at: now() - Duration::hours(1),
                extra: BTreeMap::new(),
            },
            body: String::new(),
        }
    }

    fn not_holder() -> TransitionError {
        TransitionError::NotClaimHolder {
            id: ID.to_string(),
            holder: "alice".to_string(),
        }
    }

    fn invalid(reason: InvalidReason) -> TransitionError {
        TransitionError::InvalidTransition {
            id: ID.to_string(),
            reason,
        }
    }

    type Row<'a> = (
        Status,
        Held,
        Option<&'a str>,
        bool,
        Result<Outcome, TransitionError>,
    );

    use Held::*;
    use Status::*;

    #[test]
    fn release_matrix() {
        // (status, claim, caller, force, expected)
        let rows: Vec<Row> = vec![
            (Todo, No, None, false, Ok(Outcome::Unchanged)),
            (Todo, No, Some("bob"), false, Ok(Outcome::Unchanged)),
            (InProgress, Live, Some("alice"), false, Ok(Outcome::Changed)),
            (
                InProgress,
                Stale,
                Some("alice"),
                false,
                Ok(Outcome::Changed),
            ),
            (InProgress, Live, Some("bob"), false, Err(not_holder())),
            (InProgress, Stale, Some("bob"), false, Err(not_holder())),
            (InProgress, Live, None, false, Err(not_holder())),
            (InProgress, Live, Some("bob"), true, Ok(Outcome::Changed)),
            (InProgress, Live, None, true, Ok(Outcome::Changed)),
            (InProgress, No, None, false, Ok(Outcome::Changed)),
            (
                Done,
                No,
                Some("alice"),
                false,
                Err(invalid(InvalidReason::ReleaseTerminal(Done))),
            ),
            (
                Cancelled,
                Live,
                Some("alice"),
                true,
                Err(invalid(InvalidReason::ReleaseTerminal(Cancelled))),
            ),
        ];
        for (status, held, caller, force, expected) in rows {
            let mut t = task(status, held);
            let before = t.front.updated_at;
            let got = t.release(caller, force, now());
            assert_eq!(
                got, expected,
                "{status:?} {held:?} {caller:?} force={force}"
            );
            match got {
                Ok(Outcome::Changed) => {
                    assert_eq!(t.front.status, Todo);
                    assert!(t.front.claim.is_none());
                    assert_eq!(t.front.updated_at, now());
                    assert_eq!(
                        t.front.waiting_on.as_deref(),
                        Some("vendor"),
                        "release keeps waiting_on"
                    );
                }
                _ => assert_eq!(t.front.updated_at, before, "untouched on no-op or error"),
            }
        }
    }

    #[test]
    fn heartbeat_matrix() {
        let lease = Duration::minutes(30);
        let rows: Vec<(Status, Held, &str, Result<(), TransitionError>)> = vec![
            (InProgress, Live, "alice", Ok(())),
            (InProgress, Stale, "alice", Ok(())),
            (InProgress, Live, "bob", Err(not_holder())),
            (
                InProgress,
                No,
                "alice",
                Err(invalid(InvalidReason::HeartbeatNoClaim)),
            ),
            (
                Todo,
                No,
                "alice",
                Err(invalid(InvalidReason::HeartbeatNotInProgress(Todo))),
            ),
            (
                Done,
                No,
                "alice",
                Err(invalid(InvalidReason::HeartbeatNotInProgress(Done))),
            ),
        ];
        for (status, held, agent, expected) in rows {
            let mut t = task(status, held);
            let got = t.heartbeat(agent, lease, now());
            assert_eq!(
                got.clone().map(|_| ()),
                expected,
                "{status:?} {held:?} {agent}"
            );
            if got.is_ok() {
                let claim = t.front.claim.unwrap();
                assert_eq!(claim.lease_until, Some(now() + lease));
                assert_eq!(claim.holder, "alice");
                assert_eq!(t.front.updated_at, now());
            }
        }
    }

    #[test]
    fn checkpoint_requires_an_active_claim_held_by_the_caller() {
        let mut t = task(InProgress, Live);
        let outcome = t
            .record_checkpoint(
                "alice",
                CheckpointUpdate {
                    progress: Some("Parser done".into()),
                    next: FieldUpdate::Set("Wire".into()),
                    blocker: FieldUpdate::Keep,
                },
                now(),
            )
            .unwrap();
        assert_eq!(outcome, Outcome::Changed);
        let checkpoint = t.front.checkpoint.clone().unwrap();
        assert_eq!(checkpoint.by, "alice");
        assert_eq!(checkpoint.progress, "Parser done");
        assert_eq!(checkpoint.next.as_deref(), Some("Wire"));
        assert_eq!(t.front.updated_at, now());

        t.record_checkpoint(
            "alice",
            CheckpointUpdate {
                progress: None,
                next: FieldUpdate::Keep,
                blocker: FieldUpdate::Set("Review".into()),
            },
            now(),
        )
        .unwrap();
        let checkpoint = t.front.checkpoint.clone().unwrap();
        assert_eq!(checkpoint.progress, "Parser done");
        assert_eq!(checkpoint.next.as_deref(), Some("Wire"));
        assert_eq!(checkpoint.blocker.as_deref(), Some("Review"));

        t.record_checkpoint(
            "alice",
            CheckpointUpdate {
                blocker: FieldUpdate::Clear,
                ..Default::default()
            },
            now(),
        )
        .unwrap();
        assert!(t.front.checkpoint.unwrap().blocker.is_none());

        let mut unclaimed = task(Todo, No);
        assert_eq!(
            unclaimed.record_checkpoint(
                "alice",
                CheckpointUpdate {
                    progress: Some("x".into()),
                    ..Default::default()
                },
                now()
            ),
            Err(invalid(InvalidReason::CheckpointNoClaim))
        );

        let mut other = task(InProgress, Live);
        assert_eq!(
            other.record_checkpoint(
                "bob",
                CheckpointUpdate {
                    progress: Some("x".into()),
                    ..Default::default()
                },
                now()
            ),
            Err(not_holder())
        );

        let mut missing_progress = task(InProgress, Live);
        assert_eq!(
            missing_progress.record_checkpoint(
                "alice",
                CheckpointUpdate {
                    next: FieldUpdate::Set("Later".into()),
                    ..Default::default()
                },
                now()
            ),
            Err(TransitionError::InvalidCheckpoint(
                "--progress is required to record the first checkpoint".to_string()
            ))
        );

        let mut done = task(Done, No);
        assert_eq!(
            done.record_checkpoint(
                "alice",
                CheckpointUpdate {
                    blocker: FieldUpdate::Clear,
                    ..Default::default()
                },
                now()
            ),
            Err(invalid(InvalidReason::CheckpointTerminal(Done)))
        );
    }

    #[test]
    fn claim_matrix() {
        let lease = Duration::minutes(30);
        let blocked = ClaimGate {
            blocking: vec!["orb-dep".into()],
            ..Default::default()
        };
        let held = ClaimGate {
            hold_labels: vec!["hold".into()],
            ..Default::default()
        };
        let subtasks_open = ClaimGate {
            open_subtasks: vec!["orb-child".into()],
            ..Default::default()
        };
        let open = ClaimGate::default();
        let lease_until = |h: Held| {
            task(InProgress, h)
                .front
                .claim
                .map(|c| c.lease_until.unwrap())
        };
        let id = || ID.to_string();
        let holder = || "alice".to_string();

        let mut t = task(Todo, No);
        t.front.waiting_on = None;
        assert_eq!(t.claim("bob", false, lease, &open, now()), Ok(()));
        let claim = t.front.claim.clone().unwrap();
        assert_eq!((t.front.status, claim.holder.as_str()), (InProgress, "bob"));
        assert_eq!(claim.claimed_at, Some(now()));
        assert_eq!(claim.lease_until, Some(now() + lease));

        let mut t = task(Todo, No); // still waiting_on vendor
        assert_eq!(
            t.claim("bob", true, lease, &open, now()),
            Err(TransitionError::WaitingExternal {
                id: id(),
                waiting_on: Some("vendor".into())
            })
        );

        for (gate, expected) in [
            (
                &held,
                TransitionError::LabelBlocked {
                    id: id(),
                    labels: vec!["hold".into()],
                },
            ),
            (
                &blocked,
                TransitionError::DependencyBlocked {
                    id: id(),
                    blocking: vec!["orb-dep".into()],
                },
            ),
            (
                &subtasks_open,
                TransitionError::SubTasksOpen {
                    id: id(),
                    subtasks: vec!["orb-child".into()],
                },
            ),
        ] {
            let mut t = task(InProgress, Stale);
            t.front.waiting_on = None;
            // steal must not bypass the readiness gate
            assert_eq!(t.claim("bob", true, lease, gate, now()), Err(expected));
            assert_eq!(t.holder(), Some("alice"));
        }

        let mut t = task(Done, No);
        assert_eq!(
            t.claim("bob", false, lease, &open, now()),
            Err(invalid(InvalidReason::ClaimTerminal(Done)))
        );

        let mut t = task(InProgress, Live);
        t.front.waiting_on = None;
        assert_eq!(
            t.claim("bob", false, lease, &open, now()),
            Err(TransitionError::AlreadyClaimed {
                id: id(),
                holder: holder(),
                lease_until: lease_until(Live)
            })
        );

        let mut t = task(InProgress, Stale);
        t.front.waiting_on = None;
        assert_eq!(
            t.claim("bob", false, lease, &open, now()),
            Err(TransitionError::ClaimStale {
                id: id(),
                holder: holder(),
                lease_until: lease_until(Stale)
            })
        );
        assert_eq!(t.claim("bob", true, lease, &open, now()), Ok(()));
        let claim = t.front.claim.unwrap();
        assert_eq!(claim.holder, "bob");
        assert_eq!(claim.claimed_at, Some(now()), "steal resets claimed_at");
        assert_eq!(claim.lease_until, Some(now() + lease));
    }

    #[test]
    fn complete_and_cancel_matrix() {
        let rows: Vec<Row> = vec![
            (InProgress, Live, Some("alice"), false, Ok(Outcome::Changed)),
            (InProgress, Live, Some("bob"), false, Err(not_holder())),
            (InProgress, Live, None, false, Err(not_holder())),
            (InProgress, Live, Some("bob"), true, Ok(Outcome::Changed)),
            // a Stale Claim never blocks, for either transition
            (InProgress, Stale, Some("bob"), false, Ok(Outcome::Changed)),
            (InProgress, Stale, None, false, Ok(Outcome::Changed)),
            (Todo, No, None, false, Ok(Outcome::Changed)),
        ];
        for (status, held, caller, force, expected) in rows {
            let mut t = task(status, held);
            assert_eq!(
                t.complete(caller, force, &[], &[], now()),
                expected,
                "done {status:?} {held:?} {caller:?}"
            );
            if expected == Ok(Outcome::Changed) {
                assert_eq!(t.front.status, Done);
                assert!(t.front.claim.is_none());
                assert!(t.front.waiting_on.is_none(), "done clears waiting_on");
                assert_eq!(t.front.updated_at, now());
            }

            let mut t = task(status, held);
            assert_eq!(
                t.cancel(caller, force, now()),
                expected,
                "cancel {status:?} {held:?} {caller:?}"
            );
            if expected == Ok(Outcome::Changed) {
                assert_eq!(t.front.status, Cancelled);
                assert!(t.front.claim.is_none());
                assert!(t.front.waiting_on.is_none(), "cancel clears waiting_on");
            }
        }

        // already in the target state: no-op, untouched
        let mut t = task(Done, No);
        let before = t.front.updated_at;
        assert_eq!(
            t.complete(None, false, &[], &[], now()),
            Ok(Outcome::Unchanged)
        );
        assert_eq!(t.front.updated_at, before);
        let mut t = task(Cancelled, No);
        assert_eq!(t.cancel(None, false, now()), Ok(Outcome::Unchanged));

        // the other terminal state is rejected
        assert_eq!(
            task(Cancelled, No).complete(None, false, &[], &[], now()),
            Err(invalid(InvalidReason::CompleteCancelled))
        );
        assert_eq!(
            task(Done, No).cancel(None, false, now()),
            Err(invalid(InvalidReason::CancelDone))
        );

        // unfinished dependencies block done only, even when forced
        assert_eq!(
            task(InProgress, Live).complete(Some("alice"), true, &["orb-dep".into()], &[], now()),
            Err(TransitionError::DependencyBlocked {
                id: ID.into(),
                blocking: vec!["orb-dep".into()]
            })
        );

        // open SubTasks also block done, even when forced
        assert_eq!(
            task(InProgress, Live).complete(Some("alice"), true, &[], &["orb-child".into()], now()),
            Err(TransitionError::SubTasksOpen {
                id: ID.into(),
                subtasks: vec!["orb-child".into()]
            })
        );
    }

    #[test]
    fn reopen_matrix() {
        let dependents = vec!["orb-down".to_string()];
        for status in [Done, Cancelled] {
            let mut t = task(status, No);
            assert_eq!(t.reopen(false, &[], now()), Ok(()));
            assert_eq!(t.front.status, Todo);
            assert!(t.front.claim.is_none());
            assert!(t.front.waiting_on.is_none(), "reopen clears waiting_on");
            assert_eq!(t.front.updated_at, now());

            let mut t = task(status, No);
            assert_eq!(
                t.reopen(false, &dependents, now()),
                Err(invalid(InvalidReason::ReopenDoneDependents(
                    dependents.clone()
                )))
            );
            assert_eq!(t.reopen(true, &dependents, now()), Ok(()));
        }
        for status in [Todo, InProgress] {
            assert_eq!(
                task(status, No).reopen(true, &[], now()),
                Err(invalid(InvalidReason::ReopenActive(status)))
            );
        }
    }

    #[test]
    fn errors_keep_codes_and_details() {
        let error = OrbitError::from(invalid(InvalidReason::ReopenDoneDependents(vec![
            "a".into(),
            "b".into(),
        ])));
        assert_eq!(error.code, code::INVALID_TRANSITION);
        assert_eq!(error.message, format!("{ID} has done dependents: a, b"));
        assert_eq!(
            error.details,
            Some(json!({ "done_dependents": ["a", "b"] }))
        );

        let error = OrbitError::from(not_holder());
        assert_eq!(error.code, code::NOT_CLAIM_HOLDER);
        assert_eq!(
            error.details,
            Some(json!({ "task": ID, "holder": "alice" }))
        );
    }

    #[test]
    fn transitions_keep_unknown_fields_and_clear_stray_times() {
        let extra = serde_yaml_ng::Value::String("keep me".into());
        for run in [
            |t: &mut Task| t.release(None, true, now()).map(|_| ()),
            |t: &mut Task| t.cancel(None, true, now()).map(|_| ()),
            |t: &mut Task| t.complete(None, true, &[], &[], now()).map(|_| ()),
        ] {
            let mut t = task(InProgress, Live);
            t.front.extra.insert("owner_note".into(), extra.clone());
            run(&mut t).unwrap();
            assert_eq!(t.front.extra.get("owner_note"), Some(&extra));
        }

        let mut t = task(Todo, No);
        t.front.stray_claim_times = Some(crate::task::StrayClaimTimes {
            claimed_at: None,
            lease_until: Some(now()),
        });
        t.cancel(None, false, now()).unwrap();
        assert!(t.front.stray_claim_times.is_none());
    }
}

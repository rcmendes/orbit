//! The Remote Tracker port (ADR 0018).
//!
//! Core owns the sync engine once — baselines, three-way conflict
//! detection, previews, Auto-push, and the Remote Issue catalog. An
//! adapter only translates Remote Issues into Orbit field terms and
//! declares which Orbit fields it maps.

use std::sync::Arc;

use crate::board::Board;
use crate::error::{OrbitError, Result};
use crate::github::client::GitHubClient;
use crate::remote::RemoteProject;
use crate::remote::model::*;
use crate::task::Status;

/// The provider-neutral seam every Remote Tracker adapter implements.
pub trait RemoteAdapter {
    /// Tracker id this adapter serves (`github` today).
    fn tracker(&self) -> &str;

    /// The Mapped fields this adapter publishes for `project`. Fields it
    /// does not map stay local-only: never pushed, never pulled.
    fn mapped_fields(&self, project: &RemoteProject) -> MappedFields;

    /// Whether `project` may receive new Remote Issues on this Board.
    fn project_available(&self, board: &Board, project: &str) -> Result<bool>;

    /// Fetch one Remote Issue in Orbit terms.
    fn fetch_issue(&self, project: &RemoteProject, id: &str) -> Result<RemoteIssue>;

    /// Fetch many Remote Issues in one pass. Returns one entry per
    /// requested ref, in request order; a per-entry `Err` isolates the
    /// failure to that ref. The default issues one `fetch_issue` call per
    /// ref; adapters with a batch endpoint override it.
    fn fetch_issues(&self, refs: &[RemoteIssueRef]) -> Vec<Result<RemoteIssue>> {
        refs.iter()
            .map(|issue| {
                self.fetch_issue(
                    &RemoteProject {
                        tracker: self.tracker().to_string(),
                        project: issue.project.clone(),
                    },
                    &issue.id,
                )
            })
            .collect()
    }

    /// List a Remote Project's issues for the Remote Issue catalog.
    fn list_issues(&self, project: &RemoteProject) -> Result<Vec<RemoteIssueSummary>>;

    /// Create a Remote Issue from Orbit fields.
    fn create_issue(
        &self,
        project: &RemoteProject,
        fields: RemoteIssueFields,
    ) -> Result<RemoteIssue>;

    /// Apply a provider-neutral update to a Remote Issue. `current` is the
    /// snapshot the plan was computed against.
    fn update_issue(
        &self,
        project: &RemoteProject,
        id: &str,
        update: &RemoteIssueUpdate,
        current: &RemoteIssue,
    ) -> Result<RemoteIssue>;

    /// Whether this adapter syncs parent/dependency relationships.
    fn supports_relationships(&self) -> bool {
        false
    }

    /// Reconcile remote relationship edges for `id`. `desired` edges are
    /// ensured present; `removals` are dropped when present. Edges the
    /// caller does not mention are preserved.
    fn update_relationships(
        &self,
        project: &RemoteProject,
        id: &str,
        desired: &[DesiredRelation],
        removals: &[RelationChange],
    ) -> Result<()>;

    /// The remote status the adapter reports for an Orbit `status` push.
    /// Trackers that cannot express every Orbit status fold them (GitHub
    /// reports `in_progress` as its open state).
    fn remote_status_for(&self, status: Status) -> Status {
        status
    }

    /// Whether `issue`'s adapter-managed status markers agree with Orbit
    /// `status` (GitHub: the reserved in-progress label matches
    /// `in_progress`). Marker drift never pushes on its own.
    fn status_markers_agree(&self, issue: &RemoteIssue, status: Status) -> bool {
        let _ = (issue, status);
        true
    }
}

/// Resolves the [`RemoteAdapter`] for a Remote Tracker name.
pub trait RemoteResolver {
    /// Tracker ids with an installed adapter.
    fn trackers(&self) -> Vec<String> {
        Vec::new()
    }

    /// Build the adapter serving `tracker`.
    fn adapter(&self, tracker: &str) -> Result<Box<dyn RemoteAdapter>>;
}

/// Closure resolvers serve explicitly configured trackers only; they report
/// no `trackers()` so no implicit projects are discovered through them.
impl<F> RemoteResolver for F
where
    F: Fn(&str) -> Result<Box<dyn RemoteAdapter>>,
{
    fn adapter(&self, tracker: &str) -> Result<Box<dyn RemoteAdapter>> {
        self(tracker)
    }
}

/// The production resolver: built-in adapters configured from the Board.
///
/// The command edge may share one `GitHubClient` across every adapter in
/// the operation via [`BoardResolver::with_client`]; [`BoardResolver::new`]
/// adapters resolve their client from the environment when the adapter is
/// constructed (still at a command edge), so config-only port facts never
/// call `gh` or read the fixture.
pub struct BoardResolver<'a> {
    pub board: &'a Board,
    client: Option<Arc<dyn GitHubClient>>,
}

impl RemoteResolver for BoardResolver<'_> {
    fn trackers(&self) -> Vec<String> {
        vec!["github".to_string()]
    }

    fn adapter(&self, tracker: &str) -> Result<Box<dyn RemoteAdapter>> {
        match tracker {
            "github" => {
                // Without an injected client the adapter resolves the client
                // from the environment at construction; config-only port
                // facts (`mapped_fields`, `project_available`, …) never call
                // it — the first call that reaches GitHub does.
                let config = crate::github::adapter::GitHubAdapter::effective_config(self.board);
                Ok(Box::new(match &self.client {
                    Some(client) => {
                        crate::github::adapter::GitHubAdapter::with_client(client.clone(), config)
                    }
                    None => crate::github::adapter::GitHubAdapter::from_config(config),
                }))
            }
            other => Err(OrbitError::config(format!(
                "no Remote Tracker adapter is installed for `{other}`"
            ))),
        }
    }
}

impl<'a> BoardResolver<'a> {
    /// The resolver the engine uses when the caller does not inject one.
    /// Adapters resolve their client from the environment at construction.
    pub fn new(board: &'a Board) -> Self {
        Self {
            board,
            client: None,
        }
    }

    /// The resolver for a command edge that already built the operation's
    /// shared `GitHubClient`.
    pub fn with_client(board: &'a Board, client: Arc<dyn GitHubClient>) -> Self {
        Self {
            board,
            client: Some(client),
        }
    }
}

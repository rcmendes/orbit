//! Provider-neutral access to a Task's Remote Issue link (ADR 0018).
//!
//! Tasks store links under `remote:`; the legacy `github:` key is still
//! read everywhere through [`link_view`] and rewritten to `remote:` by
//! [`materialize`] the next time the sync engine persists the Task.

use serde::Serialize;

use crate::github::link::{GitHubLink, GitHubSyncBaseline};
use crate::remote::model::SyncBaseline;
use crate::remote::{RemoteLink, RemoteProject};
use crate::task::Task;

/// A Task's Remote Issue link in provider-neutral form, with the sync
/// baseline already decoded. Owned so callers can hold it across mutations.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct RemoteLinkView {
    pub tracker: String,
    /// Remote Project id in tracker terms (`owner/repo` for GitHub).
    pub project: String,
    /// Remote Issue id in tracker terms.
    pub id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub url: Option<String>,
    pub active: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub baseline: Option<SyncBaseline>,
}

impl RemoteLinkView {
    pub fn remote_project(&self) -> RemoteProject {
        RemoteProject {
            tracker: self.tracker.clone(),
            project: self.project.clone(),
        }
    }

    /// `owner/repo#42`-style identity for messages and catalogs.
    pub fn identity(&self) -> String {
        format!("{}#{}", self.project, self.id)
    }
}

/// The Task's Remote Issue link regardless of `active`, preferring the
/// native `remote:` key and falling back to the legacy `github:` key.
pub fn link_view(task: &Task) -> Option<RemoteLinkView> {
    if let Some(link) = &task.front.remote {
        return Some(remote_view(link));
    }
    task.front.github.as_ref().map(github_view)
}

/// The Task's active Remote Issue link; detached provenance returns `None`.
pub fn active_link(task: &Task) -> Option<RemoteLinkView> {
    link_view(task).filter(|link| link.active)
}

/// JSON form of the effective link for `show`/`list` output, or `Null`.
pub fn link_json(task: &Task) -> serde_json::Value {
    match link_view(task) {
        Some(link) => serde_json::to_value(link).unwrap_or(serde_json::Value::Null),
        None => serde_json::Value::Null,
    }
}

fn remote_view(link: &RemoteLink) -> RemoteLinkView {
    RemoteLinkView {
        tracker: link.tracker.clone(),
        project: link.project.clone(),
        id: link.id.clone(),
        url: link.url.clone(),
        active: link.active,
        baseline: link
            .baseline
            .as_ref()
            .and_then(|value| decode_baseline(&link.tracker, value)),
    }
}

fn github_view(link: &GitHubLink) -> RemoteLinkView {
    let remote = link.to_remote_link();
    RemoteLinkView {
        tracker: remote.tracker,
        project: remote.project,
        id: remote.id,
        url: remote.url,
        active: remote.active,
        baseline: link
            .last_sync
            .as_ref()
            .map(GitHubSyncBaseline::to_sync_baseline),
    }
}

/// Decode the opaque `remote.baseline` payload. Current links store a
/// [`SyncBaseline`]; links migrated by `orbit migrate` still carry the
/// GitHub-shaped baseline, which the GitHub bridge converts. Anything
/// unrecognised reads as "no baseline" so the next sync re-baselines
/// conservatively.
fn decode_baseline(tracker: &str, value: &serde_yaml_ng::Value) -> Option<SyncBaseline> {
    if let Ok(baseline) = serde_yaml_ng::from_value::<SyncBaseline>(value.clone()) {
        return Some(baseline);
    }
    if tracker == "github"
        && let Ok(legacy) = serde_yaml_ng::from_value::<GitHubSyncBaseline>(value.clone())
    {
        return Some(legacy.to_sync_baseline());
    }
    None
}

/// Rewrite a legacy `github:` link as `remote:` in place. No-op when the
/// Task already links through `remote:` or has no link at all.
pub fn materialize(task: &mut Task) {
    if task.front.remote.is_some() {
        return;
    }
    if let Some(github) = task.front.github.take() {
        task.front.remote = Some(github.to_remote_link());
    }
}

/// The Task's Remote Issue link as a mutable [`RemoteLink`], materializing
/// a legacy `github:` link first. Returns `None` for Local Tasks.
pub fn link_mut(task: &mut Task) -> Option<&mut RemoteLink> {
    materialize(task);
    task.front.remote.as_mut()
}

/// Encode a [`SyncBaseline`] into the opaque `remote.baseline` slot.
pub fn encode_baseline(baseline: &SyncBaseline) -> Option<serde_yaml_ng::Value> {
    serde_yaml_ng::to_value(baseline).ok()
}

/// GitHub-shaped JSON view of the Task's Remote Issue link, kept for output
/// compatibility (ADR 0018): `github:`-tracker links — legacy `github:`
/// frontmatter or `remote:` links on the github tracker — render in the
/// historical shape; other trackers and Local Tasks render `Null`.
pub fn github_compat_json(task: &Task) -> serde_json::Value {
    if let Some(github) = &task.front.github {
        return serde_json::to_value(github).unwrap_or(serde_json::Value::Null);
    }
    let Some(link) = &task.front.remote else {
        return serde_json::Value::Null;
    };
    if link.tracker != "github" {
        return serde_json::Value::Null;
    }
    let mut value = serde_json::json!({
        "repo": link.project,
        "number": link.id.parse::<u64>().unwrap_or(0),
    });
    if !link.active {
        value["active"] = serde_json::json!(false);
    }
    if let Some(baseline) = &link.baseline
        && let Some(baseline) = decode_baseline(&link.tracker, baseline)
        && let Ok(baseline) = serde_json::to_value(
            crate::github::link::GitHubSyncBaseline::from_sync_baseline(&baseline),
        )
    {
        value["last_sync"] = baseline;
    }
    value
}

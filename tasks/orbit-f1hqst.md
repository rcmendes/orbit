---
title: Workspace Issue Catalog and Local-First GitHub Orchestration
status: done
type: epic
priority: 2
labels:
- enhancement
created_at: 2026-10-28T22:46:56.870931Z
updated_at: 2026-10-29T11:06:08.661173Z
---

## Goal

Make a folder of GitHub repositories one Orbit Workspace. Orbit provides a local, rebuildable catalog of their Issues while keeping GitHub Issues as Remote Issues until a user explicitly imports them into the claimable Task workflow.

## Workspace layout and discovery

- The user-facing Workspace is a normal folder, not a Git repository.
- The Workspace Board lives internally at `<workspace>/.orbit/board`; the rebuildable Remote Issue catalog lives separately at `<workspace>/.orbit/cache`.
- Commands use the Workspace root from its root or from any child repository, including across child Git boundaries.
- Discover direct child Git repositories with GitHub `origin` remotes. A full-mode Workspace is limited to one GitHub organization, with explicit include/exclude controls.
- New repositories are detected during explicit Workspace refresh/reconcile. No background polling or webhooks.
- Existing Boards can be migrated with preview, preserving Tasks, comments, IDs, dependencies, parentage, and history. ID collisions are remapped; duplicate GitHub Issue identities require a user decision; Workspace settings win; original Boards remain as backups.

## Remote Issue catalog

- Cache all GitHub Issues (open and closed), not PRs.
- Store summary data locally in a persistent, rebuildable cache; fetch body/comments on demand.
- Show freshness and per-repository errors. A stale/unavailable repository retains its last cache until explicitly detached.
- Remote Issues are catalog entries, not Tasks: they are never claimable and never appear in `orbit ready` unless imported.

## Task and Issue lifecycle

- **Local Task:** created and managed only in Orbit.
- **Remote Issue:** catalog-only; GitHub owns its lifecycle.
- **Linked Task:** one Orbit Task paired with one GitHub Issue using stable `owner/repo#number` identity. Orbit is the source of truth for editable fields and lifecycle; sync is explicit.
- Import creates a Linked Task from a Remote Issue or links an existing Local Task. Closed Issues import as terminal Tasks. Duplicate Issue links are rejected or sent for explicit resolution.
- Export creates a GitHub Issue from a Local Task and links only after GitHub confirms creation. Active Tasks export by default; terminal Tasks require explicit selection.
- Import and Export support single records and reviewed batches. They never process the entire catalog by default.
- Conflicting edits on both sides produce field-level conflicts. The user chooses whether to pull the GitHub value or keep the Orbit value and push it; sync never silently overwrites.
- Detaching a Workspace repository or individual Linked Task is explicit and previewed. Linked Tasks become Local Tasks with inactive GitHub provenance; Remote Issue snapshots remain archived and non-claimable; detach never changes GitHub. Reattach matches by stable identity and requires preview/confirmation.

## Agent integration and diagnostics

- Workspace setup supports Claude Code, Codex, Cursor, and OpenCode from child repositories, using managed native instructions/configuration that preserve user-authored files.
- Matt Pocock skills remain a separate, automatically detected Skills/Plugins compatibility integration, not an agent harness or separate manual bridge installation.
- A Workspace reconcile/setup operation applies integrations to newly discovered repositories.
- Workspace-aware `orbit doctor` diagnoses discovery/access issues, stale caches, duplicate links, migration/sync conflicts, and agent setup drift. It suggests safe next steps but does not guess how to merge, unlink, close, or delete work.

## Non-goals

- Pull-request cataloging.
- Multiple GitHub organizations in one full-mode Workspace.
- Background refresh or webhook-driven synchronization.
- Automatically importing every Remote Issue into the Task graph.
- Automatically resolving data conflicts or detaching remote work.

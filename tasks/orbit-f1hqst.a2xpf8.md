---
title: Build and browse the Remote Issue catalog
status: todo
type: task
priority: 1
labels:
- enhancement
- ready-for-agent
depends_on:
- orbit-f1hqst.r02zj4
parent: orbit-f1hqst
area: board
created_at: 2026-10-28T22:47:50.427118Z
updated_at: 2026-10-28T22:47:50.427118Z
---

## Goal

Give Workspace users a local, rebuildable catalog of GitHub Issues across discovered repositories without inserting every Issue into Orbit's claimable Task graph.

## Acceptance criteria

- Explicit refresh paginates and caches all Issues (open and closed) for Workspace repositories; pull requests are excluded.
- Store stable identity (`owner/repo#number`), title, state, labels, Issue Fields, URL, and timestamps in a persistent cache under the Workspace's `.orbit/cache`, separate from Board Task files and Board history.
- Fetch body/comments when an Issue is opened or imported, not for every catalog row.
- Provide CLI and MCP operations to refresh, list/filter, and inspect Remote Issues by repo, state, labels, text, and linked/unlinked state.
- Show per-repository last refresh, errors, and stale status. A failed refresh keeps the previous snapshot and never makes it claimable.
- Refresh is explicit; it performs no GitHub writes and does not create Orbit Tasks.
- Remote Issue catalog entries are excluded from `orbit list`, `ready`, claims, and Task export unless explicitly imported.

## Verification

- Add fixture-backed tests for pagination, open/closed inclusion, PR exclusion, stale-cache retention, partial per-repo failures, and catalog isolation from Task readiness.
- Update Workspace and GitHub catalog docs and command help.

---
title: Add Workspace-aware doctor diagnostics
status: todo
type: task
priority: 2
labels:
- enhancement
- ready-for-agent
depends_on:
- orbit-f1hqst.84hvkj
- orbit-f1hqst.ajym98
- orbit-f1hqst.r02zj4
- orbit-f1hqst.a2xpf8
- orbit-f1hqst.b2w48k
- orbit-f1hqst.35n35x
- orbit-f1hqst.vh1xg8
parent: orbit-f1hqst
area: cli
created_at: 2026-10-28T22:48:35.120531Z
updated_at: 2026-10-28T22:48:45.170926Z
---

## Goal

Give users one safe diagnostic view of Workspace, Board, Remote Issue catalog, Linked Task sync, and child-repo agent setup health.

## Acceptance criteria

- `orbit doctor` works from the Workspace root and any discovered child repo, and reports which Workspace/Board it resolved.
- Diagnose missing or malformed Workspace layout, undiscovered/unreachable/stale repositories, unsupported organization/origin, GitHub auth/scopes/Issue Fields, and cache freshness/partial-refresh errors.
- Diagnose duplicate GitHub identities across Tasks, orphaned Remote Issue links, sync conflicts, invalid migration remnants, and local Board conflicts.
- Diagnose missing or conflicting managed agent instructions/MCP setup for Claude Code, Codex, Cursor, OpenCode, and detected Matt skill integration.
- Human and JSON reports identify severity, affected repo/Task/Issue, cause, and a safe next action.
- Doctor never resolves semantic conflicts, merges/deletes Tasks, detaches repositories, closes Issues, or overwrites user-authored agent files. Existing deterministic `--fix` operations remain explicitly limited to safe local repairs.

## Verification

- Integration tests cover healthy Workspace, missing child repo, inaccessible GitHub repo, stale cache, duplicate Issue identity, sync conflict, migration conflict, and setup drift.
- Confirm all diagnostics are read-only unless a documented deterministic `--fix` is explicitly requested.

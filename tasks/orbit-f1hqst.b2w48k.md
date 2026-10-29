---
title: Make Orbit authoritative for Linked Task synchronization
status: done
type: task
priority: 1
labels:
- enhancement
- ready-for-agent
depends_on:
- orbit-f1hqst.cy4kzy
- orbit-f1hqst.a07zbd
parent: orbit-f1hqst
checkpoint:
  by: rodrigomendes
  at: 2026-10-29T00:14:44.725310Z
  progress: Implemented Orbit-authoritative explicit GitHub sync, conflict resolution, relationship handling, retry-safe partial apply, fixtures, tests, and docs. Commit ceb6a94; merged latest integration/workspace-issue-catalog (already up to date). cargo test and clippy pass.
  next: Review implementation and close the ticket when accepted.
area: cli
created_at: 2026-10-28T22:48:16.365154Z
updated_at: 2026-10-29T00:16:30.679793Z
---

## Goal

Keep Linked Tasks aligned with their GitHub Issues while making Orbit the editing/source-of-truth surface and preserving explicit, conflict-safe synchronization.

## Acceptance criteria

- Linked Tasks are uniquely paired to one GitHub Issue by `owner/repo#number`.
- Explicit sync preview shows pull, push, merge, and conflict decisions. `--apply` is required to write; no background polling or webhook writes.
- Orbit is authoritative for linked title, body, status, labels, `area`, `verification`, and supported relationships. Local edits are pushed to GitHub on sync.
- Remote-only edits can be incorporated when the corresponding Orbit field has not changed since the last baseline. If both sides changed the same field, present values and require the user to choose pull or keep/push.
- Orbit `todo`/`in_progress` keeps the GitHub Issue open; `done` and `cancelled` close it; reopening in Orbit reopens it. Remote status changes are surfaced under the same conflict/baseline rules.
- Labels are synchronized according to the local value; relationships map to GitHub sub-issues and blocked-by links only where supported. Unsupported cross-repo edges are reported, not flattened. GitHub assignees remain separate from Orbit claims.
- The Task's sync baseline is updated only after a successful apply; partial failures identify affected Tasks and remain safely retryable.

## Verification

- Fixture-backed tests cover local-only changes, remote-only changes, same-field conflicts, status transitions, labels, metadata fields, relationships, and retry after partial failure.
- Document conflict resolution and Orbit-authoritative field rules.

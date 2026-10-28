---
title: Export Local Tasks as GitHub Issues
status: in_progress
type: task
priority: 1
labels:
- enhancement
- ready-for-agent
depends_on:
- orbit-f1hqst.r02zj4
parent: orbit-f1hqst
area: cli
claimed_by: rodrigomendes
claimed_at: 2026-10-28T23:20:53.946201Z
lease_until: 2026-10-28T23:51:13.943207Z
created_at: 2026-10-28T22:47:50.481711Z
updated_at: 2026-10-28T23:21:13.943207Z
---

## Goal

Let a user publish selected Local Tasks to a Workspace repository as GitHub Issues and turn them into Linked Tasks.

## Acceptance criteria

- Export supports one Task or an explicitly selected batch and previews repository, Issue content, labels, fields, and status before writing.
- Active Local Tasks are eligible by default; done/cancelled Tasks require an explicit include-terminal option.
- Require or infer a destination repository only when unambiguous; reject a Task already linked to an Issue and direct the user to sync instead.
- Create the GitHub Issue first, then persist its stable repository/number link and sync baseline on the Task only after GitHub confirms creation.
- Preserve Orbit Task identity, relationships, checkpoints, and local source-of-truth semantics.
- Report per-Task successes and failures for a batch; rerunning must not silently create duplicate Issues for already-linked Tasks.
- `--json` and human previews/results are consistent; no writes occur before explicit apply/confirmation.

## Verification

- Add fixture-backed tests for single/batch export, terminal-task opt-in, ambiguous repo, existing links, partial failures, and link persistence.
- Document the export and retry workflow.

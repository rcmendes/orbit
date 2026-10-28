---
title: Persist claim checkpoints across agent handoffs
status: todo
type: task
priority: 2
created_at: 2026-10-28T14:07:08.761511Z
updated_at: 2026-10-28T14:07:08.761511Z
---

## Context

When an agent's claim becomes stale or is released, the next agent needs a concise, structured handoff rather than having to reconstruct progress from comments or logs.

## Scope

Add an optional latest checkpoint associated with the active work on a Task and stored on the shared Orbit Board. It is Orbit-owned execution state, not GitHub Issue metadata.

## Acceptance criteria

- A claimant can record and update a checkpoint with current progress, the next useful action, and any blocker.
- The checkpoint is visible to a subsequent claimant through normal task-detail/agent workflows, including after stale-claim takeover, release, or reassignment.
- Checkpoints do not change readiness or claim eligibility.
- When a Task becomes done or cancelled, its latest checkpoint is retained as archived history but omitted from active handoff context.
- The behavior is covered through Orbit's supported interfaces and tests; it is not synchronized to GitHub Issues.

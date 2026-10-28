---
title: Add orbit github status and diagnostics
status: in_progress
type: task
priority: 1
labels:
- enhancement
- ready-for-agent
depends_on:
- orbit-ytwjhm.z30hsp
parent: orbit-ytwjhm
checkpoint:
  by: impl-v8vgqe
  at: 2026-10-28T16:04:49.942668Z
  progress: 'Implemented and committed read-only GitHub status plus doctor diagnostics, docs/help, and integration coverage. Merged latest integrate/orbit-ytwjhm at 4aa7a350 into the ticket branch as e2657eb, preserving status diagnostics and full/basic mode plus opt-in field creation. Final cargo test: 385 passed.'
  next: Await integration merger; do not mark ticket done until merged.
area: cli
claimed_by: impl-v8vgqe
claimed_at: 2026-10-28T15:39:34.116849Z
lease_until: 2026-10-28T16:34:49.908857Z
created_at: 2026-10-28T15:23:57.647591Z
updated_at: 2026-10-28T16:04:49.942668Z
---

## Goal

Give users one read-only command that explains whether GitHub integration is healthy and what to do next.

## Scope

- Add `orbit github status` with human and `--json` output.
- Report GitHub auth identity and relevant scopes.
- Report the configured organization and linked repositories.
- Report whether `Area` and `Verification` are configured and resolved.
- Report linked Task count, pending pull/push changes, conflicts, and unsupported cross-repository relationships.
- Recommend the next command for the current state.
- Extend `orbit doctor` with GitHub checks when the provider is configured.

## Acceptance criteria

- A healthy configuration reports auth, organization, repositories, fields, and linked Task count.
- Missing auth, missing fields, unlinked repositories, pending conflicts, and unsupported relationships are visible.
- The command is read-only and does not mutate the Board or GitHub.
- `--json` exposes the same data in a stable structure.
- `orbit doctor` reports GitHub configuration problems without failing unrelated Board checks.

## Verification

- Add integration tests for healthy, unconfigured, missing-field, and conflicting states.
- Confirm the command performs no writes.

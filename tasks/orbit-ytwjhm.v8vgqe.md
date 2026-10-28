---
title: Add orbit github status and diagnostics
status: todo
type: task
priority: 1
labels:
- enhancement
- ready-for-agent
depends_on:
- orbit-ytwjhm.z30hsp
parent: orbit-ytwjhm
area: cli
created_at: 2026-10-28T15:23:57.647591Z
updated_at: 2026-10-28T15:23:57.647591Z
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

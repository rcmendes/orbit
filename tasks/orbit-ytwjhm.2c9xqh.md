---
title: Add explicit GitHub basic mode for personal accounts
status: in_progress
type: task
priority: 1
labels:
- enhancement
- ready-for-agent
parent: orbit-ytwjhm
checkpoint:
  by: impl-2c9xqh
  at: 2026-10-28T15:34:20.124428Z
  progress: Implemented explicit GitHub basic mode, personal-repository create/adopt/sync behavior, structured-metadata rejection, and Triage label-only capability; focused GitHub tests pass.
  next: Finish targeted Triage verification, run requested checks, inspect docs and diff, commit, then merge latest integrate/orbit-ytwjhm into the ticket branch.
area: cli
claimed_by: impl-2c9xqh
claimed_at: 2026-10-28T15:27:21.098653Z
lease_until: 2026-10-28T16:04:20.096859Z
created_at: 2026-10-28T15:23:46.609777Z
updated_at: 2026-10-28T15:34:20.124428Z
---

## Goal

Support personal GitHub repositories explicitly for title, body, label, and open/closed sync, without pretending that organization Issue Fields are available.

## Scope

- Add an explicit provider mode, such as `mode = "basic"` under `[github]`.
- In basic mode, allow linked personal repositories and title/body/label/state sync.
- Reject or omit `area` and `verification` rather than silently dropping them.
- Do not require `orbit github setup` in basic mode.
- Make Triage's structured metadata capabilities unavailable or clearly limited in basic mode.
- Document the limitations and the upgrade path to organization Issue Fields.

## Acceptance criteria

- A personal repository can be linked and synced in basic mode.
- Attempts to push `area` or `verification` in basic mode fail with a clear explanation.
- The full mode remains unchanged for organization-owned repositories.
- Configuration validation rejects ambiguous combinations such as basic mode with required Issue Fields.
- Docs state exactly which features are unavailable in basic mode.
- Tests cover basic-mode create/adopt/sync and structured-metadata rejection.

## Verification

- Add integration tests with the GitHub fixture for personal-repository behavior.
- Run `cargo test github` and `cargo clippy -- -D warnings`.

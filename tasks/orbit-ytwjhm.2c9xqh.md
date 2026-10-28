---
title: Add explicit GitHub basic mode for personal accounts
status: done
type: task
priority: 1
labels:
- enhancement
- ready-for-agent
parent: orbit-ytwjhm
checkpoint:
  by: impl-2c9xqh
  at: 2026-10-28T15:39:04.804690Z
  progress: Implemented and documented basic mode; merged latest integration branch updates including URL-based adopt and single-linked-repository create. GitHub/Triage focused tests pass after merge. Clippy reports pre-existing repo lints.
  next: Run final clippy on merged tip, verify clean branch includes current integration tip, then close ticket with the lint limitation recorded.
area: cli
created_at: 2026-10-28T15:23:46.609777Z
updated_at: 2026-10-28T15:42:25.365128Z
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

---
title: Add opt-in GitHub Issue Field creation
status: in_progress
type: task
priority: 2
labels:
- enhancement
- ready-for-agent
depends_on:
- orbit-ytwjhm.z30hsp
parent: orbit-ytwjhm
checkpoint:
  by: impl-q567ya
  at: 2026-10-28T15:40:18.692397Z
  progress: Verified requested branch and integration base; read ticket, TDD instructions, glossary, and GitHub setup implementation. Test seam is CLI setup with JSON fixture.
  next: Inspect CLI args and GitHub field API, then add failing setup fixture tests.
area: setup
claimed_by: impl-q567ya
claimed_at: 2026-10-28T15:39:34.186238Z
lease_until: 2026-10-28T16:10:01.852254Z
created_at: 2026-10-28T15:23:57.708934Z
updated_at: 2026-10-28T15:40:18.692397Z
---

## Goal

Let an authorized organization administrator create the required Issue Fields from Orbit instead of switching to the GitHub UI.

## Scope

- Add `orbit github setup --create-fields`.
- Create `Area` as a single-select field with options `board`, `cli`, `mcp`, `setup`, `docs`, and `release`.
- Create `Verification` as a text field.
- Reuse existing fields when they already exist.
- Validate permissions before writing and report when the token lacks organization administration access.
- Keep the default `orbit github setup` read-only with respect to field creation.

## Acceptance criteria

- With sufficient permissions, one command creates or reuses both fields and stores their ids.
- Without sufficient permissions, the command fails with a clear remediation path.
- Existing fields are not duplicated or overwritten.
- Field types and Area options match the Orbit metadata model.
- Tests cover create, reuse, and permission-denied paths using a mock or fixture.

## Verification

- Add integration tests for field creation and reuse.
- Confirm `orbit github setup` without the flag still only validates and stores ids.

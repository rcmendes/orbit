---
title: Add interactive human review for Triage proposals
status: in_progress
type: task
priority: 2
labels:
- enhancement
- ready-for-agent
depends_on:
- orbit-ytwjhm.v8vgqe
parent: orbit-ytwjhm
checkpoint:
  by: impl-7x1cqh
  at: 2026-10-28T16:15:14.638138Z
  progress: Implemented and committed interactive TTY-gated triage review, proposal stdin, docs/help, and tests. Full test suite passed (390 tests); merged latest integrate/orbit-ytwjhm (already up to date at 3912363).
  next: Integration owner must merge ticket branch into integration; keep ticket open until that merge completes.
area: cli
claimed_by: impl-7x1cqh
claimed_at: 2026-10-28T16:06:52.411754Z
lease_until: 2026-10-28T16:43:05.862425Z
created_at: 2026-10-28T15:24:06.023440Z
updated_at: 2026-10-28T16:15:14.638138Z
---

## Goal

Make Triage review usable by a human without hand-editing JSON or reading raw `--json` output.

## Scope

- Add an interactive mode to `orbit triage review <task>`.
- Accept a proposal from `--proposal-file`, or from stdin when the file is `-`.
- Render a field-by-field diff for `area`, `verification`, and label additions/removals.
- Show uncertainty and no-op proposals clearly.
- Offer apply or cancel after review; applying must still require explicit confirmation.
- Keep `--json` non-interactive and backward compatible.
- Do not prompt when stdin or stdout is not a TTY.

## Acceptance criteria

- A human can review and approve a proposal without opening the JSON file.
- Cancelling leaves the Board unchanged.
- Confirming applies the same Orbit-first changes as `triage apply --approve`.
- `--json` output remains machine-readable and stable.
- Non-interactive execution fails or proceeds deterministically without hanging.
- Tests cover preview rendering, cancellation, approval, and non-TTY behavior.

## Verification

- Add integration tests using a proposal fixture and simulated input.
- Confirm no GitHub writes occur during review.

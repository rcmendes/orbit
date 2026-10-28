---
title: Enforce canonical triage label invariants
status: in_progress
type: task
priority: 1
labels:
- enhancement
- ready-for-agent
parent: orbit-dkyngc
area: cli
claimed_by: implement-dkyngc-2v2513
claimed_at: 2026-10-28T15:30:52.959552Z
lease_until: 2026-10-28T16:04:46.312208Z
created_at: 2026-10-28T15:20:07.717389Z
updated_at: 2026-10-28T15:34:46.312208Z
---

## Goal

Prevent `orbit triage` from producing conflicting triage state or category roles while preserving metadata-only proposals.

## Scope

- Validate the resulting label set whenever a triage proposal adds or removes labels.
- Define the state roles as `needs-triage`, `needs-info`, `ready-for-agent`, `ready-for-human`, and `wontfix`.
- Define the category roles as `bug` and `enhancement`.
- Reject a proposal whose resulting label set contains more than one state role or more than one category role.
- Do not require a state or category role for proposals that do not change labels, or for Tasks that legitimately have none.
- Keep the existing canonical-label validation and `TRIAGE_INVALID_PROPOSAL` error code, with a clear message naming the conflicting roles.
- Document the invariant in `docs/agents/triage-workflow.md`.

## Acceptance criteria

- A proposal that would leave two state roles fails with `TRIAGE_INVALID_PROPOSAL`.
- A proposal that would leave two category roles fails with `TRIAGE_INVALID_PROPOSAL`.
- A valid state transition still applies.
- An area-only or verification-only proposal still applies without requiring label changes.
- Tests cover conflicting state roles, conflicting category roles, a valid transition, and a metadata-only proposal.

## Verification

- `cargo test triage`
- `cargo fmt --check`
- `cargo clippy -- -D warnings` when Rust sources change.

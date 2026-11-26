---
status: accepted
---

# ADR 0024: Issues record when they were resolved

The Board view's Closed window (ADR 0023) needs to know when an Issue closed.
Until now it used `updated_at`, so any later edit to a closed Issue, such as a
comment-free label change, made it look newly closed.

**Decision.** Issues gain an optional `resolved_at` timestamp.

- It is set when a Transition moves an Issue from a non-closed Status into a
  `done` or `canceled` Status.
- A move between closed Statuses keeps the existing value.
- `reopen`, or any Transition into a non-closed Status, clears it.
- It is written by the same mutation that changes the Status, so it is covered
  by the Board lock and the one-commit-per-mutation rule.
- Issues closed before this change have no value. Readers fall back to
  `updated_at`; there is no migration. Issues closed on a Remote Tracker use the
  same fallback.
- `orbit list --json` and `orbit show --json` expose `resolved_at`.

## Considered options

- **Keep `updated_at`.** Rejected: edits reset the window.
- **Derive it from the Change feed.** Exact, but each poll would replay the
  Board folder's Git history.

## Consequences

- This extends the Issue model of ADR 0022 with one additive, optional field, so
  older readers ignore it. **No contract bump** (decided at implementation, stays
  `1.1.0`): the field is optional and additive, no command or argument changes,
  and an older binary preserves the unknown frontmatter key and falls back to
  `updated_at`. The `list`/`show` JSON gains `resolved_at` (`null` when unset),
  which the golden tests in `tests/contract.rs` pin.
- A future `orbit list --closed-since <duration>` can use it.

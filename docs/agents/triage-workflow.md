# Agent Triage workflow

`orbit triage` writes metadata on a **GitHub-linked** Task: `area`, task-level
`verification`, and canonical labels. It is not Matt Pocock's triage skill.
The split is in [Compatibility boundary](#compatibility-boundary).

Orbit stays local-first: these commands write the Board first. GitHub changes
only when you sync explicitly.

## Scope

A proposal may change:

- `area` (controlled values from [`project-guidance.md`](./project-guidance.md))
- Task-level `verification` (stored text; the command does not run it)
- Canonical triage/category labels: `needs-triage`, `needs-info`, `ready-for-agent`,
  `ready-for-human`, `wontfix`, `bug`, `enhancement`

`orbit triage` does **not**:

- Gather context, verify a claim, or grill
- Write an agent brief ([`agent-brief.md`](./agent-brief.md)) or maintain `.out-of-scope/`
- Adopt or import GitHub Issues (use `orbit github adopt`)
- Change GitHub-owned title, body, or open/closed state
- Add arbitrary labels
- Run verification commands

When evidence is insufficient, record `uncertainty` in the proposal and leave metadata
unchanged.

## Compatibility boundary

Orbit is the tracker. Matt Pocock's engineering skills own the triage state
machine:

- Gather context from the ticket and the codebase
- Verify the claim
- Grill when the request is underspecified
- Write the agent brief ([`agent-brief.md`](./agent-brief.md))
- Record rejected enhancements in `.out-of-scope/`

`orbit triage` is the metadata step those skills can call. Given a proposal, it
previews or writes `area`, `verification`, and canonical labels on a linked
Task. It does not decide whether a claim is true, interview anyone, compose a
brief, or create `.out-of-scope/` files.

Wayfinder map membership is a parent edge. Filtering still uses
`wayfinder:<map-id>` as documented in [`issue-tracker.md`](./issue-tracker.md);
this page does not change those commands.

## Commands

1. **Context** — load current metadata, allowed areas, canonical labels, and project guidance:

   ```bash
   orbit triage context <ID> --json
   ```

2. **Draft a proposal** — write JSON (example):

   ```json
   {
     "rationale": "CLI command surface change",
     "area": "cli",
     "verification": "cargo test triage",
     "labels_add": ["ready-for-agent"],
     "labels_remove": ["needs-triage"]
   }
   ```

   For uncertainty without changes:

   ```json
   {
     "uncertainty": "Issue body lacks reproduction steps; cannot classify area yet."
   }
   ```

3. **Review** — validate and preview without writing:

   ```bash
   orbit triage review <ID> --proposal-file proposal.json --json
   ```

4. **Apply** — requires explicit approval; updates Orbit only:

   ```bash
   orbit triage apply <ID> --proposal-file proposal.json --approve --json
   ```

5. **Sync to GitHub** — publish Orbit-owned fields and labels:

   ```bash
   orbit github sync <ID> --direction push --apply
   ```

## Proposal fields

| Field | Meaning |
| --- | --- |
| `rationale` | Optional human/agent note (not written to the task) |
| `uncertainty` | When set alone, preview shows no changes and apply is rejected |
| `area` | Set controlled `area` |
| `clear_area` | Remove `area` |
| `verification` | Set task-level verification text |
| `clear_verification` | Remove verification |
| `labels_add` / `labels_remove` | Canonical labels only |

Forbidden proposal keys (GitHub-owned): `title`, `body`, `status`, `state`.

## Errors

When a proposal adds or removes labels, Orbit checks the label set that would remain. That set may contain at most one state role (`needs-triage`, `needs-info`, `ready-for-agent`, `ready-for-human`, `wontfix`) and at most one category role (`bug`, `enhancement`). Zero of either role is allowed. A proposal that only changes `area` or `verification` does not run this check. Labels that are not state or category roles are left in place.

| Code | Meaning |
| --- | --- |
| `TRIAGE_NOT_LINKED` | Task has no GitHub link |
| `TRIAGE_APPROVAL_REQUIRED` | `apply` without `--approve` |
| `TRIAGE_INVALID_PROPOSAL` | Invalid JSON, forbidden fields, non-canonical labels, or more than one state or category role |

See also [`agent-brief.md`](./agent-brief.md), [`triage-labels.md`](./triage-labels.md), and [`issue-tracker.md`](./issue-tracker.md).

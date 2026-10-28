# Agent Triage workflow

Reusable workflow for reviewing **GitHub-linked** Orbit Tasks and proposing metadata
updates. Orbit stays local-first: triage writes the Board first; GitHub is updated only
through explicit sync.

## Scope

Triage may propose changes to:

- `area` (controlled values from [`project-guidance.md`](./project-guidance.md))
- Task-level `verification`
- Canonical triage/category labels: `needs-triage`, `needs-info`, `ready-for-agent`,
  `ready-for-human`, `wontfix`, `bug`, `enhancement`

Triage does **not**:

- Adopt or import GitHub Issues (use `orbit github adopt`)
- Change GitHub-owned title, body, or open/closed state
- Add arbitrary labels
- Run verification commands

In GitHub basic mode, Triage supports label changes only. `area` and
`verification` require full mode with organization Issue Fields; basic-mode
context reports them as unavailable, and review/apply reject proposals that
change either field.

When evidence is insufficient, record `uncertainty` in the proposal and leave metadata
unchanged.

## Steps for agents

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

| Code | Meaning |
| --- | --- |
| `TRIAGE_NOT_LINKED` | Task has no GitHub link; adopt the intended Issue with `orbit github adopt <task-id> <issue-url>`. |
| `TRIAGE_APPROVAL_REQUIRED` | `apply` was run without `--approve`; review the proposal, then rerun the reported command with `--approve`. |
| `TRIAGE_INVALID_PROPOSAL` | The proposal is invalid; the error identifies the cause and directs you to edit the JSON and review it again. |

Human errors include a `Next:` action. JSON keeps the existing error envelope and
adds `cause`, `remediation`, and Task/proposal-file context to `error.details`.
Do not add GitHub-owned fields (`title`, `body`, `status`, or `state`) to a
proposal. Only use the supported metadata fields above and canonical labels.

See also [`triage-labels.md`](./triage-labels.md) and [`issue-tracker.md`](./issue-tracker.md).

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
- Adopt or import GitHub Issues (use `orbit github adopt` or the Workspace
  `orbit workspace issues import` preview/apply workflow)
- Change Task title, body, status, or open/closed state (edit those through
  normal Orbit Task commands, then review and explicitly sync them)
- Add arbitrary labels
- Run verification commands

In GitHub basic mode, Triage supports label changes only. `area` and
`verification` require full mode with organization Issue Fields; basic-mode
context reports them as unavailable, and review/apply reject proposals that
change either field.

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

Wayfinder map membership is a parent edge. Scope a map with
`orbit ready --parent` or `orbit list --parent`, as documented in
[`issue-tracker.md`](./issue-tracker.md). This page does not change those
commands.

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

3. **Review** — validate and display the proposal. Interactive review shows a field-by-field diff
   for `area`, `verification`, and label additions/removals when a proposal file is used and both
   stdin and stdout are terminals:

   ```bash
   orbit triage review <ID> --proposal-file proposal.json
   ```

   The command asks for explicit confirmation. Enter `y` or `yes` to apply; any cancel response or
   end of input leaves the Board unchanged. If either stream is not a terminal, review prints the
   diff and exits without prompting or applying. `--json` also remains non-interactive and
   preserves the JSON preview output. A proposal can instead be read from stdin with
   `--proposal-file -`:

   ```bash
   orbit triage review <ID> --proposal-file - --json < proposal.json
   ```

4. **Apply** — the explicit non-interactive alternative requires `--approve`; it updates Orbit
   only:

   ```bash
   orbit triage apply <ID> --proposal-file proposal.json --approve --json
   ```

5. **Sync to GitHub** — review and apply do not write to GitHub. Publish Orbit-owned fields and
   labels only with an explicit sync:

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

Forbidden proposal keys (not managed by Triage): `title`, `body`, `status`, `state`.

## Errors

When a proposal adds or removes labels, Orbit checks the label set that would remain. That set may contain at most one state role (`needs-triage`, `needs-info`, `ready-for-agent`, `ready-for-human`, `wontfix`) and at most one category role (`bug`, `enhancement`). Zero of either role is allowed. A proposal that only changes `area` or `verification` does not run this check. Labels that are not state or category roles are left in place.

| Code | Meaning |
| --- | --- |
| `TRIAGE_NOT_LINKED` | Task has no GitHub link; adopt the intended Issue with `orbit github adopt <task-id> <issue-url>`. |
| `TRIAGE_APPROVAL_REQUIRED` | `apply` was run without `--approve`; review the proposal, then rerun the reported command with `--approve`. |
| `TRIAGE_INVALID_PROPOSAL` | The proposal is invalid; the error identifies the cause, including conflicting state/category roles or non-canonical labels, and directs you to edit the JSON and review it again. |

Human errors include a `Next:` action. JSON keeps the existing error envelope and
adds `cause`, `remediation`, and Task/proposal-file context to `error.details`.
Do not add Task lifecycle or content fields (`title`, `body`, `status`, or
`state`) to a Triage proposal. Only use the supported metadata fields above and
canonical labels. Orbit remains authoritative for those linked Task fields;
publish their changes only through explicit, conflict-safe GitHub sync.

See also [`agent-brief.md`](./agent-brief.md), [`triage-labels.md`](./triage-labels.md), and [`issue-tracker.md`](./issue-tracker.md).

Triage apply preserves metadata on disk if the automatic Git commit fails and
reports `COMMIT_FAILED` in the output warnings. Run `orbit sync` to retry the
commit; do not repeat the proposal merely to recover the commit.

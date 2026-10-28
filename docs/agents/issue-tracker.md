# Issue tracker: Orbit

Issues and specs for this repo live in the Orbit board. A Board is Markdown on the
orphan `orbit` branch, checked out in the Orbit folder. In the default layout
that folder is `.orbit`, and every worktree of this repository shares the main
checkout's `.orbit`; a repository linked to a shared Board uses that Board
instead. Tasks are Markdown files with YAML frontmatter and are served by the
`orbit` CLI. For a workspace that spans repositories, run `orbit init` once in
the workspace folder and join each repository with
`orbit link <workspace>/.orbit`.

For the full command reference, examples, and usage scenarios, see
[../commands.md](../commands.md).

For the component map, task state machine, readiness gate, and sequence
diagrams, see [../ARCHITECTURE.md](../ARCHITECTURE.md).

## Board discovery

Every command resolves the Board in this order: `--dir`/`ORBIT_DIR`, then the
repository link, then the nearest `.orbit` walking up to the repository root (or
to `$HOME` outside one, skipping `~/.orbit`), then the main checkout's `.orbit`,
then the legacy hidden layout with a warning, then `~/.orbit` if it exists. In
practice:

```bash
mkdir -p <WORKSPACE> && cd <WORKSPACE>
orbit init                       # create <WORKSPACE>/.orbit for a multi-repository workspace
orbit link <WORKSPACE>/.orbit    # join it from a repository
orbit unlink                     # return to this repository's own Board
orbit --dir <DIR> list           # override the link for one command
```

| Goal | Command |
| --- | --- |
| Create a project-local Board | `orbit init` |
| Create a Board in a specific folder | `orbit init <DIR>` |
| Create the machine-wide fallback | `orbit init --global` |
| Join an existing Board | `orbit link <DIR>` |
| Move a legacy Board | `orbit doctor --migrate` |

A `link` warning about a shadowed Board means the repository still has its own
Board on disk; linking did not delete it. A missing linked Board fails with
`LINK_NOT_FOUND`; re-link or restore the Board. A legacy `.git/orbit/board` or
`<workspace>/board` still resolves with a warning; run `orbit doctor --migrate`
to move it to `.orbit`.

## Vocabulary

Orbit calls the unit a **Task** and a child a **SubTask**. Translate external
tracker terms rather than inventing a second model:

- **Issue** means Task. Create one with `orbit add`.
- **Sub-issue** means SubTask. Create one with `--parent <id>`.
- **Epic** or **spec** means a top-level Task with `type: epic`. Create it with
  `orbit add "..." --type epic`. Put the scope, decisions, and acceptance
  criteria in its body.

`kind` is structural (`task` or `subtask`). `type` is the work shape (`task`,
`epic`, `spike`, `chore`, or `decision`) and is metadata only. `epic` is refused
on a SubTask. See [ADR 0010](../adr/0010-structural-type-field.md).

The `epic` label is legacy: Orbit does not infer `type: epic` from it, and
`--exclude-label epic` still works on older boards. New work should use
`--type epic` and `--exclude-type epic`.

## Conventions

- Use a stable agent identity per session and pass it as `--by` on `claim`,
  `heartbeat`, `done`, `cancel`, and `comment` (for example `--by opencode`).
- Never edit task files or YAML frontmatter directly. State, label, and comment
  changes go through the CLI.
- `orbit ready --json` returns `data.tasks`, each with `id`, `title`, `priority`,
  `labels`, `blocking`, and claim fields.
- `orbit show <ID> --json` returns the full task, including `body`, `labels`,
  `kind`, `type`, `area`, `verification`, `parent`, `subtasks`, `dependents`, and `comments`.
- `orbit list` prints an aligned table for humans. It truncates long titles and
  its columns are presentation only; parse `--json` in automation. `orbit ready`
  and `orbit tree` use compact summary lines for the same reason.
- Break a feature into SubTasks with `orbit add "<title>" --parent <parent-id>`.
  SubTasks are one level deep. Use `--depends-on` only for real blockers between
  units of work; the Parent Task waits for its SubTasks automatically.
- Use `orbit ready --parent <id>` as the frontier for one epic or spec. A Parent
  Task is not Ready while any SubTask is open.
- Use `orbit tree <id>` to see the parent, its SubTasks, and their blocking edges
  in one scoped view. `orbit prime` lists parents whose SubTasks are all resolved
  as close candidates; apply the finishing task's result and close the parent
  explicitly.
- Use `--type epic` or `--exclude-type epic` on `list` or `ready` to find or
  hide spec/epic containers. `--exclude-label epic` remains for legacy boards.
- Never use `done --force` or `cancel --force` on a Parent Task with open
  SubTasks: it cancels every open child. Close or cancel the children first, or
  leave the parent open until the family is finished.
- In a multi-repository workspace, add `--repo <name>` to `orbit ready` or
  `orbit list` to narrow the queue to one repository label.

## Labels

Labels are lowercase strings stored on the task. Add and remove them with:

```bash
orbit label add <ID> <label>...
orbit label remove <ID> <label>...
orbit label list
```

The canonical triage state labels are `needs-triage`, `needs-info`,
`ready-for-agent`, `ready-for-human`, and `wontfix`. The category labels are `bug`
and `enhancement`; these stay labels, not Types. `needs-triage`, `needs-info`, and `ready-for-human` are **hold
labels**: tasks carrying one are not Ready and cannot be claimed until the label is
removed. `ready-for-agent`, `bug`, and `enhancement` are neutral.

## Comments

Comments are append-only and carry an author and timestamp:

```bash
orbit comment add <ID> --by <agent> --body "Triage notes..."
orbit comment list <ID>
```

Every triage comment must begin with:

> *This was generated by AI during triage.*

Comments on terminal tasks are allowed. Do not edit or delete comments; add a new
correcting comment instead.

## Checkpoints

A checkpoint is the structured handoff for active work: progress, the next useful
action, and any blocker. Only the claim holder can write it:

```bash
orbit checkpoint <ID> --by <agent> --progress "..." --next "..." --blocker "..."
orbit checkpoint <ID> --by <agent> --clear-blocker
```

The record survives `release`, `claim --steal`, and `reopen`, so the next claimant
sees it in `show`, `ready`, MCP `get`, or the claim result. It never changes
readiness or claim eligibility. On a done or cancelled task the latest checkpoint
stays as archived history and new checkpoints are refused; use a comment for notes
after closing.

## When a skill says "publish to the issue tracker"

Create one task per ticket, blockers first, and capture the returned `data.id`:

```bash
orbit add "<title>" -p <0-3> --body-file - --depends-on <blocker-id>...
```

Feed the Markdown body on stdin. Add `--label ready-for-agent` to publish a ticket
that is ready for an agent, or add it afterward with `orbit label add`. If the
source is an existing issue or spec, create each ticket as a SubTask with
`--parent <source-id>`; otherwise create top-level Tasks. Use `--type epic` when
publishing a spec or epic parent. Then use the returned ID for any dependent
ticket. To add a blocking edge later:

```bash
orbit depend <dependent-id> <blocker-id>
```

Orbit has no label registry. Parentage is one level deep: create a SubTask with
`orbit add --parent <id>`, and use `--depends-on` only for real blockers. Labels
are free-form strings, comments are per-task threads, and blocking edges are
`--depends-on`. Put parent context and acceptance criteria in the body; use labels
for state and category, and comments for triage notes.

## When a skill runs triage

Orbit is the tracker. Matt Pocock's engineering skills own the triage state
machine: gathering context, verifying the claim, grilling, writing the agent
brief, and maintaining `.out-of-scope/`.

`orbit triage` is a deterministic metadata primitive for a GitHub-linked Task.
It sets `area`, task-level `verification`, and the canonical labels in
[`triage-labels.md`](./triage-labels.md). It does not verify a claim, grill,
write a brief, or update `.out-of-scope/`. The commands are in
[`triage-workflow.md`](./triage-workflow.md).

Before you implement a `ready-for-agent` ticket, read the agent brief. The
template is in [`agent-brief.md`](./agent-brief.md).

Wayfinder map membership is the parent edge. `wayfinder:<map-id>` stays a
filter; the commands stay in [Wayfinding operations](#wayfinding-operations).

## Triage operations

- **Unlabeled:** `orbit list --unlabeled --json`
- **Needs triage:** `orbit list --label needs-triage --json`
- **Needs info:** `orbit list --label needs-info --json`
- **Ready for agents:** `orbit list --label ready-for-agent --json`
- **Category:** add `bug` or `enhancement` with `orbit label add`.
- **State transition:** remove the old state label, add the new one.
- **Wontfix:** add the `wontfix` label and close with
  `orbit cancel <ID> --by <agent>`.
- **Comments:** `orbit comment add <ID> --by <agent> --body ...`; read with
  `orbit comment list <ID>` or `orbit show <ID> --json`.

For GitHub-linked Tasks, use the structured workflow in
[`triage-workflow.md`](./triage-workflow.md): `orbit triage context`, `review`, and
`apply --approve`, then `orbit github sync --direction push --apply`.

## When a skill says "fetch the relevant ticket"

```bash
orbit show <ID> --json
```

Use `orbit list --all --json` to inspect the board. `orbit ready --json` lists only
unclaimed tasks whose dependencies are done or cancelled and which carry no hold
label.

## Claiming and completing work

```bash
orbit ready --claim --by opencode
orbit heartbeat <ID> --by opencode
orbit checkpoint <ID> --by opencode --progress "..." --next "..."
orbit done <ID> --by opencode --comment "Implemented and verified."
orbit cancel <ID> --by opencode
```

Pass `--comment` (or `--comment-file`) to `done` or `cancel` when the
terminal note belongs with the transition; it is written and committed with the
status change. Use `orbit comment add` for notes on an active task. Record a
checkpoint before releasing or handing off so the next claimant inherits the
state.

If a claim is stale, take it over with
`orbit claim <ID> --by opencode --steal`. If the board returns `LOCKED`, retry
after a short backoff.

## Wayfinding operations

A Wayfinder map is a Parent Task labelled `wayfinder:map`; its children are
SubTasks created with `--parent <map-id>`. The parent edge records the
relationship; the `wayfinder:<map-id>` label remains useful for filtering.

- **Map**: one Task labelled `wayfinder:map`. Put the destination, scope, and
  decisions in its body, then capture the returned `data.id`:

  ```bash
  orbit add "Wayfinder: <destination>" --label wayfinder:map --body-file - --json
  ```

  Find maps with `orbit list --label wayfinder:map --json`.

- **Child SubTask**: create one SubTask per child with the map as its parent and
  the map's per-map label:

  ```bash
  orbit add "<child title>" --parent <map-id> --label "wayfinder:<map-id>" --body-file - --json
  ```

- **Dependency**: `--depends-on <dependency-id>` at creation or
  `orbit depend <task-id> <dependency-id>` later creates a real Dependency: the
  first Task cannot become Ready until the second is `done` or `cancelled`. Use it
  only when one child must wait for another. Never use `depend` to record
  parentage; `--parent` does that, and the map waits for its SubTasks
  automatically.

- **Frontier**: the Ready SubTasks of one map, not the whole Board:

  ```bash
  orbit ready --parent <map-id> --json
  ```

  `orbit ready --parent` returns only that map's Ready SubTasks; `--claim` takes
  the first one. Without `--parent`, `orbit ready --claim` claims the
  highest-priority Ready Task anywhere on the Board.

- **Resolve**: `orbit done` each SubTask as its work lands. The map becomes Ready
  when every SubTask is done or cancelled, and `orbit done` closes it.

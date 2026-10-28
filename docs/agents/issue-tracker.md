# Issue tracker: Orbit

Issues and specs for this repo live in the Orbit Board. A Board is Markdown on
the orphan `orbit` branch, checked out in an Orbit folder. A repository-local
Board is `.orbit`; every worktree shares it. A Workspace Board is
`<workspace>/.orbit/board` and its separate rebuildable catalog cache is
`<workspace>/.orbit/cache`. Tasks are Markdown files with YAML frontmatter and
are served by the `orbit` CLI.

For the full command reference, examples, and usage scenarios, see
[../commands.md](../commands.md).

For the component map, task state machine, readiness gate, and sequence
diagrams, see [../ARCHITECTURE.md](../ARCHITECTURE.md).

## Board discovery

Every command resolves the Board in this order: `--dir`/`ORBIT_DIR`, repository
link, containing Workspace Board (walking above child Git roots), nearest
repository `.orbit`, main checkout `.orbit`, legacy hidden layout, then existing
`~/.orbit`. The Workspace root is the user-facing path for initialization and
overrides. Repositories inside it need no per-repository link; repositories
outside it can join explicitly. In practice:

```bash
mkdir -p <WORKSPACE> && cd <WORKSPACE>
orbit init <WORKSPACE>           # create <WORKSPACE>/.orbit/board
orbit --dir <WORKSPACE> list     # use that root as an override
orbit link <WORKSPACE>           # join it from an outside repository
orbit unlink                     # return to this repository's own Board
orbit --dir <DIR> list           # override the link for one command
```

| Goal | Command |
| --- | --- |
| Create a project-local Board | `orbit init` |
| Initialize a Workspace Board | `orbit init <WORKSPACE>` |
| Consolidate existing Workspace and child Boards | `orbit workspace migrate --dry-run`, then `orbit workspace migrate` |
| Refresh or inspect its repository catalog | `orbit workspace refresh`, `orbit workspace status` |
| Override repository discovery | `orbit workspace include OWNER/REPO`, `orbit workspace exclude OWNER/REPO` |
| Create the machine-wide fallback | `orbit init --global` |
| Join an existing Board | `orbit link <DIR>` |
| Move a legacy Board | `orbit doctor --migrate` |

Workspace repository discovery is explicit: `status` reads the last refresh,
and `refresh` scans direct children without importing GitHub Issues as Tasks.

A child-local Board that competes with its Workspace Board is reported as
`AMBIGUOUS_BOARD`; commands keep using the Workspace Board and do not delete the
local copy. Creating the first Workspace Board with `orbit init` refuses to
shadow an existing child Board or conflicting `.orbit` contents. Use
`orbit workspace migrate` to preview and combine existing Boards; when no
Workspace Board exists, it stages the merged Board and activates it after
validation. Source Boards remain as backups. A missing linked Board fails with
`LINK_NOT_FOUND`; re-link or restore it. Legacy `.git/orbit/board` and
`<workspace>/board` layouts continue to resolve with a warning and can be
migrated with `orbit doctor --migrate`. A legacy Board directly at
`<workspace>/.orbit` is reported during Workspace init; keep using its explicit
Board path or migrate it to `<workspace>/.orbit/board` before retrying.

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

Wayfinder map membership is the parent edge. Scope a map with
`orbit ready --parent <map-id>` or `orbit list --parent <map-id>`; the
commands are in [Wayfinding operations](#wayfinding-operations).

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

GitHub sync and Triage require Orbit 0.9.0 or newer. For GitHub-linked Tasks,
use the structured workflow in
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

A map is a Parent Task labelled `wayfinder:map`. Each child is a SubTask
created with `--parent <map-id>` and labelled `wayfinder:<type>`, where type is
one of `research`, `prototype`, `grilling`, or `task`. Map membership is the
parent edge. Scope a map with `orbit ready --parent <map-id>` or
`orbit list --parent <map-id>`. Do not record membership with a per-map label.

GitHub assignees are not Orbit claims and are not used for Wayfinder ownership.
Claim with `orbit claim <id> --by <agent>` or
`orbit ready --parent <map-id> --claim --by <agent>`.

Blocking edges stay `--depends-on` / `orbit depend`. Resolution stays
`orbit done` or `orbit cancel`.

- **Map**: one Parent Task labelled `wayfinder:map`. Put the destination, scope,
  and decisions in its body, then capture the returned `data.id`:

  ```bash
  orbit add "Wayfinder: <destination>" --label wayfinder:map --body-file - --json
  ```

  Find maps with `orbit list --label wayfinder:map --json`.

- **Child SubTask**: create one SubTask per child with the map as its parent and
  one type label: `wayfinder:research`, `wayfinder:prototype`,
  `wayfinder:grilling`, or `wayfinder:task`.

  ```bash
  orbit add "<child title>" --parent <map-id> --label wayfinder:research --body-file - --json
  ```

- **Dependency**: `--depends-on <dependency-id>` at creation or
  `orbit depend <task-id> <dependency-id>` later creates a real Dependency: the
  first Task cannot become Ready until the second is `done` or `cancelled`. Use it
  only when one child must wait for another. Never use `depend` to record
  parentage; `--parent` does that, and the map waits for its SubTasks
  automatically.

- **Claim**: an Orbit lease. GitHub assignees do not own the ticket.

  ```bash
  orbit claim <id> --by <agent>
  orbit ready --parent <map-id> --claim --by <agent>
  ```

  `orbit ready --parent` returns only that map's Ready SubTasks; `--claim` takes
  the first one. Without `--parent`, `orbit ready --claim` claims the
  highest-priority Ready Task anywhere on the Board.

- **Frontier**: the Ready SubTasks of one map, not the whole Board:

  ```bash
  orbit ready --parent <map-id> --json
  orbit list --parent <map-id> --json
  ```

- **Resolve**: `orbit done <id> --by <agent>` or `orbit cancel <id> --by <agent>`
  each SubTask as its work lands. The map becomes Ready when every SubTask is
  done or cancelled, and `orbit done` closes it.

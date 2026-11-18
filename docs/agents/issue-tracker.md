# Issue tracker: Orbit

Issues and specs for this repo live in the Orbit Board. Every Board is a
standalone Git repository on the orphan `orbit` branch (ADR 0019), checked out
in a Board folder. A repository Board lives at `./.orbit/board`; a Board
anchored outside Git lives at `<anchor>/.orbit/board` with its rebuildable
member inventory at `<anchor>/.orbit/cache`. Issues are Markdown files with YAML
frontmatter and are served by the `orbit` CLI.

For the full command reference, examples, and usage scenarios, see
[../commands.md](../commands.md).

For the component map, issue state machine, readiness gate, and sequence
diagrams, see [../ARCHITECTURE.md](../ARCHITECTURE.md).

## Board discovery

Every command resolves the Board in this order: `--dir`/`ORBIT_DIR`, repository
link pointer, nearest `.orbit` walking up across Git boundaries (restarted
from a worktree's main checkout), the retired in-Git store for migration
diagnostics, then nothing. The anchor folder is the user-facing path for
initialization and overrides. Repositories inside it are member Repositories
that need no explicit join; repositories outside it can join with
`orbit repo add`. In practice:

```bash
mkdir -p <ANCHOR> && cd <ANCHOR>
orbit init <ANCHOR>             # create <ANCHOR>/.orbit/board
orbit --dir <ANCHOR>/.orbit/board list   # use that Board as an override
orbit repo add <REPO>           # join a repository from outside the anchor
orbit repo remove <NAME>        # drop an explicit membership
orbit repo list                 # discovered and explicit member Repositories
orbit repo refresh              # rebuild the member inventory
```

| Goal | Command |
| --- | --- |
| Create a project-local Board | `orbit init` |
| Initialize an anchor Board | `orbit init <ANCHOR>` |
| Join an existing Board from a repository | `orbit repo add <DIR>` |
| Manage member Repositories | `orbit repo list`, `orbit repo refresh`, `orbit repo remove` |
| Create an ordinary Board in the home directory | Run `orbit init` in `~` |
| Convert a pre-ADR-0019 Board | `orbit migrate` (preview, then `--apply`) |
| Share the Board across machines | `orbit board host`, `orbit board sync` |

When `orbit init` runs on an anchor that still contains child Boards, it
previews and merges them: Issue IDs are preserved, imported Issues record their
Repository, differing child prefixes move to `id.accepted_prefixes`, and the
child Boards stay on disk under `<anchor>/.orbit/backups/`.

A member-local Board that competes with its anchor Board is reported as
`AMBIGUOUS_BOARD`; commands keep using the resolved Board and do not delete
the local copy. Creating the first anchor Board with `orbit init` refuses to
shadow an existing member Board or conflicting `.orbit` contents. A legacy
Board directly at `<anchor>/.orbit` is reported during init; keep using its
explicit Board path or migrate it to `<anchor>/.orbit/board` before retrying.
Legacy `.git/orbit/board` and `<anchor>/board` layouts continue to resolve
with a warning and are converted with `orbit migrate`.

## Vocabulary

Orbit calls the unit a **Issue** and a child a **sub-issue**. Translate external
tracker terms rather than inventing a second model:

- **Issue** means Issue. Create one with `orbit add`.
- **Sub-issue** means sub-issue. Create one with `--parent <id>`.
- **Epic** or **spec** means a top-level Issue with `type: epic`. Create it with
  `orbit add "..." --type epic`. Put the scope, decisions, and acceptance
  criteria in its body.

`kind` is structural (`task` or `subtask`). `type` is the Issue type from the
Board's `board.types` (`epic`, `story`, `task`, `bug`, `sub-task` by default)
and decides where in `board.hierarchy` the Issue may sit: a child must be on a
strictly deeper level than its parent. See [ADR 0010](../adr/0010-structural-type-field.md)
and [ADR 0022](../adr/0022-issue-model-and-platform-architecture.md).

The `epic` label is legacy: Orbit does not infer `type: epic` from it, and
`--exclude-label epic` still works on older boards. New work should use
`--type epic` and `--exclude-type epic`.

On a Board with the `mattpocock` profile applied (`orbit setup mattpocock`),
the skills' `bug` and `enhancement` categories are Issue types — `bug` and
`story` — not labels; see [triage-labels.md](./triage-labels.md).

## Conventions

- Use a stable agent identity per session and pass it as `--by` on `claim`,
  `heartbeat`, `done`, `cancel`, and `comment` (for example `--by opencode`).
- Never edit task files or YAML frontmatter directly. State, label, and comment
  changes go through the CLI.
- `orbit ready --json` returns `data.issues`, each with `id`, `key`, `title`,
  `priority`, `labels`, `blocking`, and claim fields.
- Every Issue has a sequential project key (`ORB-12`) allocated at creation;
  it resolves anywhere an Issue ID does (`orbit show ORB-12`, `--parent ORB-3`),
  case-insensitively. The immutable `id` in JSON remains the file name; `key`
  and `key_aliases` carry the handles. Human `list`/`show` lead with the key.
- `orbit show <ID> --json` returns the full issue, including `body`, `labels`,
  `kind`, `type`, `component`, `verification`, `parent`, `subtasks`, `dependents`, and `comments`.
- `orbit list` prints an aligned table for humans. When the Board has remote
  material, an `ISSUE` column shows `-` for a Local Issue, `#N` for a Linked
  Issue, and `~#N` for detached provenance; use `--linked` or `--local` to
  filter. It truncates long titles and its columns are
  presentation only; parse `--json` in automation. `orbit ready` and `orbit
  tree` use compact summary lines for the same reason.
- Break a feature into sub-issues with `orbit add "<title>" --parent <parent-id>`.
  sub-issues nest as deep as `board.hierarchy` allows. Use `orbit depend`
  only for real blockers between
  units of work; the Parent Issue waits for its sub-issues automatically.
- Use `orbit ready --parent <id>` as the frontier for one epic or spec. A Parent
  Issue is not Ready while any sub-issue is open.
- Use `orbit tree <id>` to see the parent, its sub-issues, and their blocking edges
  in one scoped view. `orbit prime` lists parents whose sub-issues are all resolved
  as close candidates; apply the finishing issue's result and close the parent
  explicitly.
- Use `--type epic` or `--exclude-type epic` on `list` or `ready` to find or
  hide spec/epic containers. `--exclude-label epic` remains for legacy boards.
- Never use `done --force` or `cancel --force` on a Parent Issue with open
  sub-issues: it cancels every open child. Close or cancel the children first, or
  leave the parent open until the family is finished.
- Inside a member Repository, `orbit ready` and `orbit list` scope to it plus
  Board-wide Issues; `--all` widens to the whole Board.

## Labels and triage state

Labels are lowercase strings stored on the issue. Add and remove them with:

```bash
orbit label add <ID> <label>...
orbit label remove <ID> <label>...
orbit label list
```

The canonical triage vocabulary is the five roles `needs-triage`, `needs-info`,
`ready-for-agent`, `ready-for-human`, and `wontfix`, plus the category roles
`bug` and `enhancement`. How a role is recorded depends on the Board's
profile — see [triage-labels.md](./triage-labels.md) for the mapping:

- With the `mattpocock` profile applied, the state roles are workflow
  statuses reached through `orbit transition <ID> <role>` (`Triage`,
  `Needs Info`, `Ready`, and `Cancelled` with resolution `Won't Do`),
  `ready-for-human` is a `human` assignee kind, and `bug`/`enhancement` are
  the `bug`/`story` Issue types. Only the `Ready` status lists in
  `orbit ready`; the other statuses are never Ready and cannot be claimed.
- Without a profile, the roles remain labels: `needs-triage`, `needs-info`,
  and `ready-for-human` are **hold labels** that keep an issue out of
  `orbit ready` and block claims until removed; `ready-for-agent`, `bug`,
  and `enhancement` are neutral; `wontfix` is a label plus
  `orbit cancel <ID> --by <agent>`.

## Comments

Comments are append-only and carry an author and timestamp:

```bash
orbit comment add <ID> --by <agent> --body "Triage notes..."
orbit comment list <ID>
```

Every triage comment must begin with:

> *This was generated by AI during triage.*

Comments on terminal issues are allowed. Do not edit or delete comments; add a new
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
readiness or claim eligibility. On a done or cancelled issue the latest checkpoint
stays as archived history and new checkpoints are refused; use a comment for notes
after closing.

## Review, evidence, and asks

Work moves through review on the `standard` and `agentic` workflows:

```bash
orbit submit <ID> --by <agent> --command "cargo test" --exit-code 0 \
  --commit <sha> --file <path>... --note "..."
orbit review <ID> --verdict approve --by <reviewer>
orbit review <ID> --verdict request-changes --reason "..." --by <reviewer>
```

`submit` needs at least one evidence flag and lands the issue in `in_review`;
the evidence lands on the `evidence` Property and the submitter on
`orbit.review`. `request-changes` requires `--reason` and returns the issue to
its author. A `[policy]` rule with `distinct_from = ["author"]` on
`request_changes` refuses a reviewer who is the author. When the only
violations are `max_attempts`, the issue escalates to `needs_human` through the
workflow's `escalate` edge instead of failing.

`orbit ask <ID> "<question>"` records an open Ask and moves the issue to
`needs_human` when the `ask` edge allows; the open Ask blocks readiness,
`claim`, `done`, and every transition except `answer` (`ASK_OPEN`) until
`orbit answer <ID> "<answer>"` records the reply and restores the recorded
status.

## When a skill says "publish to the issue tracker"

Create one issue per ticket, blockers first, and capture the returned `data.id`:

```bash
orbit add "<title>" -p <priority-name> --body-file -
```

Feed the Markdown body on stdin. On a Board with the `mattpocock` profile, a
ticket that is ready for an agent starts in `Triage` and moves with
`orbit transition <ID> ready-for-agent`; on an unprofiled Board, add
`--label ready-for-agent` (or `orbit label add` afterward). If the
source is an existing issue or spec, create each ticket as a sub-issue with
`--parent <source-id>`; otherwise create top-level Issues. Use `--type epic` when
publishing a spec or epic parent. Then use the returned ID for any dependent
ticket. To add a blocking edge later:

```bash
orbit depend <dependent-id> <blocker-id>
```

Orbit has no label registry. Parentage follows the Board's `board.hierarchy`
(default `epic` > `story`/`task`/`bug` > `sub-task`): create a sub-issue with
`orbit add --parent <id>`, and use `orbit depend` only for real blockers. Labels
are free-form strings, comments are per-issue threads, and blocking edges are
`blocks` links. Put parent context and acceptance criteria in the body; use labels
for state and category, and comments for triage notes.

## When a skill runs triage

Orbit is the tracker. Matt Pocock's engineering skills own the triage state
machine: gathering context, verifying the claim, grilling, writing the agent
brief, and maintaining `.out-of-scope/`. The five roles map onto Orbit through
the Board's profile — see [triage-labels.md](./triage-labels.md).

`orbit triage` is a deterministic metadata primitive for any Issue, Local or
Linked. It sets `component`, issue-level `verification`, and the canonical labels in
[`triage-labels.md`](./triage-labels.md). It does not verify a claim, grill,
write a brief, or update `.out-of-scope/`. The commands are in
[`triage-workflow.md`](./triage-workflow.md). Triage *state* (which role a
Issue carries) is a workflow transition under the `mattpocock` profile;
`orbit triage` still owns the metadata fields and publishes them to Linked
Issues.

Before you implement a `ready-for-agent` ticket, read the agent brief. The
template is in [`agent-brief.md`](./agent-brief.md).

Wayfinder map membership is the parent edge. Scope a map with
`orbit ready --parent <map-id>` or `orbit list --parent <map-id>`; the
commands are in [Wayfinding operations](#wayfinding-operations).

## Triage operations

On a Board with the `mattpocock` profile, triage state is a workflow status:

- **Unlabeled:** `orbit list --unlabeled --json`
- **Needs triage:** `orbit list --status Triage --json` (or
  `orbit transition <ID> needs-triage` to file one there)
- **Needs info:** `orbit list --status "Needs Info" --json`
- **Ready for agents:** `orbit ready --json` (the `Ready` status, unblocked)
- **Category:** `bug`/`enhancement` are Issue types — `orbit add --type bug`,
  `orbit update <ID> --type story`
- **State transition:** `orbit transition <ID> needs-triage|needs-info|ready-for-agent|ready-for-human|wontfix`
- **Wontfix:** `orbit transition <ID> wontfix` closes the Issue with resolution
  `Won't Do`; `orbit reopen <ID>` returns it to `Triage`.
- **Human work:** `orbit transition <ID> ready-for-human`, then
  `orbit assign <ID> --to <name>` (assignee kind `human`).
- **Comments:** `orbit comment add <ID> --by <agent> --body ...`; read with
  `orbit comment list <ID>` or `orbit show <ID> --json`.

On an unprofiled Board the same roles are labels instead: filter with
`orbit list --label <role> --json`, move state by removing the old role label
and adding the new one, and close `wontfix` with `orbit cancel <ID> --by
<agent>`.

Remote Tracker sync and Triage require Orbit 0.9.0 or newer. For Linked Issues,
use the structured workflow in
[`triage-workflow.md`](./triage-workflow.md): `orbit triage context`, `review`, and
`apply --approve`, then `orbit remote sync --direction push --apply`.

## When a skill says "fetch the relevant ticket"

```bash
orbit show <ID> --json
```

Use `orbit list --all --include-closed --json` to inspect the whole board.
`orbit ready --json` lists only
unclaimed issues in the workflow's ready status whose dependencies are in a
closed status category and which carry no hold label.

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
status change. Use `orbit comment add` for notes on an active issue. Record a
checkpoint before releasing or handing off so the next claimant inherits the
state.

If a claim is stale, take it over with
`orbit claim <ID> --by opencode --steal`. If the board returns `LOCKED`, retry
after a short backoff.

## Wayfinding operations

A map is a Parent Issue labelled `wayfinder:map`. Each child is a sub-issue
created with `--parent <map-id>` and labelled `wayfinder:<type>`, where type is
one of `research`, `prototype`, `grilling`, or `task`. Map membership is the
parent edge. Scope a map with `orbit ready --parent <map-id>` or
`orbit list --parent <map-id>`. Do not record membership with a per-map label.

GitHub assignees are not Orbit claims and are not used for Wayfinder ownership.
Claim with `orbit claim <id> --by <agent>` or
`orbit ready --parent <map-id> --claim --by <agent>`.

Blocking edges are `blocks` links, added with `orbit depend`. Resolution stays
`orbit done` or `orbit cancel`.

- **Map**: one Parent Issue labelled `wayfinder:map`. Put the destination, scope,
  and decisions in its body, then capture the returned `data.id`:

  ```bash
  orbit add "Wayfinder: <destination>" --label wayfinder:map --body-file - --json
  ```

  Find maps with `orbit list --label wayfinder:map --json`.

- **Child sub-issue**: create one sub-issue per child with the map as its parent and
  one type label: `wayfinder:research`, `wayfinder:prototype`,
  `wayfinder:grilling`, or `wayfinder:task`.

  ```bash
  orbit add "<child title>" --parent <map-id> --label wayfinder:research --body-file - --json
  ```

- **Dependency**: `orbit depend <task-id> <dependency-id>` creates a real Dependency: the
  first Issue cannot become Ready until the second is in a closed status category. Use it
  only when one child must wait for another. Never use `depend` to record
  parentage; `--parent` does that, and the map waits for its sub-issues
  automatically.

- **Claim**: an Orbit lease. GitHub assignees do not own the ticket.

  ```bash
  orbit claim <id> --by <agent>
  orbit ready --parent <map-id> --claim --by <agent>
  ```

  `orbit ready --parent` returns only that map's Ready sub-issues; `--claim` takes
  the first one. Without `--parent`, `orbit ready --claim` claims the
  highest-priority Ready Issue anywhere on the Board.

- **Frontier**: the Ready sub-issues of one map, not the whole Board:

  ```bash
  orbit ready --parent <map-id> --json
  orbit list --parent <map-id> --json
  ```

- **Resolve**: `orbit done <id> --by <agent>` or `orbit cancel <id> --by <agent>`
  each sub-issue as its work lands. The map becomes Ready when every sub-issue is in a closed
  status category, and `orbit done` closes it.

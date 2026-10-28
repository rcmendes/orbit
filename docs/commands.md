# Command guide

Status: this guide describes v0.8.2, including the Orbit folder
([ADR 0012](./adr/0012-orbit-folder.md)). The unified `init [DIR]` surface,
resolver, scoped path-free agent setup, and `doctor` migration are implemented.

Orbit has one command per operation. Every command explains itself:

```bash
orbit <command> --help    # long description, options, and examples
orbit <command> -h        # compact summary
```

This guide adds the part help cannot: **when** each command is the right tool.
Board commands print human-readable text by default; add `--json` to get exactly
one JSON object on stdout. Human output is for terminals, not parsers: `orbit
list` renders an aligned table or, with `--group`, a grouped outline, while
`ready`, `tree`, and `prime` use compact summary lines. The table's columns,
ordering, and title truncation are presentation only; `--json` is the stable
contract.

## Global options

| Option | Meaning |
| --- | --- |
| `--json` | Print exactly one JSON object on stdout, including errors. |
| `--dir <PATH>` | Use a specific Orbit folder (`DIR`) for this command; the path is not a project root. |
| `ORBIT_DIR` | Environment equivalent of `--dir`. |

Exit codes are `0` for success, `1` for a domain failure (for example
`DEPENDENCY_BLOCKED` or a failing `doctor`), and `2` for a usage error.

Task commands resolve the Board in this order: `--dir`/`ORBIT_DIR`, then the
repository link, then the nearest `.orbit` walking up to the repository root
(or to `$HOME` outside a repository, skipping `~/.orbit`), then the main
checkout's `.orbit`, then the legacy hidden layout with a warning, then
`~/.orbit` if it exists. `--dir` and `ORBIT_DIR` name the Orbit folder directly;
a legacy Board root is accepted with a warning. `orbit init` creates a Board
instead of discovering one. See [issue-tracker.md](./agents/issue-tracker.md) for
the multi-repository model.

## Which command do I need?

| Goal | Command |
| --- | --- |
| Set up a repository or workspace | `orbit init`, `orbit link`, `orbit unlink` |
| File a new ticket | `orbit add` |
| Break work into blockers | `orbit add --depends-on`, `orbit depend`, `orbit undepend` |
| Triage incoming work | `orbit label add/remove/list` |
| Find work for an agent | `orbit ready` |
| Take a specific ticket | `orbit claim` |
| Keep a claim alive | `orbit heartbeat` |
| Hand work back | `orbit release` |
| Finish or abandon work | `orbit done`, `orbit cancel` |
| Undo a close or remove a mistake | `orbit reopen`, `orbit delete` |
| Add context to a ticket | `orbit comment add`, `orbit update` |
| Read the board | `orbit list`, `orbit show`, `orbit tree`, `orbit prime` |
| Collect or archive the board | `orbit export` |
| Wire an editor or agent | `orbit setup`, `orbit mcp` |
| Recover or maintain the board | `orbit doctor`, `orbit sync` |
| Link GitHub Issues (optional) | `orbit github` — see [github-sync.md](./github-sync.md) |
| Triage linked Tasks | `orbit triage` — see [agents/triage-workflow.md](./agents/triage-workflow.md) |

## Set up a Board

### `orbit init` — create the Board

Creates the Orbit folder (`DIR`, default `./.orbit`) and the Board inside it.
Inside a Git working tree, `DIR` is a worktree of the repository on the `orbit`
branch; outside one, it is a standalone Git repository on the same branch.
The command derives the Task-ID slug, writes the exclusion entry for worktree
mode, and prints a numbered setup report. Re-running on the same `DIR` is safe.

**Good fit:** the first time you use Orbit in a repository, when creating a
shared workspace folder, or when turning a standalone folder into a Board.

```bash
orbit init                       # Board in ./.orbit; asks about agents in a TTY
orbit init --agents none         # Board only; no agent files
orbit init --agents all          # wire every supported harness
orbit init --print               # preview every file that would change
orbit init --portable-mcp        # MCP command `orbit`; editor PATH must find it
orbit init ~/src/acme-board      # Board at ~/src/acme-board
orbit init --global              # Board at ~/.orbit
orbit init --slug teramigo       # Task IDs become teramigo-…
```

**Core options**

| Option | Use it when | Example |
| --- | --- | --- |
| *none* | Start a project-local Board at `./.orbit`. | `orbit init` |
| `DIR` | Choose the Board folder explicitly. Inside a working tree it becomes a worktree; outside it becomes a standalone repository. | `orbit init ~/src/acme/.orbit` |
| `--global` | Create or reuse the opt-in `~/.orbit` fallback. It conflicts with positional `DIR` and `--dir`/`ORBIT_DIR`, and discovery never creates it. | `orbit init --global` |
| `--slug SLUG` | Choose the Task-ID prefix instead of deriving it from `DIR`. Explicit input is strict; a populated Board returns `SLUG_LOCKED`. | `orbit init --slug acme` |
| `--agents LIST` | Wire agents non-interactively: `none`, `all`, `auto`, or a client list. | `orbit init --agents all` |
| `--print` | Preview the Board, exclusion entry, and generated agent files without writing or prompting. | `orbit init DIR --print` |

**Choose by scenario**

- **New product repository:** run `orbit init` at the root. The Board is `./.orbit`, shared by linked worktrees, and IDs use the folder name.
- **Shared workspace:** create a dedicated folder, run `orbit init` inside it, then `orbit link ~/src/acme/.orbit` in each repository. Use `orbit init ~/src/acme/.orbit` if you are not already in the folder.
- **CI or scripted setup:** preview with `--print`, then use `--agents none` and an explicit `--slug` when the machine should not receive editor files and IDs must be stable.
- **Global fallback:** use `--global` only for an intentional machine-wide Board. Project-local Boards still win in discovery.
- **Joining an existing Board:** use `orbit link DIR`; `init` is for creating or reusing the target `DIR`, not for joining another Board.
- **Migrating a legacy Board:** use `orbit doctor --migrate`; `init` is not a migration command.

**Supporting options**

| Option | Use it when | Example |
| --- | --- | --- |
| `--dir PATH` / `ORBIT_DIR` | You need the global DIR override for one command. Positional `DIR` wins for `init`. | `orbit --dir ~/src/acme/.orbit init` |
| `--no-agents` | You want `--agents none` without the selector. | `orbit init --no-agents` |
| `--hooks` | You also want the Claude Code SessionStart hook. | `orbit init --agents claude --hooks` |
| `--portable-mcp` | MCP configs should call bare `orbit`; the editor's PATH must resolve it. | `orbit init --portable-mcp` |
| `--mcp-bin PATH` | Record a specific Orbit binary; conflicts with `--portable-mcp`. | `orbit init --mcp-bin /usr/local/bin/orbit` |

Every run ends with the same three-section report:

1. **Board** — whether it was created or reused, its path, branch, storage mode,
   Task-ID prefix, and the `.git/info/exclude` entry in worktree mode.
2. **Coding agents** — the harnesses wired, the MCP command written and whether
   it is absolute or portable, plus every file created, updated, or already
   unchanged.
3. **Verify** — `orbit doctor`, followed by the next commands for the selected
   harnesses or, when none were wired, `orbit setup <client>`. Each next command
   carries a short explanation of what it does.

`--print` reports the same steps in future tense (`would create`, `would list`)
and adds the full generated file contents without writing anything. A standalone
Board always includes `orbit link DIR` in its next steps so member repositories
can join it.

On the first run that wires an MCP client, a terminal also asks whether MCP
configs should record an absolute binary path (the default) or the portable
`orbit` command. The absolute path works for GUI-launched editors without any
shell setup; the portable command only works when the editor's `PATH` resolves
`orbit`. Use `--portable-mcp` to choose it without a prompt. Once a selected
harness has an MCP config, later runs do not ask.

Task IDs use the Board's slug as their prefix. A `DIR` named `.orbit` uses its
parent folder name (`~/src/acme/.orbit` → `acme-`); any other `DIR` uses its own
name; `--global` uses `orb` with a warning. Pass `--slug` to choose it
explicitly. Re-running `init` preserves the stored prefix; an empty Board can
change it, while a populated Board returns `SLUG_LOCKED` and points at
`orbit slug migrate <slug>`. `--print` reports the same result without writing.
The JSON and human output carry `prefix`, `storage`, and the worktree `exclude`
entry; the JSON also reports `global`, `created`, `preview`, `portable_mcp`, and
the managed files.

`init` takes `DIR` from the positional argument, then `--dir`/`ORBIT_DIR`, then
`./.orbit`; in a linked worktree the default is the main checkout's `.orbit`.
`--global` is exclusive with those and means `~/.orbit`. `link` uses only its
positional `DIR`; `--dir`/`ORBIT_DIR` never retarget the pointer. A repository
can own one local Board; a second `.orbit` in the same working tree is refused.
A nested repository maintains its own resolution scope.

Without `--agents`, a terminal asks whether to wire detected harnesses; without
a terminal, `init` writes no agent files and explains how to wire them with
`orbit setup`. A standalone Board always includes its `orbit link` hint.
`--agents` accepts `none`, `all`, or a comma-separated client list. `init` refuses to create a
Board that would compete with an existing repository-local resolution for `DIR`:
a link, an ancestor `.orbit`, the main checkout's `.orbit`, or a legacy Board is
reported as `ALREADY_INITIALIZED` before anything is written. `--global` and the
`~/.orbit` fallback are exempt. `orbit unlink` clears a link; move or migrate an
ancestor, main-checkout, or legacy Board.

### A shared workspace

A workspace is one Board plus every repository linked to it. Create the Board in
a dedicated folder, then join each repository:

```bash
mkdir -p ~/src/acme && cd ~/src/acme
orbit init ~/src/acme/.orbit --slug acme   # or just: orbit init

cd ~/src/acme/api
orbit link ~/src/acme/.orbit
```

`orbit init` only creates the Board. It does not clone or link repositories.

### `orbit link` — reuse a Board from another repository

Points this repository at an existing Orbit folder, including from all of its
worktrees. The pointer is absolute and lives in the Git common directory. It
also refreshes any existing managed client files; generated configs contain no
Board path, so this is a version reconciliation, not a path rewrite. The refresh
keeps the configured MCP command mode, so a portable `orbit` entry stays
portable. Pass the Orbit folder itself (`.../.orbit`); `link` does not append
`.orbit`.

**Good fit:** a workspace with several related repositories (API, web app,
agents) where one product Board should hold all of the work. Create the Board in
the workspace folder, then `link` in each repository.

```bash
orbit link ~/src/acme/.orbit          # join a shared Board
orbit link ~/src/acme/.orbit --no-refresh   # leave agent configs alone
```

Linking does not delete a local Board. If one exists it is shadowed and `link`
warns about it; the main checkout's `.orbit` and a legacy local Board are both
reported. A legacy target — an old `<workspace>/board` or a board root — is
accepted with a `LEGACY_LAYOUT` warning that continues while the linked Board
keeps a legacy layout. A target that holds no Board returns `LINK_NOT_FOUND`; the
pointer is not repaired automatically.

### `orbit unlink` — go back to a local Board

Removes the shared-Board pointer and refreshes any existing managed client files.
The shared Board is untouched.

**Good fit:** a repository is leaving the workspace, or you want to replace a
link with an isolated Board.

```bash
orbit unlink
orbit init        # create the local Board if it does not exist yet
```

### `orbit slug` — inspect or migrate the Task-ID identity

`slug show` reports the bare slug, the literal `id.prefix`, and the number of
Tasks, SubTasks, and comment files. It is read-only. `slug migrate <SLUG>`
rewrites every Task and SubTask ID from the current prefix to `<SLUG>-`.

**Good fit:** an early Board was created under a repository or workspace name
that no longer fits. Migration is the explicit escape hatch from the one-prefix
rule; `init --slug` remains the creation and empty-Board path.

```bash
orbit slug show                        # orb, orb-, 42 tasks, 17 SubTasks
orbit slug migrate atlas --dry-run     # print the plan; write nothing
orbit slug migrate atlas               # rewrite, validate, and commit once
orbit slug migrate atlas --force       # proceed despite live claims
```

The random suffix is preserved: `orb-a7b2c9` becomes `atlas-a7b2c9`, and
`orb-a7b2c9.d4b3df` becomes `atlas-a7b2c9.d4b3df`. Migration also rewrites
`parent` and `depends_on`, renames comment files, updates the Orbit-generated
`# Comments: <id>` header, and changes `id.prefix`. It validates the result in a
temporary Git worktree, commits once as
`orbit: migrate task prefix from orb- to atlas-`, then fast-forwards the Board.
Pending heartbeat writes are committed first, so the migration commit is the
single rollback point.

`--dry-run` reports counts, the reference count, and any live claims, but never
writes. The JSON plan also includes every `from`/`to` mapping and any collisions.
Applying with live claims requires `--force`; the affected task IDs and holders
are listed so they can re-read the Board. An unhealthy Board returns
`MIGRATION_BLOCKED` (run `orbit doctor` first), and a target that would collapse
two IDs returns `PREFIX_COLLISION`. On an empty Board migration is equivalent to
setting the prefix.

Task bodies, comment text, checkpoint text, Git history, pull requests, and external links are
**not** rewritten. A body that mentions `orb-a7b2c9` keeps that text; old links
and cached agent IDs stop resolving. `--json` reports `current`, `target`,
`changed`, `tasks`, `subtasks`, `comment_files`, `references`, `mappings`,
`claims`, `collisions`, `dry_run`, `applied`, and `commit`. `slug show` reports
`slug`, `prefix`, `tasks`, `subtasks`, and `comment_files`.

### `orbit setup` — adjust one harness

Installs, checks, or removes a harness integration without touching the Board.
`--scope project` (the default) writes repository-level files; `--scope user`
writes the harness's user-level locations. `--print`, `--check`, and `--remove`
operate on the selected scope. Generated configs record the Orbit binary but not
the Board path, so moving the Orbit folder does not stale them.

User scope writes Claude memory to `~/.claude/CLAUDE.md`, its hook to
`~/.claude/settings.json`, and MCP to `~/.claude.json`; Cursor rules, the shared
skill, and MCP to `~/.cursor/rules/orbit.mdc`, `~/.agents/skills/orbit/SKILL.md`,
and `~/.cursor/mcp.json`; Codex to `~/.codex/AGENTS.md`, the shared skill, and
`~/.codex/config.toml`; OpenCode to `~/.config/opencode/AGENTS.md` and
`~/.config/opencode/opencode.json`; and the mattpocock tracker bridge to
`~/.agents/issue-tracker.md`. `--print` names each exact file.

MCP entries record the absolute path of the running binary by default, which
keeps GUI-launched editors working without `PATH` setup. Pass `--portable-mcp`
to write the bare `orbit` command instead; every editor or agent that launches
the MCP server must then resolve Orbit on its `PATH`. A repair without
`--portable-mcp` returns entries to the absolute-path default.

**Good fit:** repairing one editor after an upgrade, checking in CI that
integration files are current, setting up a machine-wide integration, or
removing Orbit's content before uninstalling.

```bash
orbit setup claude --hooks     # instructions + MCP + SessionStart hook
orbit setup all --scope user   # machine-wide instruction and MCP files
orbit setup all --check        # installed / missing / stale
orbit setup all --portable-mcp # MCP command `orbit`; editor PATH must find it
orbit setup cursor --print     # preview
orbit setup all --remove       # remove only Orbit-managed content
```

**Core options**

| Option | Use it when | Example |
| --- | --- | --- |
| `--scope project` | Default. Write instruction and MCP files for this repository. | `orbit setup claude` |
| `--scope user` | One machine-wide integration; writes the harness's user-level locations. | `orbit setup cursor --scope user` |
| `--check` | CI or a repair check: report installed, missing, or stale and exit non-zero on drift. | `orbit setup all --check` |
| `--remove` | Remove only Orbit-managed files, sections, hooks, and MCP entries. | `orbit setup all --remove` |
| `--no-mcp` | Write instruction and skill files without MCP entries. | `orbit setup codex --no-mcp` |
| `--hooks` | Add the Claude Code SessionStart hook. | `orbit setup claude --hooks` |
| `--portable-mcp` | Write bare `orbit` in MCP configs; the editor's PATH must resolve it. | `orbit setup opencode --portable-mcp` |
| `--print` | Preview the exact files that would be written or removed. | `orbit setup all --print` |

An existing `docs/agents/issue-tracker.md` without Orbit's managed marker is
treated as a hand-written guide: setup reports it as `skipped` and leaves it
unchanged, `--check` treats the integration as satisfied, and agent refresh
still recognizes the file. Tracker files with the marker update and remove
normally.

### `orbit mcp` — expose the Board to an MCP client

Runs the Model Context Protocol server over stdio with 14 tools and the
`orbit://tasks/{id}` resource. Editors launch it from their MCP configuration.
`create` accepts `parent` and `type`; `update` accepts `type`; `checkpoint`
records the claim holder's handoff; `list` and `ready` accept `parent`, `repo`,
`labels`, `exclude_labels`, `types`, and `exclude_types` to scope a feature or
repository.

**Good fit:** an editor or agent that speaks MCP should read and update the Board
directly instead of shelling out to the CLI.

```bash
orbit mcp                          # normally launched by the editor
orbit --dir ~/src/acme/.orbit mcp   # serve a shared Board
```

## Create and shape work

### `orbit add` — file a ticket

Creates a task in `todo` and prints its ID, which you need for dependents,
labels, and comments.

**Good fit:** a bug report, a feature, a follow-up from a review, or a ticket
that a planning skill is publishing to the tracker. Put context and acceptance
criteria in the body; use labels for triage state.

```bash
# A high-priority bug with a Markdown body
orbit add "Login rejects valid tokens" -p 1 --label bug --body-file ticket.md

# Work that must wait for a blocker
orbit add "Publish the release" -p 1 --depends-on orb-a7b2c9

# A ticket waiting on something outside the Board
orbit add "Upgrade the parser" --waiting-on "upstream 2.4" --body-file -

# A top-level epic or spec
orbit add "Spec: SSO login" --type epic --body-file spec.md

# Publish several tickets, blockers first; each add prints the new ID
orbit add "Design the schema" -p 0 --label ready-for-agent
# created orb-f14c02  P0  Design the schema
orbit add "Ship the API" -p 1 --depends-on orb-f14c02 --label ready-for-agent
# created orb-a7b2c9  P1  Ship the API

# Break a feature into a SubTask
orbit add "API: add the endpoint" --parent orb-a7b2c9 --label repo:api
# created orb-a7b2c9.b3c4d5  P2  API: add the endpoint  (subtask of orb-a7b2c9)
```

`--parent` creates a SubTask under an existing Parent Task. The SubTask ID is the
parent ID, a dot, and a fresh suffix (`orb-a7b2c9.b3c4d5`); the `parent` field
stores the same parent. SubTasks are one level deep and can still have real
dependencies. The Parent Task becomes Ready only when every SubTask is done or
cancelled.

`--type` sets the work shape: `task` (default), `epic`, `spike`, `chore`, or
`decision`. Type is metadata and does not affect readiness. `epic` is refused on a
SubTask. `bug` and `enhancement` remain labels, not Types.

A spec or epic is a top-level Task with `type: epic`; its tickets are SubTasks.
This is also the compatibility mapping for Beads and Matt Pocock's skills:
**Issue** = Task, **Sub-issue** = SubTask, **Epic** = top-level Task with
`type: epic`. Put the spec in the parent body, publish tickets with `--parent`,
and use `orbit ready --parent <id>` as the frontier. There is no deeper nesting;
flatten a ticket's own breakdown into sibling tickets with `--depends-on`. Never
use `done --force` or `cancel --force` on a spec parent with open tickets: it
cancels them. See [ADR 0009](./adr/0009-keep-task-vocabulary.md) and
[ADR 0010](./adr/0010-structural-type-field.md).

`--area` sets an optional controlled classification (`board`, `cli`, `mcp`,
`setup`, `docs`, or `release`). Filter with `orbit list --area cli` or
`orbit ready --exclude-area docs`. `--verification` stores concise checks or
commands for agents; shared project defaults live in
[`docs/agents/project-guidance.md`](./agents/project-guidance.md).

### `orbit update` — edit a ticket in place

Changes the title, body, priority, type, area, verification, or external blocker
without touching status, claims, labels, or dependencies. `--type epic` is refused on a SubTask.

**Good fit:** scope changed after triage, a spec needs refinement, or an external
blocker was resolved.

```bash
orbit update orb-a7b2c9 --priority 0
orbit update orb-a7b2c9 --type spike
orbit update orb-a7b2c9 --body-file refined-spec.md
orbit update orb-a7b2c9 --clear-waiting
orbit update orb-a7b2c9 --area mcp --verification "cargo test --test mcp"
orbit update orb-a7b2c9 --clear-area --clear-verification
```

### `orbit depend` and `orbit undepend` — connect work

`depend <id> <dep-id>` makes the first task wait until the second is done or
cancelled. Cycles, self-dependencies, adding a dependency to a terminal task, and
parent/child edges in either direction are rejected. `undepend` removes the edge
and is idempotent.

**Good fit:** a blocker is discovered after both tickets exist, or a dependency
was wrong and must be removed. Use `orbit add --parent` for parentage; a
dependency blocks the child, while a parent blocks the feature.

```bash
orbit depend orb-a7b2c9 orb-f14c02   # orb-a7b2c9 waits for orb-f14c02
orbit undepend orb-a7b2c9 orb-f14c02
```

Prefer `orbit add --depends-on` when the edge is known at creation time.

### `orbit label` — triage state and category

Labels are free-form lowercase slugs. `needs-triage`, `needs-info`, and
`ready-for-human` are hold labels: while present, the task is not ready and
cannot be claimed. Adding and removing labels is idempotent.

**Good fit:** moving a ticket through triage, marking it ready for an agent, or
recording that it is a bug or enhancement.

```bash
orbit label add orb-a7b2c9 bug needs-triage
orbit label remove orb-a7b2c9 needs-triage
orbit label add orb-a7b2c9 ready-for-agent
orbit label list                       # labels in use, with counts
orbit list --label needs-triage        # everything awaiting triage
```

### `orbit comment` — keep a per-task thread

Comments are append-only, with an author and timestamp, and are allowed on closed
tasks. Corrections are new comments; nothing is edited or deleted.

**Good fit:** triage notes, handoff context, links to logs, or a record of why a
decision was made. Put durable requirements in the body instead.

```bash
orbit comment add orb-a7b2c9 --by claude --body "Reproduced on macOS."
cargo test 2>&1 | orbit comment add orb-a7b2c9 --by claude --body-file -
orbit comment list orb-a7b2c9
```

## Find work

### `orbit list` — inspect the board

Lists tasks with combinable filters. Done and cancelled tasks are hidden by
default; pass `--all`, or name a terminal status with `--status`, to include
them. Human output is an aligned table with `ID`, `STATUS`, `P`, `TYPE`,
`TITLE`, `LABELS`, and `CLAIMED BY` columns; titles longer than 60 characters
are shortened with an ellipsis. Root Tasks are always ordered by priority, then
creation time and ID; each Parent Task is followed by its SubTasks, also ordered
by priority. `--group` draws that hierarchy as one outlined table: a banner for
each Parent Task, its SubTasks underneath, and a final `No parent` section. Use
`--json` when a script or agent consumes the result.

**Good fit:** a human or agent wants an overview, a triage queue, one agent's
in-progress work, or the full history.

```bash
orbit list
orbit list --label needs-triage
orbit list --label repo:api --label ready-for-agent   # every label must match
orbit list --repo api                               # shorthand for repo:api
orbit list --status in_progress --claimed-by claude
orbit list --status done            # closed work, without --all
orbit list --unlabeled
orbit list --parent orb-a7b2c9      # only this feature's SubTasks
orbit list --type epic              # only epics and specs
orbit list --group                  # parents and SubTasks in one outlined table
orbit list --exclude-type epic      # hide spec/epic containers
orbit list --all --json
```

Human output with `--group` looks like this:

```text
╭───────────────────┬────────┬────┬──────┬─────────────────────────────┬─────────────────────────────┬────────────╮
│ ID                │ STATUS │ P  │ TYPE │ TITLE                       │ LABELS                      │ CLAIMED BY │
├─────────────────────────────────────────────────────────────────────────────────────────────────────────────────┤
│ orb-a7b2c9  Spec: SSO login across api, web, infra                           epic · todo · P1 · ready-for-agent │
├─────────────────────────────────────────────────────────────────────────────────────────────────────────────────┤
│ orb-a7b2c9.z21c2w │ todo   │ P0 │ task │ infra: register OIDC client │ ready-for-agent, repo:infra │            │
│ orb-a7b2c9.hxdmqt │ todo   │ P1 │ task │ api: OIDC callback          │ ready-for-agent, repo:api   │            │
│ orb-a7b2c9.c5qmed │ todo   │ P2 │ task │ web: login redirect         │ ready-for-agent, repo:web   │            │
╰───────────────────┴────────┴────┴──────┴─────────────────────────────┴─────────────────────────────┴────────────╯
```

The table is intentionally not a machine interface. Column widths grow with the
data, labels are joined with commas, a claim holder appears in the last column,
and long titles are truncated. Parse `--json` instead.

`--type` is repeatable and matches any listed work shape; `--exclude-type` hides
any listed shape. `--label` is repeatable and every label must match;
`--exclude-label` is repeatable and hides a task carrying any of the named labels.
`--repo NAME` is shorthand for `--label repo:NAME`. Parent groups follow the
parent's priority; SubTasks inside a group follow their own priority. Tasks with
no parent appear in the final `No parent` section. JSON always carries `kind`,
`type`, `parent`, `subtasks`, and `open_subtasks` with the full title. A Parent
Task with open SubTasks is not Ready, so `list` may show it even though `ready`
does not.

For work that can start now, use `orbit ready` instead: `list` shows all open
work, including blocked tasks.

### `orbit ready` — the agent queue

Shows tasks that are `todo`, unclaimed, not waiting on an external blocker, carry
no hold label, and have all dependencies done or cancelled. A Parent Task is not
Ready while any SubTask is open. Results are ordered by priority, then creation
time.

With `--claim`, the first task is claimed atomically in the same command and
moves to `in_progress` with a lease. When nothing is ready, the command succeeds
with `data: null` and `meta.reason: "NO_READY_TASKS"`. `--parent` scopes the
queue to one feature's SubTasks.

**Good fit:** an agent asks what to do next, a supervisor polls for idle work, or
a worker wants to take exactly one task without racing another agent.

```bash
orbit ready --limit 5
orbit ready --claim --by claude --json
orbit ready --parent orb-a7b2c9 --claim --by claude   # one feature only
orbit ready --repo api --claim --by claude            # one repository only
orbit ready --exclude-type epic --claim --by claude   # skip spec containers
orbit ready --json              # NO_READY_TASKS means wait and retry
```

`--parent`, `--repo`, repeatable `--label`, `--type`, and `--exclude-label`
combine with AND. `--type` matches any listed shape; `--exclude-type` hides any
listed shape. `--exclude-label` remains for legacy label-only boards. A released
task can still carry the previous claimant's checkpoint; `--json` includes it so
the next claim starts with the handoff context.

### `orbit show` — load one ticket

Prints the full task: body, labels, dependencies, kind, type, parent, SubTasks,
dependents, claim state, checkpoint, and comments. `--json` is the form an agent
should load before starting work. A terminal task keeps its latest checkpoint as
archived history, marked by `checkpoint_archived: true`.

**Good fit:** before implementing a ticket, when checking why a task is blocked,
or when reading a handoff checkpoint or comment.

```bash
orbit show orb-a7b2c9
orbit show orb-a7b2c9 --json
```

### `orbit tree` — inspect one epic or spec

Shows one Parent Task, its direct SubTasks, and their blocking edges. Unlike
`list --group` (the whole Board) and `list --parent` (children only), `tree`
always includes the root and its family, with resolved/total progress on the
root.

**Good fit:** inspecting a spec, epic, or feature before publishing tickets,
checking the frontier, or seeing why one ticket is blocked.

```bash
orbit tree orb-a7b2c9
orbit tree orb-a7b2c9 --json
```

`tree` expects a Parent Task; a SubTask returns `INVALID_ARGUMENT`. `--json`
returns `data.root`, `data.tasks` with the root first, and `data.count`.

### `orbit prime` — starting context for a session

Prints the agent workflow instructions, board counts, top ready tasks, Parent
Tasks whose SubTasks are all resolved (close candidates), and stale claims. The
Claude Code SessionStart hook runs `orbit prime --hook-json` automatically.

**Good fit:** the first command of an agent session, a status prompt, or a quick
health view of what is ready and what was abandoned.

```bash
orbit prime --by claude
orbit prime --by claude --repo api   # scope counts and the queue to one repository
orbit prime --json
```

`--repo` and repeatable `--label` scope the counts, ready tasks, close
candidates, and stale claims to a repository or label set. A close candidate is
a non-terminal Parent Task with at least one SubTask and no open SubTasks; Orbit
never closes it automatically. Task summaries in `--json` include a checkpoint
when one is recorded, so a stale claim carries its handoff context.

### `orbit export` — snapshot the board

Emits a deterministic JSON snapshot of every Task and SubTask, including terminal
work, with type, labels, dependency and parent edges, parent progress fields,
checkpoints, derived readiness, and comments.
The snapshot is ordered by task ID and contains no timestamps or machine-local
paths.

**Good fit:** feeding a knowledge base, archiving a board, or diffing two
snapshots. The collector should pin `schema_version`.

```bash
orbit export --json > board.json
orbit export --json | jq '.data.tasks[] | select(.kind == "subtask")'
orbit export --json | jq '.data.board'
```

The snapshot is the standard envelope with `data.schema_version`, `data.board`,
and `data.tasks`; each task has the same shape as `show --json`, plus comments.

## Claim and finish work

### `orbit claim` — take a specific task

Claims one task by ID instead of taking the top of the ready queue. The task must
be active and unclaimed (a stale claim needs `--steal`), not waiting on an
external blocker or an unfinished dependency, free of hold labels, and, if it is
a Parent Task, have no open SubTasks. The claim result includes any checkpoint
left by the previous holder.

**Good fit:** a human assigns a ticket, an agent resumes known work, or another
agent's lease expired and the task must be recovered.

```bash
orbit claim orb-a7b2c9 --by claude
orbit claim orb-a7b2c9 --by claude --steal   # take over a live or stale claim
```

`--steal` overrides the holder's lease, not the readiness rules: hold labels and
unfinished dependencies still block the claim.

### `orbit heartbeat` — keep a claim alive

Extends the lease from now. Only the holder can heartbeat. Heartbeats are
lease-only writes and are not committed until the next mutation or `orbit sync`.

**Good fit:** a long build or test run would outlive the default 30-minute lease,
or a supervisor wants to confirm an agent is still alive.

```bash
orbit heartbeat orb-a7b2c9 --by claude
```

### `orbit checkpoint` — hand off active work

Records or replaces the claim holder's latest checkpoint: current progress, the
next useful action, and any blocker. The first write requires `--progress`; later
writes can update only the fields that changed, and `--clear-next` or
`--clear-blocker` removes one. The caller must hold the claim. A stale lease still
counts until another agent takes the task over, so the outgoing holder can hand
off after a long run.

The checkpoint is visible in `show` (human and JSON), the JSON of `ready`,
`list`, and `prime`, the claim result, MCP `get`, and `export`. It survives
`release`, `claim --steal`, and `reopen`; it never changes readiness, claim
eligibility, or dependencies. Each write attributes the record to the current
claimant, even when a partial update keeps the previous progress text. On a done
or cancelled task the latest checkpoint is retained in the task file as archived
history, but new checkpoints are refused there.

**Good fit:** pausing work, handing a task to another agent, or leaving enough
context that a stale claim can be recovered without reading logs.

```bash
# Record progress, the next action, and a blocker
orbit checkpoint orb-a7b2c9 --by claude \
  --progress "Parser and tests pass" --next "Wire the CLI" \
  --blocker "Waiting on review"

# Update one field; omitted fields keep their previous value
orbit checkpoint orb-a7b2c9 --by claude --clear-blocker

# Leave a handoff, then requeue the task
orbit checkpoint orb-a7b2c9 --by claude --progress "Half done" --next "Finish the tests"
orbit release orb-a7b2c9 --by claude
```

### `orbit release` — hand work back

Clears the claim and returns the task to `todo`. The holder can release; `--force`
releases someone else's claim. Terminal tasks cannot be released. The latest
checkpoint is kept for the next claimant.

**Good fit:** work is deliberately handed back, a human takes over, or a claim was
taken by mistake and the queue should see the task again.

```bash
orbit release orb-a7b2c9 --by claude
orbit release orb-a7b2c9 --force
```

### `orbit done` — close completed work

Moves the task to `done`. Unfinished dependencies always block completion. A
Parent Task with open SubTasks is refused; `--force` cancels those SubTasks and
then closes the parent, and also overrides a competing live claim. `--comment`
records the completion note in the same transaction as the status change.

**Good fit:** the work is implemented and verified. This is the normal end of the
agent loop.

```bash
orbit done orb-a7b2c9 --by claude --comment "Implemented and verified."
orbit done orb-a7b2c9 --by claude --comment-file notes.md
orbit done orb-a7b2c9 --force --comment "Deferred the remaining pieces"
```

### `orbit cancel` — abandon work

Cancels a task without completing it. Cancelled tasks count as resolved, so
dependents can become ready. A live claim from another agent blocks cancellation
unless `--force` is passed. A Parent Task with open SubTasks is refused; `--force`
cancels those SubTasks too.

**Good fit:** the work is no longer needed, is a duplicate, or is a wontfix. Add
`--comment` to record the reason.

```bash
orbit cancel orb-a7b2c9 --by claude --comment "Superseded by orb-f14c02"
orbit cancel orb-a7b2c9 --force
```

### `orbit reopen` — undo a close

Returns a done or cancelled task to `todo`. Reopening a SubTask of a done Parent
Task reopens the parent too, so the feature is not left done with reopened work.
A SubTask under a cancelled parent is refused until the parent is reopened. It
also refuses when a dependent is already done, because that would leave completed
work with an unfinished dependency; `--force` allows it and `doctor` reports the
historical warning.

**Good fit:** a release regressed, a ticket was closed by mistake, or a cancelled
idea is back in scope.

```bash
orbit reopen orb-a7b2c9
orbit reopen orb-a7b2c9 --force
```

### `orbit delete` — remove a mistake

Permanently deletes a task and its comment thread. It refuses when dependents
exist, when the task has SubTasks, or when another agent holds a live claim;
`--force` also removes the ID from each dependent's dependency list and deletes
the whole SubTask subtree.

**Good fit:** duplicates or tickets created in the wrong Board. Prefer `cancel`
when the work should remain in history.

```bash
orbit delete orb-a7b2c9
orbit delete orb-a7b2c9 --force
```

## Maintain the board

### `orbit doctor` — health check

Reports the discovery trace, legacy layout, half-moved Board (`AMBIGUOUS_BOARD`), orphan worktree (`ORPHAN_WORKTREE`), storage mismatch (`STORAGE_MISMATCH`), and agent configs with an absolute Board path (`ABSOLUTE_BOARD_PATH`). Running without a Board is not itself a failure. When a Board resolves, it also checks malformed tasks, dangling dependencies, missing or nested parents, parent/ID mismatches, cycles, orphan or corrupt comments, stale temp files, uncommitted changes, a wrong checked-out branch, and a Claude hook that cannot find the running Orbit binary on `PATH`. Exits `1` when unhealthy.

`--fix` removes stale temp files and re-attaches an orphan `orbit` worktree whose `DIR` was deleted. `--migrate` moves a legacy hidden or `<workspace>/board` layout to the Orbit folder; it is explicit and resumable.

**Good fit:** after a crash, a merge, a board branch switch, a suspicious `NOT_FOUND`/`CORRUPT_TASK` error, or before adopting the new layout; also as a CI check.

```bash
orbit doctor
orbit doctor --fix        # temp files and an orphan worktree
orbit doctor --migrate    # move a legacy Board to .orbit
orbit doctor --json
```

### `orbit sync` — persist lease-only changes

Commits pending board changes, mostly heartbeats, to the board branch. Nothing is
pushed.

**Good fit:** a long session only heartbeated, or `doctor` reports uncommitted
changes.

```bash
orbit sync
```

### `orbit github` — GitHub Issue provider

Connects the current checkout's origin repository, adopts or creates Issues, and
runs bidirectional sync with conflict detection. `orbit github connect` checks
`gh` authentication and repository access. For an organization-owned repository,
it validates Issue Fields and writes `[github]` configuration. For a personal
repository, it leaves configuration unchanged and reports next steps; manually
configure `mode = "basic"` to use personal-repository sync. Existing integrations
can link repositories explicitly. Sync requires the `gh` CLI and `[github]` in
`config.toml`; full mode uses organization Issue Fields, while basic mode limits
Triage to labels. See [github-sync.md](./github-sync.md).

```bash
orbit github connect
orbit github repo link acme/widget
orbit github setup
orbit github adopt orb-a7b2c9 https://github.com/acme/widget/issues/42
orbit github adopt orb-a7b2c9 --repo acme/widget --number 42  # legacy form
orbit github create orb-a7b2c9                 # uses the only linked repository
orbit github sync orb-a7b2c9              # dry-run plan
orbit github sync orb-a7b2c9 --apply      # apply pull/push plan
```

### `orbit triage` — agent review and apply

Reviews GitHub-linked Tasks only. Agents draft a proposal JSON file, preview changes
with `review`, apply approved metadata to Orbit with `apply --approve`, then publish
with `orbit github sync`. Does not modify GitHub-owned title, body, or status. See
[agents/triage-workflow.md](./agents/triage-workflow.md).

```bash
orbit triage context orb-a7b2c9 --json
orbit triage review orb-a7b2c9 --proposal-file proposal.json --json
orbit triage apply orb-a7b2c9 --proposal-file proposal.json --approve --json
orbit github sync orb-a7b2c9 --direction push --apply
```

## Scenarios

### A solo maintainer triages a new report

```bash
orbit add "Login rejects valid tokens" -p 1 --label bug --label needs-triage --body-file report.md
# ... investigate, then decide:
orbit label add orb-a7b2c9 ready-for-agent
orbit label remove orb-a7b2c9 needs-triage
orbit comment add orb-a7b2c9 --by rodrigo --body "Confirmed; add a regression test."
```

### An agent takes one task and finishes it

```bash
orbit ready --claim --by claude --json      # receives one task plus lease
orbit show orb-a7b2c9 --json                # load body and comments
# ... implement ...
orbit heartbeat orb-a7b2c9 --by claude      # during a long verification run
orbit done orb-a7b2c9 --by claude --comment "Implemented and verified; tests added."
```

### Two agents work in parallel without collisions

```bash
# worker A
orbit ready --claim --by agent-a

# worker B, at the same time
orbit ready --claim --by agent-b
```

Each claim is atomic; when several tasks are ready, both workers get one. If two
workers target the same task, the loser gets `ALREADY_CLAIMED` with the holder and
lease.

### An agent crashes mid-task

```bash
orbit prime --json                     # lists stale claims
orbit claim orb-a7b2c9 --by agent-b --steal
orbit heartbeat orb-a7b2c9 --by agent-b
```

Use `release --force` instead when the task should go back to the queue unclaimed.

### A release train with ordered work

```bash
orbit add "Design the schema" -p 0 --label ready-for-agent
# created orb-f14c02  P0  Design the schema
orbit add "Ship the API" -p 1 --depends-on orb-f14c02 --label ready-for-agent
# created orb-a7b2c9  P1  Ship the API
orbit add "Publish the SDK" -p 1 --depends-on orb-a7b2c9 --label ready-for-agent
```

`orbit ready` returns only the first task. Completing it makes the next one ready
in the same order.

### A feature split into SubTasks

```bash
# the feature
orbit add "SSO login across api, web, infra" --body-file feature.md
# created orb-a7b2c9  P2  SSO login across api, web, infra

# one SubTask per repository
orbit add "api: OIDC callback" --parent orb-a7b2c9 --label repo:api
orbit add "web: login redirect" --parent orb-a7b2c9 --label repo:web
orbit add "infra: register client" --parent orb-a7b2c9 --label repo:infra

# an agent in the api repository picks its piece
orbit list --parent orb-a7b2c9 --label repo:api --json
orbit claim orb-a7b2c9.b3c4d5 --by api-agent

# when every piece is done or cancelled, close the feature explicitly
orbit done orb-a7b2c9 --by coordinator --comment "SSO shipped"
```

Each SubTask is claimed and completed independently. The Parent Task only becomes
Ready when the last one is done or cancelled. `orbit done --force` on the parent
cancels the remaining SubTasks instead of leaving them open.

### A Board with its own Task-ID prefix

```bash
orbit init --slug teramigo
orbit add "First task" --json
# teramigo-32fdfd
```

The slug is adopted once at creation. Existing `orb-` Boards keep working
unchanged; migration is optional. To change a populated Board, preview
`orbit slug migrate <slug> --dry-run`, then apply it. `init --slug` still only
changes an empty Board; a populated Board returns `SLUG_LOCKED` and points at the
migration command.

### A workspace of several repositories

```bash
# create the workspace and its shared Board
mkdir -p ~/src/acme && cd ~/src/acme
orbit init

# in each related repository
orbit link ~/src/acme/.orbit
```

All repositories and their worktrees share one dependency graph. The Board lives
at `~/src/acme/.orbit`; use `orbit --dir ~/src/acme/.orbit list` for a one-off
command against it regardless of the current repository.

### Collect the board into a knowledge base

```bash
orbit export --json > board.json
```

The snapshot includes every task, including done and cancelled work, with labels,
dependency and parent edges, comments, and derived readiness. It is ordered by
task ID and contains no timestamp, so the same board state produces the same
data. Pin `schema_version` in the collector.

### Move work from triage to an agent

```bash
orbit list --label needs-triage --json
orbit label add orb-a7b2c9 ready-for-agent
orbit label remove orb-a7b2c9 needs-triage
orbit ready --claim --by claude
```

Hold labels keep untriaged work out of the agent queue, so the second and third
commands are what actually publish the ticket.

### Recover after a crash or a bad merge

```bash
orbit doctor
orbit doctor --fix      # remove stale temp files and re-attach a deleted .orbit
orbit doctor --migrate  # move a legacy Board to .orbit
orbit sync              # commit anything left uncommitted
```

`doctor` never rewrites tasks or comments, so it is always safe to run.

## Common outcomes and errors

| Error | What to do |
| --- | --- |
| `NO_READY_TASKS` | Not an error: nothing is startable. Wait, triage, or remove a hold label. |
| `ALREADY_CLAIMED` | Another agent holds a live lease. Pick other work, or `--steal` to take it over. |
| `CLAIM_STALE` | The holder's lease expired. Recover with `orbit claim <id> --steal`. |
| `DEPENDENCY_BLOCKED` | Finish or cancel the IDs in `error.details.blocking`. |
| `WAITING_EXTERNAL` | Resolve the blocker, then `orbit update <id> --clear-waiting`. |
| `LABEL_BLOCKED` | Remove the hold label (`needs-triage`, `needs-info`, `ready-for-human`). |
| `NOT_CLAIM_HOLDER` | Use the holder's `--by`, or `--force` where the command allows it. |
| `SUBTASKS_OPEN` | Finish or cancel the IDs in `error.details.subtasks`, or pass `--force` to `done` or `cancel`. |
| `SUBTASKS_EXIST` | The Parent Task still has SubTasks. Use `delete --force` to remove the whole subtree, or delete the SubTasks first. |
| `PARENT_MISMATCH` | The SubTask ID and its `parent` field disagree. Rename the file or fix the frontmatter so the ID encodes the same parent. |
| `INVALID_SLUG` | `--slug` and `slug migrate` take a bare 1–20 character lowercase word with alphanumeric ends and no `--`; use the normalized suggestion in the error. |
| `SLUG_LOCKED` | The Board has tasks. Run `orbit slug migrate <slug>` to rewrite every ID, or keep the current prefix. |
| `MIGRATION_BLOCKED` | Migration refused because of live claims, an unhealthy Board, a dirty Board, or a concurrent Board move. Read `error.details.reason`; run `orbit doctor`, then retry with `--force` only for live claims. |
| `PREFIX_COLLISION` | Two old IDs would become the same new ID. No files were changed; choose another slug. |
| `PREFIX_MISMATCH` | `id.prefix` and the task filenames disagree. Restore the configured prefix first; migration refuses an inconsistent Board. |
| `DANGLING_PARENT` | Run `doctor`; the parent ID is missing. Recreate the parent or fix the SubTask's frontmatter. |
| `NESTED_SUBTASK` | A SubTask points to itself or to another SubTask. SubTasks are one level deep. |
| `INVALID_TYPE` | `epic` was used on a SubTask. Epic is top-level only; use `task`, `spike`, `chore`, or `decision` for a child. |
| `ALREADY_INITIALIZED` | A Board already resolves for this working tree. Use it, run `orbit unlink` to clear a link, move or migrate an ancestor/main/legacy Board, or pass a `DIR` outside every working tree. |
| `BOARD_NOT_FOUND` | Run `orbit init`, pass `--dir`/`ORBIT_DIR`, or `orbit link` an existing Board. |
| `CONFIG_ERROR` | The target `DIR` is not empty (apart from `.git`), or a configuration value is invalid. Use a fresh directory for a new standalone Board. |
| `LINK_NOT_FOUND` | The linked `DIR` is missing. Re-link it or restore the Board. |
| `LEGACY_LAYOUT` | The Board still uses the old hidden, `<workspace>/board`, legacy pointer, or explicit old-board-root layout. It works with a warning; run `orbit doctor --migrate` to move it. |
| `AMBIGUOUS_BOARD` | A new `.orbit` and a legacy Board both exist. Run `orbit doctor --migrate` to finish or resume the move. |
| `ORPHAN_WORKTREE` | `DIR` was deleted but the `orbit` branch remains. `orbit doctor --fix` or `orbit init` re-attaches it without task loss. |
| `STORAGE_MISMATCH` | `config.toml` says `worktree` or `repository`, but `DIR` is the other shape. Repair the worktree or run `doctor --migrate`. |
| `ABSOLUTE_BOARD_PATH` | A managed agent config still records a Board path. Re-run `orbit setup` to rewrite it path-free. |
| `LOCKED` | Retryable: another writer holds the lock. Retry after a short backoff. |

## See also

- [README](../README.md) for the overview, quick start, and workspace models.
- [Architecture](./ARCHITECTURE.md) for the component map, state machine, command flow, and sequence diagrams.
- [Specification](./SPEC.md) for the canonical JSON contract and transition rules.
- [Glossary](./GLOSSARY.md) for the canonical vocabulary.

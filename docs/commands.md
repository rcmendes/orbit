# Command guide

Status: this guide describes the current member-Repository model, Board host
and sync, Remote Issue catalog, scoped path-free agent setup, and Orbit folder
([ADR 0012](./adr/0012-orbit-folder.md), [ADR 0019](./adr/0019-board-folder-is-its-own-repository.md))
behavior. The unified `init [DIR]` surface, resolver, `orbit repo` membership
commands, and `orbit migrate` conversion are implemented.

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
| `--if-version <VERSION>` | Run an Issue mutation only when the Issue's `updated_at` still matches VERSION (the value `orbit show` reports); a stale version fails with retryable `VERSION_CONFLICT`. `orbit batch` takes a per-operation `if_version` instead. |
| `--idempotency-key <KEY>` | Make a write retry-safe: repeating the call with the same key returns the first recorded result (`meta.idempotent_replay: true`) instead of running again. |
| `--dir <PATH>` | Use a Board anchor or a specific Board directory for this command. |
| `ORBIT_DIR` | Environment equivalent of `--dir`. |
| `--no-sync` | Skip Remote Tracker auto-push and Board host auto-sync for this command (hidden from `--help`; still accepted). `ORBIT_NO_AUTO_SYNC=1` (or `ORBIT_NO_HOST_SYNC=1`) does the same for a process. Explicit `orbit remote sync` and `orbit board sync` are unaffected. |

Exit codes are `0` for success, `1` for a domain failure (for example
`DEPENDENCY_BLOCKED` or a failing `doctor`), and `2` for a usage error.

Issue commands resolve the Board in this order: `--dir`/`ORBIT_DIR`, repository
link pointer, nearest `.orbit` walking up across Git boundaries (restarted
from a worktree's main checkout), the retired in-Git store for migration
diagnostics, then nothing. An explicit override can name a Board anchor or a
Board directory. `orbit init` creates a Board at `<anchor>/.orbit/board` outside
Git and at `./.orbit/board` inside a repository. See
[issue-tracker.md](./agents/issue-tracker.md) for the multi-repository model.

## Which command do I need?

| Goal | Command |
| --- | --- |
| Set up a repository or Board anchor | `orbit init`, `orbit repo add`, `orbit repo remove` |
| Manage member Repositories | `orbit repo list`, `orbit repo refresh` |
| Convert a pre-ADR-0019 Board | `orbit migrate` |
| Convert a Board to the Issue model | `orbit migrate model` |
| File a new ticket | `orbit add` |
| Break work into blockers | `orbit depend`, `orbit undepend`, `orbit link` |
| Triage incoming work | `orbit label add/remove/list` |
| Find work for an agent | `orbit ready` |
| Take a specific ticket | `orbit claim` |
| Assign without a lease | `orbit assign` |
| Keep a lease alive | `orbit heartbeat` |
| Hand work back | `orbit release` |
| Finish or abandon work | `orbit done`, `orbit cancel` |
| Undo a close or remove a mistake | `orbit reopen`, `orbit delete` |
| Run a Board-defined workflow transition | `orbit transition` |
| Add context to a ticket | `orbit comment add`, `orbit update` |
| Attach a tool's own data to an Issue | `orbit property set/remove` |
| Apply many changes at once, all or none | `orbit batch` |
| Read the board | `orbit list`, `orbit search`, `orbit digest`, `orbit show`, `orbit tree`, `orbit prime` |
| Follow what changed on the Board | `orbit changes`, `orbit history` |
| Inspect the command surface | `orbit schema` |
| Collect or archive the board | `orbit export` |
| Wire an editor or agent | `orbit setup`, `orbit mcp` |
| Recover or maintain the board | `orbit doctor`, `orbit sync`, `orbit index` |
| Host, synchronize, or relocate the Board | `orbit board host`, `orbit board sync`, `orbit board move` |
| Link GitHub Issues (optional) | `orbit remote connect` — see [github-sync.md](./github-sync.md) |
| Triage Issues | `orbit triage` — see [agents/triage-workflow.md](./agents/triage-workflow.md) |

## Set up a Board

### `orbit init` — create the Board

Every Board is its own Git repository on branch `orbit` (ADR 0019); Board
history never lands in a code repository. Inside a Git repository, `init`
creates or reuses the hidden Board at `./.orbit/board`, with the rebuildable
cache beside it at `./.orbit/cache`. The whole `.orbit` folder is hidden
through `.git/info/exclude`, never `.gitignore`. A positional `DIR` that is
already a Git working-tree root, including `.`, means "initialize that
checkout" and uses its `./.orbit/board` (the main checkout's from a linked
worktree), so `orbit init .` is equivalent to `orbit init`. Any other `DIR`
inside a repository becomes a visible Board folder anchored by the
repository's `.orbit/board` pointer file. Outside Git, `orbit init` at an anchor folder creates the Board at
`<anchor>/.orbit/board` and reserves `<anchor>/.orbit/cache`; any other
directory there becomes a visible Board folder anchored by a sibling `.orbit`.
Member Repositories discover the Board automatically, including across their
Git boundary. Re-running at the same root is safe.

**Good fit:** the first time you use Orbit in a repository or initialize a
multi-repository Board.

```bash
orbit init                       # repo ./.orbit/board, or anchor ./.orbit/board
orbit init .                     # same as plain init inside a repository
orbit init --agents none         # Board only; no agent files
orbit init --agents all          # wire every supported harness
orbit init --print               # preview every file that would change
orbit init --portable-mcp        # MCP command `orbit`; editor PATH must find it
orbit init ~/src/acme            # anchor Board at ~/src/acme/.orbit/board
(cd ~ && orbit init)            # ordinary Board at ~/.orbit/board
orbit init --slug teramigo       # Issue IDs become teramigo-…
orbit init --visible             # visible Board folder ./issues instead of ./.orbit/board
orbit init --visible=tracker     # visible Board folder ./tracker
orbit init --host=../board-host  # also set the Board host (repo path or git URL)
```

**Core options**

| Option | Use it when | Example |
| --- | --- | --- |
| *none* | Start a repository-local Board in Git or an anchor Board outside Git. | `orbit init` |
| `DIR` | Choose a Board anchor, an existing repository root (initialize at its `./.orbit/board`), or an explicit Board directory — visible outside Git, or a visible folder inside a repository. Anchor Board files live at `<DIR>/.orbit/board`. | `orbit init ~/src/acme` |
| `--slug SLUG` | Choose the Issue-ID prefix instead of deriving it from `DIR`. Explicit input is strict; a populated Board returns `SLUG_LOCKED`. | `orbit init --slug acme` |
| `--agents LIST` | Wire agents non-interactively: `none`, `all`, `auto`, or a client list. | `orbit init --agents all` |
| `--print` | Preview the Board, exclusion entries, and generated agent files without writing or prompting. | `orbit init DIR --print` |
| `--visible[=NAME]` | Use a visible Board folder instead of the hidden `.orbit/board` layout. Bare `--visible` names it `issues`; `=NAME` picks the name. Applies when the target resolves to a repository or anchor root — an explicit directory is already visible. | `orbit init --visible=tracker` |
| `--host[=REPO\|URL]` | Set the Board host: a member Repository name/path or git URL whose remote stores the Board's `orbit` branch. Bare `--host` prompts in a terminal; `=REPO\|URL` sets it without prompting. An existing host branch with a different Slug is refused (`BOARD_HOST_CONFLICT`); a matching one is joined. | `orbit init --host=../board-host` |

**Choose by scenario**

- **New product repository:** run `orbit init` at the root. The Board is `./.orbit/board`, a standalone repository shared by linked worktrees, and IDs use the folder name.
- **Multi-repository Board:** run `orbit init ~/src/acme` or `orbit init` at its non-Git root. Repositories inside discover the Board automatically; repositories outside it can `orbit repo add`.
- **CI or scripted setup:** preview with `--print`, then use `--agents none` and an explicit `--slug` when the machine should not receive editor files and IDs must be stable.
- **Home Board:** run `orbit init` in `~` for an ordinary Board there. It is discovered normally, never created implicitly, and remains exempt from conflicting-Board checks. There is no `--global` flag.
- **Joining an existing Board:** use `orbit repo add DIR` from the repository that should share the Board; `init` is for creating or reusing the target `DIR`, not for joining another Board.
- **Sharing a Board across machines:** pass `--host` (or run `orbit board host` later) so the Board's `orbit` branch lives on one host repository; `orbit board sync` merges and pushes, and `board.host_sync = "auto"` syncs at `orbit prime` and after commits.
- **Migrating a pre-ADR-0019 Board:** use `orbit migrate`; `init` refuses legacy layouts with `BOARD_NOT_MIGRATED`. `orbit init` on an anchor that still contains child Boards previews and merges them in place, preserving Issue IDs and keeping the children as backups under `.orbit/backups/`.

**Supporting options**

| Option | Use it when | Example |
| --- | --- | --- |
| `--dir PATH` / `ORBIT_DIR` | Override discovery with a Board anchor or Board directory. Positional `DIR` wins for `init`. | `orbit --dir ~/src/acme init` |
| `--no-agents` | You want `--agents none` without the selector. | `orbit init --no-agents` |
| `--hooks` | You also want the Claude or Devin SessionStart hook. | `orbit init --agents claude --hooks` |
| `--portable-mcp` | MCP configs should call bare `orbit`; the editor's PATH must resolve it. | `orbit init --portable-mcp` |
| `--mcp-bin PATH` | Record a specific Orbit binary; conflicts with `--portable-mcp`. | `orbit init --mcp-bin /usr/local/bin/orbit` |

Every run ends with the same three-section report:

1. **Board** — whether it was created or reused, its path, branch, storage mode
   (`repository`: every Board is a standalone repository), Issue-ID prefix, and
   the `.git/info/exclude` entries for the Orbit folder and any visible Board
   folder.
2. **Coding agents** — the harnesses wired, the MCP command written and whether
   it is absolute or portable, plus every file created, updated, or already
   unchanged.
3. **Verify** — `orbit doctor`, followed by the next commands for the selected
   harnesses or, when none were wired, `orbit setup <client>`. Each next command
   carries a short explanation of what it does.

`--print` reports the same steps in future tense (`would create`, `would list`)
and adds the full generated file contents without writing anything. A standalone
Board always includes `orbit repo add DIR` in its next steps so member
repositories can join it.

On the first run that wires an MCP client, a terminal also asks whether MCP
configs should record an absolute binary path (the default) or the portable
`orbit` command. The absolute path works for GUI-launched editors without any
shell setup; the portable command only works when the editor's `PATH` resolves
`orbit`. Use `--portable-mcp` to choose it without a prompt. Once a selected
harness has an MCP config, later runs do not ask.

Issue IDs use the Board's slug as their prefix. An anchor Board uses its
anchor-folder name (`~/src/acme` → `acme-`); a repository Board uses its
repository name; a Board anchored at `~/.orbit` uses `orb` with a warning. Pass `--slug` to choose it
explicitly. Re-running `init` preserves the stored prefix; an empty Board can
change it, while a populated Board returns `SLUG_LOCKED` and points at
`orbit slug migrate <slug>`. `--print` reports the same result without writing.

Every Issue also gets a sequential **project key** — `PREFIX-N`, such as
`ORB-12` — allocated under the Board lock at creation and tracked in
`keys.toml`. The key is the handle humans and agents quote: `orbit show
ORB-12` (any case) resolves it everywhere an Issue ID is accepted, while the
immutable Issue ID stays the file name and the `id` in JSON. When Board sync
re-keys an Issue after a cross-clone collision, the old key moves to
`key_aliases` and keeps resolving. See [SPEC.md](./SPEC.md#project-key).
The JSON and human output carry `prefix`, `storage`, and the `exclude` entries;
the JSON also reports `global`, `created`, `preview`, `portable_mcp`, and
the managed files.

`init` takes its target from the positional argument, then `--dir`/`ORBIT_DIR`.
Without a target, it uses `./.orbit` in Git, a discovered Board anchor from a
member Repository, or the current directory as an anchor outside Git.
Running `init` in `~` creates an ordinary Board at `~/.orbit/board`. `repo add` uses
only its positional `DIR`; `--dir`/`ORBIT_DIR` never retargets the pointer. A
repository can own one local Board; a second `.orbit` in the same working tree
is refused. Member repositories inside an anchor use its Board without an
explicit add. A second local Board is reported as `AMBIGUOUS_BOARD`; creating
an anchor Board refuses to shadow an existing member Board or any unrecognized
`.orbit` entry. A legacy Board directly at `<anchor>/.orbit` is reported for
migration rather than moved or shadowed.

Without `--agents`, a terminal asks whether to wire detected harnesses; without
a terminal, `init` writes no agent files and explains how to wire them with
`orbit setup`. A standalone Board always includes its `orbit repo add` hint.
`--agents` accepts `none`, `all`, or a comma-separated client list. `init` refuses to create a
Board that would compete with an existing repository-local resolution for `DIR`:
a link, an ancestor `.orbit`, the main checkout's `.orbit`, or a legacy Board is
reported as `ALREADY_INITIALIZED` before anything is written. The
`~/.orbit` Board is exempt. `orbit repo remove` forgets an explicit
membership; move or migrate an ancestor, main-checkout, or legacy Board.

### A Board across repositories

A Board can span several Git repositories. Repositories inside the folder that
anchors the Board — a non-Git directory, or a repository root — are member
Repositories discovered automatically. Repositories outside it join
explicitly:

```bash
mkdir -p ~/src/acme && cd ~/src/acme
orbit init --slug acme                     # or: orbit init ~/src/acme

# A member repository resolves the same Board automatically.
cd ~/src/acme/api
orbit list

# A repository outside the anchor joins explicitly.
orbit repo add ~/src/acme/api --name api
```

The Board is at `~/src/acme/.orbit/board`; the rebuildable member inventory is
cached at `~/src/acme/.orbit/cache/member-repositories.json`. `orbit init`
does not clone repositories. When the anchor already contains child Boards —
repositories that anchored their own `.orbit` before the parent Board
existed — init previews the merge, preserves every Issue ID, records each
imported Issue's Repository, moves differing child ID prefixes to
`id.accepted_prefixes`, and keeps the child Boards on disk under
`<anchor>/.orbit/backups/`. Before importing, init reads and checks every child's
Issues, Comments, and merged prefix configuration. An Issue collision returns
`CONFIG_ERROR` before any imports are published or child Boards are relocated;
a newly created anchor Board may remain. Retrying accepts an unchanged Issue
already imported with its Repository attribution and preserves existing Comments.
An edited Issue still conflicts. Publication is atomic per file, so an I/O failure
or interruption can leave partial imports; fix the cause and rerun init.
An existing child Board or an unrecognized `.orbit` layout is reported instead of being shadowed or discarded. If a Board
already exists at the anchor, `orbit init <anchor>` reuses it.

#### `orbit repo` — manage member Repositories

```bash
orbit repo list [--json]                  # discovered and explicit members
orbit repo add PATH [--name NAME]         # join a repository explicitly
orbit repo remove NAME                    # forget an explicit membership
orbit repo refresh [--json]               # rebuild the member inventory
```

`orbit repo add PATH` writes the target repository's Orbit link pointer so
Board discovery resolves this Board from inside it, records explicit
membership, fills new Issues' `repository` field from the current Repository, and
refreshes managed agent configuration. `orbit repo remove NAME` drops
explicit membership and clears the link pointer when it points back to this
Board, then refreshes agent configuration again. Repositories discovered
below the anchor cannot be removed — their membership comes from the folder
layout. `orbit repo refresh` rescans the anchor, rebuilds
`member-repositories.json`, and reports which Repositories joined or left.

### `orbit slug` — inspect or migrate the Issue-ID identity

`slug show` reports the bare slug, the literal `id.prefix`, and the number of
Issues, sub-issues, and comment files. It is read-only. `slug migrate <SLUG>`
rewrites every Issue and sub-issue ID from the current prefix to `<SLUG>-`.

**Good fit:** an early Board was created under a repository or anchor name
that no longer fits. Migration is the explicit escape hatch from the one-prefix
rule; `init --slug` remains the creation and empty-Board path.

```bash
orbit slug show                        # orb, orb-, 42 issues, 17 sub-issues
orbit slug migrate atlas --dry-run     # print the plan; write nothing
orbit slug migrate atlas               # rewrite, validate, and commit once
orbit slug migrate atlas --force       # proceed despite live claims
```

The random suffix is preserved: `orb-a7b2c9` becomes `atlas-a7b2c9`, and
`orb-a7b2c9.d4b3df` becomes `atlas-a7b2c9.d4b3df`. Migration also rewrites
`parent` and internal `links` targets, renames comment files, updates the Orbit-generated
`# Comments: <id>` header, and changes `id.prefix`. It validates the result in a
temporary Git worktree, commits once as
`orbit: migrate issue prefix from orb- to atlas-`, then fast-forwards the Board.
Pending heartbeat writes are committed first, so the migration commit is the
single rollback point.

`--dry-run` reports counts, the reference count, and any live claims, but never
writes. The JSON plan also includes every `from`/`to` mapping and any collisions.
Applying with live claims requires `--force`; the affected Issue IDs and holders
are listed so they can re-read the Board. An unhealthy Board returns
`MIGRATION_BLOCKED` (run `orbit doctor` first), and a target that would collapse
two IDs returns `PREFIX_COLLISION`. On an empty Board migration is equivalent to
setting the prefix.

Issue bodies, comment text, checkpoint text, Git history, pull requests, and external links are
**not** rewritten. A body that mentions `orb-a7b2c9` keeps that text; old links
and cached agent IDs stop resolving. `--json` reports `current`, `target`,
`changed`, `issues`, `sub_issues`, `comment_files`, `references`, `mappings`,
`claims`, `collisions`, `dry_run`, `applied`, and `commit`. `slug show` reports
`slug`, `prefix`, `issues`, `sub_issues`, and `comment_files`.

### `orbit setup` — adjust one harness or apply a profile

Installs, checks, or removes a harness integration without touching the Board.
`--scope project` (the default) writes repository-level files; `--scope user`
writes the harness's user-level locations. `--print`, `--check`, and `--remove`
operate on the selected scope. Generated configs record the Orbit binary but not
the Board path, so moving the Orbit folder does not stale them.

A target that is not an agent integration resolves as a **profile** — a
declarative TOML document (`profiles/<name>.toml` shipped with Orbit,
`<board>/profiles/`, `<orbit>/profiles/`, `<repo>/profiles/`,
`~/.config/orbit/profiles/`, `./profiles/`, or an explicit `./name.toml` path)
that maps a plugin's vocabulary onto the Board: `requires`/`optional`
capabilities, a minimum `contract`, a `[workflow]` template or custom
definition, `holds` merged into `non_ready_labels`, `[roles]`, `[vocabulary]`
maps, and optional `[board]` overrides. `orbit setup <plugin>` negotiates the
requirements first — an unmet profile fails with `PROFILE_UNMET` naming the
missing capabilities — then applies the profile to `config.toml` idempotently
and records it under `[[profiles]]` for `doctor` and `capabilities`. A profile
sharing an integration's name (for example `mattpocock`, which ships as a
built-in profile mapping the five triage roles onto the Issue workflow)
applies alongside the harness files. `--check` reports `installed`/`stale`/`unmet`; `--print` previews
the resulting `config.toml`; `--remove` refuses a profile-only target — profiles
are declarative, so remove the `[[profiles]]` record and its settings by hand
or apply a different profile.

User scope writes Claude memory to `~/.claude/CLAUDE.md`, its hook to
`~/.claude/settings.json`, and MCP to `~/.claude.json`; Cursor rules, the shared
skill, and MCP to `~/.cursor/rules/orbit.mdc`, `~/.agents/skills/orbit/SKILL.md`,
and `~/.cursor/mcp.json`; Codex to `~/.codex/AGENTS.md`, the shared skill, and
`~/.codex/config.toml`; OpenCode to `~/.config/opencode/AGENTS.md` and
`~/.config/opencode/opencode.json`; Devin to `~/.devin/rules/orbit.md`,
`~/.config/devin/skills/orbit/SKILL.md`, `~/.config/devin/mcp_config.json`,
and the SessionStart hook in `~/.config/devin/config.json`; and the mattpocock
tracker bridge to
`~/.agents/issue-tracker.md` and `~/.agents/triage-labels.md`. `--print` names
each exact file.

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
orbit setup acme               # apply the acme profile to the Board
orbit setup acme --check       # installed / stale / unmet
orbit setup acme --print       # preview the resulting config.toml
```

**Core options**

| Option | Use it when | Example |
| --- | --- | --- |
| `--scope project` | Default. Write instruction and MCP files for this repository. | `orbit setup claude` |
| `--scope user` | One machine-wide integration; writes the harness's user-level locations. | `orbit setup cursor --scope user` |
| `--check` | CI or a repair check: report installed, missing, or stale and exit non-zero on drift. | `orbit setup all --check` |
| `--remove` | Remove only Orbit-managed files, sections, hooks, and MCP entries. Refuses a profile-only target; a profile riding along with a harness removal stays applied. | `orbit setup all --remove` |
| `--no-mcp` | Write instruction and skill files without MCP entries. | `orbit setup codex --no-mcp` |
| `--hooks` | Add the Claude or Devin SessionStart hook. | `orbit setup claude --hooks` |
| `--portable-mcp` | Write bare `orbit` in MCP configs; the editor's PATH must resolve it. | `orbit setup opencode --portable-mcp` |
| `--print` | Preview the exact files that would be written or removed. | `orbit setup all --print` |

Existing `docs/agents/issue-tracker.md` or `docs/agents/triage-labels.md` files
without Orbit's managed marker are treated as hand-written guides: setup reports
them as `skipped` and leaves them unchanged. Tracker files with the marker
update and remove normally.

### `orbit mcp` — expose the Board to an MCP client

Runs the Model Context Protocol server over stdio with 14 tools and the
`orbit://tasks/{id}` resource. Editors launch it from their MCP configuration.
`create` accepts `parent`, `type`, `component`, `external_id`, `verification`,
and `body`; `update` accepts `type`, `component`, `repository`, `external_id`,
and `verification`; `checkpoint` records the assignee's handoff; `list` and
`ready` accept `parent`, `repository`, `components`, `exclude_components`,
`labels`, `exclude_labels`, `types`, and `exclude_types` to scope a feature,
repository, or component.

**Good fit:** an editor or agent that speaks MCP should read and update the Board
directly instead of shelling out to the CLI.

```bash
orbit mcp                          # normally launched by the editor
  orbit --dir ~/src/acme/.orbit mcp    # serve a specific Board
```

## Create and shape work

### `orbit add` — file a ticket

Creates an Issue in the workflow's initial status (`ready` in the shipped templates) and prints its ID and project key, which you need for dependents,
labels, and comments.

**Good fit:** a bug report, a feature, a follow-up from a review, or a ticket
that a planning skill is publishing to the tracker. Put context and acceptance
criteria in the body; use labels for triage state.

```bash
# A high-priority bug with a Markdown body
orbit add "Login rejects valid tokens" -p high --label bug --body-file ticket.md

# Work that must wait for a blocker
orbit add "Publish the release" -p high
orbit depend orb-b3d4e5 orb-a7b2c9    # the new ticket waits for orb-a7b2c9

# A ticket waiting on something outside the Board
orbit add "Upgrade the parser" --body-file - <<'MD'
Depends on upstream 2.4.
MD
orbit link add orb-c5e6f7 --type waiting-on --external "external:upstream-2.4"

# A top-level epic or spec
orbit add "Spec: SSO login" --type epic --body-file spec.md

# Publish several tickets, blockers first; each add prints the new ID
orbit add "Design the schema" -p highest --label ready-for-agent
# created orb-f14c02  highest  Design the schema
orbit add "Ship the API" -p high --label ready-for-agent
orbit depend orb-a7b2c9 orb-f14c02
# created orb-a7b2c9  high  Ship the API

# Make a scripted add safe to retry: a second add with the same Source Key
# returns the first Issue (meta.matched: true) instead of creating another
orbit add "Step 3" --source-key planner:plan-7:step-3

# Name the reporter explicitly; it defaults to ORBIT_AGENT, then USER
orbit add "Filed by ops" --reporter alice

# Break a feature into a sub-issue (run inside the api member Repository, or set
# it afterwards with `orbit update <id> --repository api`)
orbit add "API: add the endpoint" --parent orb-a7b2c9
# created orb-x7k2pq  medium  API: add the endpoint  (sub-issue of orb-a7b2c9)

`--reporter` records who created the Issue. It defaults to `ORBIT_AGENT`, then
`USER`, and is never changed afterwards.

`--parent` creates a sub-issue under an existing Parent Issue. The `parent` field
stores the edge; IDs are flat and never encode the parent. The Board's
`board.hierarchy` decides which types may parent which (default `epic` >
`story`/`task`/`bug` > `sub-task`), and a child can still have real
dependencies. The Parent Issue becomes Ready only when every descendant is
done or cancelled.

`--type` sets the Issue type from the Board's `board.types` vocabulary
(`epic`, `story`, `task`, `bug`, `sub-task` by default; `board.default_type`
picks the default). The type decides where in the hierarchy the Issue may sit.
`bug` and `enhancement` remain labels, not Types.

`-p`/`--priority` takes a Priority name from the Board's `board.priorities`
list — `highest`, `high`, `medium`, `low` by default, ordered most important
first. Issues store the name and sort by its position, so a Board can rename or
reorder priorities in `config.toml`; the default comes from
`board.default_priority`.

A spec or epic is a top-level Issue with `type: epic`; its tickets are sub-issues.
This is also the compatibility mapping for Beads and Matt Pocock's skills:
**Issue** = Issue, **Sub-issue** = sub-issue, **Epic** = top-level Issue with
`type: epic`. Put the spec in the parent body, publish tickets with `--parent`,
and use `orbit ready --parent <id>` as the frontier. The hierarchy depth is
whatever `board.hierarchy` configures; the default reaches `epic` > working
types > `sub-task`. To nest deeper, extend `board.types` and
`board.hierarchy` in `config.toml`.
Never
use `done --force` or `cancel --force` on a spec parent with open tickets: it
cancels them. See [ADR 0009](./adr/0009-keep-issue-vocabulary.md) and
[ADR 0010](./adr/0010-structural-type-field.md).

`--component` sets an optional Component slug (`cli`, `payments`, `mobile-api`,
...). Each Board may restrict values through `board.components` in `config.toml`;
when the list is empty any short lowercase slug is accepted. `--repository`
sets the owning member Repository (filled automatically inside one) and
`--external-id` records the Issue's identity in an external system, unique per
Board. Filter with `orbit list --component cli` or
`orbit ready --exclude-component docs`; `--repository NAME` filters on the
`repository` field.
`--verification` stores concise checks or
commands for agents; shared project defaults live in
[`docs/agents/project-guidance.md`](./agents/project-guidance.md).

### `orbit update` — edit a ticket in place

Changes the title, body, priority, type, component, repository, external id, verification, or external blocker
without touching status, assignments, labels, or dependencies. `--type` must
keep the Issue's parent and children edges valid under `board.hierarchy`.
**Good fit:** scope changed after triage, a spec needs refinement, or an external
blocker was resolved.

```bash
orbit update orb-a7b2c9 --priority highest
orbit update orb-a7b2c9 --type bug
orbit update orb-a7b2c9 --body-file refined-spec.md
orbit update orb-a7b2c9 --clear-waiting
orbit update orb-a7b2c9 --component mcp --verification "cargo test --test mcp"
orbit update orb-a7b2c9 --clear-component --clear-verification
```

### `orbit link` — typed relations and external references

A Link is a typed relation stored on an Issue: a `type` (`blocks`,
`relates-to`, `duplicates`, `discovered-from`, or a custom slug), a `target`
(an Orbit Issue ID or an external `SYSTEM:ID` reference such as
`jira:PROJ-123`), an optional `blocking` override, and arbitrary
`--attribute KEY=VALUE` pairs. `blocks` links block readiness by default;
every other type is non-blocking unless `--blocking` is passed, and
`--no-blocking` overrides `blocks`. Only blocking links drive readiness and
are checked for cycles, so non-blocking links can form cycles freely.

**Good fit:** recording that an issue duplicates or was discovered from
another, relating issues without blocking, or parking work on an external
tracker (`--external jira:PROJ-123 --url …`) that blocks readiness until the
link is removed. `orbit depend`/`undepend` remain shorthand for a blocking
`blocks` link between two issues.

```bash
orbit link add orb-a7b2c9 --type relates-to --to orb-f14c02
orbit link add orb-a7b2c9 --type blocks --external jira:PROJ-123 \
  --url https://jira.example.com/browse/PROJ-123
orbit link add orb-a7b2c9 --type duplicates --to orb-f14c02 \
  --attribute reason="same root cause"
orbit link list orb-a7b2c9
orbit link remove orb-a7b2c9 --type relates-to --to orb-f14c02
```

### `orbit depend` and `orbit undepend` — connect work

`depend <id> <dep-id>` adds a blocking `blocks` link: the first Issue waits
until the second is done or cancelled. Cycles through blocking links,
self-links, linking a terminal issue, and parent/child edges in either
direction are rejected. `undepend` removes the blocking link and is
idempotent; non-blocking links are untouched — use `orbit link remove` for
those.

**Good fit:** a blocker is discovered after both tickets exist, or a blocking
dependency was wrong and must be removed. Use `orbit add --parent` for
parentage; a dependency blocks the child, while a parent blocks the feature.

```bash
orbit depend orb-a7b2c9 orb-f14c02   # orb-a7b2c9 waits for orb-f14c02
orbit undepend orb-a7b2c9 orb-f14c02
```

Add the edge with `depend` (or `link add --type blocks`) once both Issues exist.

### `orbit label` — triage state and category

Labels are free-form lowercase slugs. `needs-triage`, `needs-info`, and
`ready-for-human` are hold labels: while present, the Issue is not ready and
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

### `orbit comment` — keep a per-Issue thread

Comments are append-only, with an author and timestamp, and are allowed on closed
issues. Corrections are new comments; nothing is edited or deleted.

**Good fit:** triage notes, handoff context, links to logs, or a record of why a
decision was made. Put durable requirements in the body instead.

```bash
orbit comment add orb-a7b2c9 --by claude --body "Reproduced on macOS."
cargo test 2>&1 | orbit comment add orb-a7b2c9 --by claude --body-file -
orbit comment list orb-a7b2c9
```

### `orbit property` — attach tool-owned data

A Property is a JSON object that one tool keeps on an Issue under its own name,
the owner — the property's namespace (a lowercase slug such as `ci`). Orbit
stores and shows it but never reads it: Properties do not affect readiness,
assignments, or status, and they are never synchronized with a Remote Tracker.
`set` replaces the owner's whole object (at most 8192 bytes); `remove` deletes
it. Other owners are untouched.

**Good fit:** a CI result, another tool's bookkeeping, or a link that no Remote
Tracker covers. Put prose for people in comments or the body instead.

```bash
orbit property set orb-a7b2c9 ci --value '{"passed":true,"run":412}'
orbit show orb-a7b2c9 --json | jq '.data.properties'
orbit property remove orb-a7b2c9 ci
```

The MCP tool `property` takes `id`, `owner`, and `value` (an object, or null to
remove).

### `orbit batch` — apply many changes at once

A Batch is a JSON list of operations applied under one lock and published as one
commit. Either every operation applies or none does: a failure writes nothing
and names the failing operation's `index` and `op` in `error.details`.
Operations are `add`, `update`, `label`, `depend`, `undepend`, `link`,
`unlink`, `claim`, `assign`, `heartbeat`, `release`, `done`, `cancel`, `reopen`,
`transition`, `comment`, and `property`,
each with its command's fields (see [SPEC.md](./SPEC.md#batch)). A reference is a
Issue ID, a project key or alias, or `key:` plus a Source Key, so later operations can point at Issues the
same Batch creates.

**Good fit:** filing a whole plan (an epic, its sub-issues, and their
dependencies) in one commit; reserving a held issue for one agent by removing the
hold and claiming in one step; any script that must not leave half its changes
behind. Re-running the same Batch after a crash finishes it, except that
`comment` operations append again and an operation a later one invalidated
(such as `claim` before `done` of the same Issue) fails; see SPEC.md.

```bash
cat > plan.json <<'JSON'
{"operations": [
  {"op": "add", "key": "planner:plan-7:epic", "title": "Plan 7", "type": "epic"},
  {"op": "add", "key": "planner:plan-7:step-1", "title": "Step 1", "parent": "key:planner:plan-7:epic"},
  {"op": "add", "key": "planner:plan-7:step-2", "title": "Step 2", "parent": "key:planner:plan-7:epic"},
  {"op": "depend", "id": "key:planner:plan-7:step-2", "on": "key:planner:plan-7:step-1"}
]}
JSON
orbit batch --file plan.json --dry-run
orbit batch --file plan.json --json | jq '.data.keys'
```

## Find work

### `orbit list` — inspect the board

Lists issues with combinable filters. Done and cancelled issues are hidden by
default; pass `--include-closed`, or name a terminal status with `--status`, to
include them. Inside a member Repository or one of its worktrees the list is
scoped to that Repository's Issues plus Board-wide Issues (issues with no `repository`);
`--all` widens it to the whole Board, and at the Board's root there is no
scoping. Human output is an aligned table whose `ID` column shows the project
key (`ORB-12`; the Issue ID for Issues that predate keys), with `STATUS`, `P`, `TYPE`,
`TITLE`, `LABELS`, and `CLAIMED BY` columns; titles longer than 60 characters
are shortened with an ellipsis. When the Board has remote material — a
configured Remote Project or any Issue carrying a Remote link — an `ISSUE`
column appears between `TYPE` and `TITLE`; it is `-` for a Local Issue, `#N`
for a Linked Issue, and `~#N` for detached provenance, and a local-only Board
omits the column entirely. Root Issues are always ordered by
priority, then creation time and ID; each Parent Issue is followed by its
sub-issues, also ordered by priority. `--group` draws that hierarchy as one
outlined table: a banner for each Parent Issue, its sub-issues underneath, and a
final `No parent` section. Use `--json` when a script or agent consumes the
result.

**Good fit:** a human or agent wants an overview, a triage queue, one agent's
in-progress work, or the full history.

```bash
orbit list
orbit list --linked                               # only Issues linked to a Remote Issue
orbit list --local                                # only Issues without an active link
orbit list --label needs-triage
orbit list --label sso --label ready-for-agent       # every label must match
orbit list --repository api                          # repository field api
orbit list --status in_progress --assignee claude
orbit list --status done            # closed work, without --include-closed
orbit list --unlabeled
orbit list --parent orb-a7b2c9      # only this feature's sub-issues
orbit list --type epic              # only epics and specs
orbit list --group                  # parents and sub-issues in one outlined table
orbit list --exclude-type epic      # hide spec/epic containers
orbit list --include-closed --json
orbit list --all --json             # every Repository, from inside a member
```

Human output with `--group` looks like this:

```text
╭───────────────────┬────────┬──────────┬──────┬───────┬─────────────────────────────┬─────────────────────────────┬────────────╮
│ ID                │ STATUS │ PRIORITY │ TYPE │ ISSUE │ TITLE                       │ LABELS                      │ CLAIMED BY │
├─────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────┤
│ orb-a7b2c9  Spec: SSO login across api, web, infra                                   epic · todo · high · ready-for-agent │
├─────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────┤
│ orb-a7b2c9.z21c2w │ todo   │ highest  │ task │ #12   │ infra: register OIDC client │ ready-for-agent │            │
│ orb-a7b2c9.hxdmqt │ todo   │ high     │ task │ -     │ api: OIDC callback          │ ready-for-agent   │            │
│ orb-a7b2c9.c5qmed │ todo   │ medium   │ task │ -     │ web: login redirect         │ ready-for-agent   │            │
╰───────────────────┴────────┴──────────┴──────┴───────┴─────────────────────────────┴─────────────────────────────┴────────────╯
```

The table is intentionally not a machine interface. Column widths grow with the
data, labels are joined with commas, an assignee appears in the last column,
and long titles are truncated. Parse `--json` instead.

`--linked` keeps only Issues with an active Remote Issue link; `--local` keeps
only Issues without one, including detached provenance. They are mutually
exclusive. The `ISSUE` column and both flags appear in human output and
`--help` only when the Board has remote material; on a local-only Board they
are hidden but still accepted. `--type` is repeatable and matches any listed work shape; `--exclude-type` hides
any listed shape. `--label` is repeatable and every label must match;
`--exclude-label` is repeatable and hides an Issue carrying any of the named labels.
`--repository NAME` matches the Issue's `repository` field case-insensitively;
`--component` and `--exclude-component` filter on the `component` field.
Explicit `--label` filters remain additional requirements. This applies to
`list`, `ready` (including `--claim`), and `prime`. Prime reports the Repository
filter separately as `repository` in JSON and `repository:` in human output; its
`labels` contains only explicit label filters. Parent groups follow the
parent's priority; sub-issues inside a group follow their own priority. Issues with
no parent appear in the final `No parent` section. JSON always carries `type`,
`parent`, `sub_issues`, and `open_sub_issues` with the full title. A Parent
Issue with open sub-issues is not Ready, so `list` may show it even though `ready`
does not.

`--limit N` caps the page and `--cursor CURSOR` resumes from a previous page's
`data.next_cursor`; ordering is fixed per query, so inserts or updates between
pages do not shift later results. `--filter JSON` applies an object of
summary-field predicates (AND): scalars match by equality or membership in an
expected array, arrays match by containment, and unknown field names fail with
`INVALID_ARGUMENT`. `--fields FIELDS` projects a comma-separated field list —
dotted paths reach into objects — and unknown names fail the same way.

For work that can start now, use `orbit ready` instead: `list` shows all open
work, including blocked issues.

### `orbit search` — qualifier search

Searches issues by free text plus `key:value` qualifiers. Every term must
match: free-text terms hit the title, labels, and body; `status:`,
`assignee:`, `label:`, `type:`, `priority:`, `component:`, and `repository:`
qualifiers filter on the matching summary field. Scalar qualifiers are
repeatable with any-of semantics and every `label:` must be present. Each
result carries a `snippet` quoting the match. The same scoping, `--include-closed`, `--filter`, `--fields`,
`--limit`, and `--cursor` rules as `list` apply; inside a member Repository the
search is scoped like `list` unless `--all` widens it to the whole Board.

**Good fit:** finding work by content or by a combination of metadata that the
`list` flags do not express, and paging through hits without repeats.

```bash
orbit search "status:ready label:backend login"
orbit search "assignee:claude type:bug" --json
orbit search "oidc" --limit 10 --cursor "$NEXT_CURSOR" --json
```

### `orbit digest` — compact issue digest

Prints a compact digest of one Issue: status, status category, readiness,
assignee, reporter, labels, open blockers, checkpoint, and comment count. When
the Issue has descendants the digest adds a `rollup` over the subtree —
`total`, `done`, `cancelled`, `blocked`, `open`, `by_status`, plus a
direct-children `sub_issues` summary — so an epic or spec reports progress and
blocked counts at a glance. Leaf Issues report no rollup.

**Good fit:** a quick health read on one ticket or a rollup over an epic
without loading every child.

```bash
orbit digest orb-a7b2c9
orbit digest orb-a7b2c9 --json
```

### `orbit ready` — the agent queue

Shows issues sitting in the workflow's ready status, unassigned, not waiting on an
external blocker, carrying no hold label, and with every dependency and sub-issue
in a closed status category. A Parent Issue is not Ready while any sub-issue is
open. Results are ordered by priority, then creation
time.

With `--claim`, the first Issue is claimed atomically in the same command and
moves to `in_progress` with a lease. When nothing is ready, the command succeeds
with `data: null` and `meta.reason: "NO_READY_TASKS"`. `--parent` scopes the
queue to one feature's sub-issues.

Inside a member Repository or one of its worktrees the queue is scoped to that
Repository's Issues plus Board-wide Issues; `--all` widens it to the whole Board,
and `--claim` only ever takes an Issue inside the scope.

**Good fit:** an agent asks what to do next, a supervisor polls for idle work, or
a worker wants to take exactly one issue without racing another agent.

```bash
orbit ready --limit 5
orbit ready --claim --by claude --json
orbit ready --parent orb-a7b2c9 --claim --by claude   # one feature only
orbit ready --repository api --claim --by claude          # one repository only
orbit ready --exclude-type epic --claim --by claude   # skip spec containers
orbit ready --json              # NO_READY_TASKS means wait and retry
```

`--parent`, `--repository`, `--component`, `--exclude-component`, repeatable
`--label`, `--type`, and `--exclude-label` combine with AND. `--type` matches any listed shape; `--exclude-type` hides any
listed shape. `--exclude-label` remains for legacy label-only boards. A released
Issue can still carry the previous assignee's checkpoint; `--json` includes it so
the next claim starts with the handoff context.

#### Scheduler strategies (opt-in)

The default queue order — priority, then creation time, then ID — is unchanged.
A Board opts into a different order or extra claim rules with `--strategy`, a
comma-separated list applied in order, or persistently with `scheduler.strategy`
in `config.toml` (the flag wins for one invocation):

```toml
[scheduler]
strategy = "critical-path,touches"
wip_limit = 3

[scheduler.capabilities]
devin = ["rust", "docs"]
```

- `default` keeps the kernel order.
- `critical-path` sorts candidates by the longest chain of non-terminal
  dependencies that transitively wait on them, so work that unblocks the most
  downstream effort is claimed first.
- `touches` avoids file-touch overlap. An Issue declares the paths it expects
  to modify with `orbit property set <id> touches --value '{"paths": ["src/auth"]}'`.
  On `--claim`, a candidate is refused when its paths overlap — equal, or one
  a `/`-separated prefix of the other — the paths of a non-terminal Issue
  assigned to a different assignee, so two agents never receive overlapping
  touches. On a plain `ready` listing the safe candidates simply sort first.
- `capability` prefers Issues whose `capabilities` Property requirements
  (`orbit property set <id> capabilities --value '{"requires": ["rust"]}'`)
  are a subset of the claiming agent's `[scheduler.capabilities]` entry. It is
  a preference, not a refusal; Issues requiring nothing match everyone.
- `wip` refuses `--claim` when the agent already holds `scheduler.wip_limit`
  non-terminal assigned Issues; the result is `data: null` with
  `meta.reason: "WIP_LIMIT"`. Listing is unaffected.

An unknown strategy name fails with `INVALID_ARGUMENT`; a `[scheduler]` section
that fails validation (unknown strategy, `wip` without `wip_limit`, a zero
limit) fails Board load with `CONFIG_ERROR`.

### `orbit show` — load one ticket

Prints the full Issue: body, labels, links, type, parent, sub-issues,
dependents, assignment state, checkpoint, and comments. A `github:`/`remote:` line
reports the Issue's Remote Issue link only when the Board has remote material
or the Issue itself carries one; a connected Board shows `github: -` for an
unlinked Issue. `--json` is the form an agent
should load before starting work. A terminal issue keeps its latest checkpoint as
archived history, marked by `checkpoint_archived: true`. `--fields FIELDS`
projects a comma-separated field list — dotted paths reach into objects — onto
the detail object before it prints; unknown names fail with
`INVALID_ARGUMENT`.

**Good fit:** before implementing a ticket, when checking why an issue is blocked,
or when reading a handoff checkpoint or comment.

```bash
orbit show orb-a7b2c9
orbit show orb-a7b2c9 --json
orbit show orb-a7b2c9 --fields id,key,status,labels --json
```

### `orbit tree` — inspect one epic or spec

Shows one Issue, its sub-issues at every level, and their blocking edges. Unlike
`list --group` (the whole Board) and `list --parent` (children only), `tree`
always includes the root and its family, with resolved/total progress on the
root. The root may be any Issue with sub-issues, including a sub-issue that has its
own; a sub-issue without sub-issues returns `INVALID_ARGUMENT`.

**Good fit:** inspecting a spec, epic, or feature before publishing tickets,
checking the frontier, or seeing why one ticket is blocked.

```bash
orbit tree orb-a7b2c9
orbit tree orb-a7b2c9 --json
```

`--json` returns `data.root`, `data.issues` with the root first and descendants
depth-first, and `data.count`.

### `orbit prime` — starting context for a session

Prints the agent workflow instructions, board counts, top ready Issues, Parent
Issues whose sub-issues are all resolved (close candidates), and stale claims. The
Claude and Devin SessionStart hooks run `orbit prime --hook-json` automatically.

**Good fit:** the first command of an agent session, a status prompt, or a quick
health view of what is ready and what was abandoned.

```bash
orbit prime --by claude
orbit prime --by claude --repository api # scope counts and the queue to one repository
orbit prime --json
```

`--repository` and repeatable `--label` scope the counts, ready Issues, close
candidates, and stale claims to a repository or label set. A close candidate is
a non-terminal Parent Issue with at least one sub-issue and no open sub-issues; Orbit
never closes it automatically. Issue summaries in `--json` include a checkpoint
when one is recorded, so a stale claim carries its handoff context.
`--max-tokens N` caps the rendered context at roughly N tokens (~4 characters
each): issue listings are trimmed first, and when the fixed instructions alone
exceed the budget the human output is truncated and the JSON `budget` object
reports `truncated: true`. `--hook-json` honors the same budget.

### `orbit export` — snapshot the board

Emits a deterministic JSON snapshot of every Issue and sub-issue, including terminal
work, with type, labels, dependency and parent edges, parent progress fields,
checkpoints, derived readiness, and comments.
The snapshot is ordered by issue ID and contains no timestamps or machine-local
paths.

**Good fit:** feeding a knowledge base, archiving a board, or diffing two
snapshots. The collector should pin `schema_version`.

```bash
orbit export --json > board.json
orbit export --json | jq '.data.issues[] | select(.type == "sub-issue")'
orbit export --json | jq '.data.board'
```

The snapshot is the standard envelope with `data.schema_version`, `data.board`,
and `data.issues`; each Issue has the same shape as `show --json`, plus comments.

### `orbit changes` and `orbit history` — the change feed

`orbit changes` lists the named events derived from the Board's Git history —
`issue.created`, `issue.transitioned`, `issue.assigned`, `issue.checkpointed`,
`issue.commented`, `issue.updated`, `issue.deleted`, `board.changed` — oldest
first. Each event carries its kind, the Issue it touched, the actor, a trace id
(the producing commit), and the commit. The response's `cursor` is the Board
head commit: pass it back as `--since` to fetch only newer events. A cursor
that is not an ancestor of the head is rejected rather than skipping or
repeating events. `--type` filters by kind (repeatable) and `--limit` keeps
the oldest events (whole commits) and moves `cursor` to the last one returned, so
paging forward never skips events; `truncated` reports a cut.

`orbit history <id>` shows one Issue's timeline, matched by Issue ID or project
key; it keeps working after the Issue is deleted because the timeline comes
from Git, not the live Board.

**Good fit:** polling for Board changes, auditing who did what, or reconstructing
an Issue's story without reading Git by hand.

```bash
orbit changes --json
orbit changes --since 4f2c9ab --type issue.transitioned --json
orbit history orb-a7b2c9 --json
orbit history ORB-12 --limit 10
```

`changes --json` returns `data.events`, `data.cursor`, and `data.count`;
`history --json` returns `data.issue`, `data.events`, and `data.count`. Both
are exposed as the read-only, idempotent MCP tools `changes` and `history`.

## Claim and finish work

### `orbit claim` — take a specific Issue

Claims one issue by ID instead of taking the top of the ready queue. The issue must
be active and unassigned (a stale lease needs `--steal`), not waiting on an
external blocker or an unfinished dependency, free of hold labels, and, if it is
a Parent Issue, have no open sub-issues. A claim sets the assignee — a human or an
agent, recorded with `--kind` (`agent` by default) — and a fresh lease together.
The claim result includes any checkpoint left by the previous assignee. A claim
by the current assignee renews its own lease, live or stale, so repeating a
claim is safe.

**Good fit:** an agent takes work it will heartbeat and finish, a human assigns a
ticket to an agent, or another agent's lease expired and the issue must be
recovered.

```bash
orbit claim orb-a7b2c9 --by claude
orbit claim orb-a7b2c9 --by claude --kind human   # a person claims with a lease
orbit claim orb-a7b2c9 --by claude --steal   # take over a live or stale lease
orbit claim orb-a7b2c9 --by claude --worktree # claim and create the issue worktree
```

`--steal` overrides the assignee's lease, not the readiness rules: hold labels and
unfinished dependencies still block the claim.

`--worktree` claims and creates the Issue's worktree in one step:
`.worktrees/<id>` on branch `ticket/<id>` inside the Issue's Repository (its
`repository` field), falling back to the member Repository containing the current
directory when the Issue names none. A new branch starts from the Repository's
local default branch (the local branch named by `origin/HEAD`, else `main`,
`master`, or the currently checked-out branch); no fetch is performed. An
existing `ticket/<id>` branch is checked out without resetting its commits.
An existing worktree is reused only at the expected path on that branch, so
repeating the claim by the same assignee is safe. A branch checked out elsewhere
or an incompatible target path is rejected before changing the claim.
`.worktrees/` is hidden through the Repository's `.git/info/exclude`. The JSON
result carries `data.worktree` with `repository`, `path`, `branch`, and `created`.

Claim and worktree creation are not one transaction. If preparation fails after
the claim commits (including a concurrent Git change), the command exits 1 with
`CLAIM_WORKTREE_FAILED`, but the claim remains active; Orbit never automatically
releases it, including an existing lease renewed by the same assignee. JSON
`error.details.claim` contains the successful claim result, `worktree` describes
the intended target, and `cause` contains the original error. Human output also
states that the claim remains active. Fix the cause and retry the same command,
or explicitly release the claim if you no longer intend to work on it.

### `orbit assign` — set an assignee without a lease

Sets who is responsible for an issue without taking a lease — how humans take
work. The assignee's `--kind` is `human` by default (`agent` allowed). A
different existing assignee is only replaced with `--force`, which also drops
the old lease; assigning the same identity again just updates its kind. A
lease-less assignee cannot `heartbeat` (there is no lease to extend) but can
still `checkpoint`, and the Issue stays out of the ready queue until released.

**Good fit:** routing work to a person, recording ownership ahead of a claim,
or handing an issue to a human reviewer.

```bash
orbit assign orb-a7b2c9 --to alice
orbit assign orb-a7b2c9 --to bob --kind agent --force
```

### `orbit heartbeat` — keep a lease alive

Extends the lease from now. Only the assignee holding a lease can heartbeat —
a lease-less (human) assignee has nothing to extend. Heartbeats are
lease-only writes and are not committed until the next mutation or `orbit sync`.

**Good fit:** a long build or test run would outlive the default 30-minute lease,
or a supervisor wants to confirm an agent is still alive.

```bash
orbit heartbeat orb-a7b2c9 --by claude
```

### `orbit checkpoint` — hand off active work

Records or replaces the assignee's latest checkpoint: current progress, the
next useful action, and any blocker. The first write requires `--progress`; later
writes can update only the fields that changed, and `--clear-next` or
`--clear-blocker` removes one. The caller must be the assignee. A stale lease still
counts until another agent takes the issue over, so the outgoing assignee can hand
off after a long run.

The checkpoint is visible in `show` (human and JSON), the JSON of `ready`,
`list`, and `prime`, the claim result, MCP `get`, and `export`. It survives
`release`, `claim --steal`, and `reopen`; it never changes readiness, claim
eligibility, or dependencies. Each write attributes the record to the current
assignee, even when a partial update keeps the previous progress text. On a done
or cancelled issue the latest checkpoint is retained in the issue file as archived
history, but new checkpoints are refused there.

**Good fit:** pausing work, handing an issue to another agent, or leaving enough
context that a stale claim can be recovered without reading logs.

```bash
# Record progress, the next action, and a blocker
orbit checkpoint orb-a7b2c9 --by claude \
  --progress "Parser and tests pass" --next "Wire the CLI" \
  --blocker "Waiting on review"

# Update one field; omitted fields keep their previous value
orbit checkpoint orb-a7b2c9 --by claude --clear-blocker

# Leave a handoff, then requeue the issue
orbit checkpoint orb-a7b2c9 --by claude --progress "Half done" --next "Finish the tests"
orbit release orb-a7b2c9 --by claude
```

### `orbit release` — hand work back

Clears the assignee and lease and returns the Issue to the workflow's release target (the ready status in the shipped templates). The assignee can release; `--force`
releases someone else's assignment. Terminal issues cannot be released. The latest
checkpoint is kept for the next assignee.

**Good fit:** work is deliberately handed back, a human takes over, or a claim was
taken by mistake and the queue should see the issue again.

```bash
orbit release orb-a7b2c9 --by claude
orbit release orb-a7b2c9 --force
```

### `orbit done` — close completed work

Moves the Issue to `done`. Unfinished dependencies always block completion. A
Parent Issue with open sub-issues is refused; `--force` cancels every open
descendant sub-issue and then closes the parent, and also overrides a competing
live claim. `--comment`
records the completion note in the same transaction as the status change.

**Good fit:** the work is implemented and verified. This is the normal end of the
agent loop.

```bash
orbit done orb-a7b2c9 --by claude --comment "Implemented and verified."
orbit done orb-a7b2c9 --by claude --comment-file notes.md
orbit done orb-a7b2c9 --force --comment "Deferred the remaining pieces"
```

### `orbit cancel` — abandon work

Cancels an issue without completing it. Cancelled issues count as resolved, so
dependents can become ready. A live lease from another agent blocks cancellation
unless `--force` is passed. A Parent Issue with open sub-issues is refused; `--force`
cancels every open descendant sub-issue too.

**Good fit:** the work is no longer needed, is a duplicate, or is a wontfix. Add
`--comment` to record the reason.

```bash
orbit cancel orb-a7b2c9 --by claude --comment "Superseded by orb-f14c02"
orbit cancel orb-a7b2c9 --force
```

### `orbit reopen` — undo a close

Returns a done or cancelled issue to the workflow's reopen target (the ready status in the shipped templates) and clears its resolution. Reopening a sub-issue reopens every
done ancestor too, nearest first, so the feature is not left done with reopened
work; the walk stops at an active ancestor. A sub-issue under a cancelled ancestor
is refused, naming that ancestor, until it is reopened. It
also refuses when a dependent is already done, because that would leave completed
work with an unfinished dependency; `--force` allows it and `doctor` reports the
historical warning.

**Good fit:** a release regressed, a ticket was closed by mistake, or a cancelled
idea is back in scope.

```bash
orbit reopen orb-a7b2c9
orbit reopen orb-a7b2c9 --force
```

### `orbit transition` — run a named workflow transition

Runs one named edge from the Board's workflow — the same edges `orbit show`
lists under `allowed_transitions`. The shipped `standard` template adds
`promote` (backlog `todo` to `ready`) and `defer` (`ready` to `todo`) on top of
the lifecycle edges; custom `[workflow]` definitions can add statuses such as
`In Review` and transitions such as `submit`. A transition into a done or
cancelled status records its resolution on the issue; a transition back to an
open status clears it. An unknown name or an edge that does not start from the
current status fails with `INVALID_TRANSITION` naming the valid transitions.

**Good fit:** moving work through Board-defined statuses that have no dedicated
command, such as promoting deferred work or submitting for review.

```bash
orbit transition orb-a7b2c9 promote
orbit transition orb-a7b2c9 submit --by claude
orbit show orb-a7b2c9 --json   # allowed_transitions lists the valid edges
```

### `orbit delete` — remove a mistake

Permanently deletes an issue and its comment thread. It refuses when dependents
exist, when the issue has sub-issues, or when another agent holds a live lease;
`--force` also removes the ID from each dependent's dependency list and deletes
the whole sub-issue subtree.

**Good fit:** duplicates or tickets created in the wrong Board. Prefer `cancel`
when the work should remain in history.

```bash
orbit delete orb-a7b2c9
orbit delete orb-a7b2c9 --force
```

## Maintain the board

### `orbit migrate` — convert a pre-ADR-0019 Board

Converts a Board that still uses a pre-ADR-0019 layout into a standalone Board
repository under the current layout, in one commit. Previews by default;
`--apply` performs the conversion. Supported sources include a Board directly
at `.orbit`, the legacy `.git/orbit/board` store, a `<anchor>/board` folder,
and worktree-backed Boards (`board.storage = "worktree"`). The result is the
canonical `.orbit/board` hidden Board — or a visible Board folder anchored by
the `.orbit/board` pointer file — with `.orbit/cache` beside it.

The same run rewrites Issue frontmatter `github:` links to `remote:`, moves
`repo:*` labels into the `repository` field, converts `[github]` configuration
into remote project entries, drops `board.storage`, and seeds `board.components`
from the Component values Issues already use. Board commands refuse legacy layouts with
`BOARD_NOT_MIGRATED` until this runs; `orbit doctor` reports the same finding
with `LEGACY_LAYOUT`.

`--apply` checks before it changes anything: it refuses when the target
folder already exists, when a staging folder from an interrupted run is left
over, or when the Board folder holds files that an enclosing repository
tracks. A worktree-backed Board's uncommitted edits are committed to its
branch first, so none are lost. The code repository's `orbit` branch is
deleted only when it still points at the migrated tip.

**Good fit:** once per Board after upgrading Orbit past
ADR-0019, or when `doctor`/`list` answers `BOARD_NOT_MIGRATED`.

```bash
orbit migrate            # preview the plan; writes nothing
orbit migrate --apply    # convert the resolved Board in place
orbit migrate --dry-run  # same preview, explicit
orbit --dir <board> migrate --apply   # migrate a specific Board directory
```

Re-running after `--apply` is a no-op, so an interrupted conversion can be
retried safely. Issue IDs, bodies, comments, assignments, and Git history are
preserved; the report lists every content change and the resulting commit.

### `orbit migrate model` — convert a Board to the Issue model

Converts a Board written before the Issue model (ADR 0022) — legacy status
names, numeric priorities, `depends_on`/`waiting_on`, `claimed_by`,
`area`/`repo`/`annotations`, missing project keys — into the canonical model,
in one commit. Previews by default; `--apply` performs the conversion. Run
`orbit migrate` first when the Board still uses a pre-ADR-0019 layout; this
command refuses with `MIGRATION_BLOCKED` until then.

The conversion is deterministic and idempotent:

- Legacy status names map onto the Board workflow by category: `todo` becomes
  the Ready status, `in_progress`/`done`/`cancelled` (and `canceled`) map to
  the matching workflow statuses. A pre-model Board (no `board.priorities`,
  no `[workflow]`) gets `workflow.template = "standard"` written explicitly.
- Numeric priorities index the ordered `board.priorities` list (0 is most
  important); `board.default_priority` is rewritten the same way.
- `depends_on`/`waiting_on` and `claimed_by`/`claimed_at`/`lease_until`
  rewrite to typed `links` and `assignee`/`lease`; `area`, `repo`, and
  `repo:*` labels move to `component`/`repository`; `annotations` merges into
  `properties`. Values that cannot move safely stay in place and are reported
  as warnings.
- Parented Issues typed as the Board's default type are re-typed to the
  hierarchy's leaf type (`sub-task` by default); Issue types the files use
  but the Board does not configure are added to `board.types`/`board.hierarchy`.
- Every Issue without a `key` receives the next sequential project key and
  `keys.toml` records the allocator state. Issue IDs, bodies, comments,
  checkpoints, unknown frontmatter fields, and existing keys/aliases are
  preserved.

`--apply` builds and validates the result in a temporary Git worktree, then
commits it as `orbit: migrate board model` and fast-forwards the Board, so a
failure leaves the original untouched. It refuses when the migration would
introduce new Board issues (`MIGRATION_BLOCKED` with the issue list) or when
the Board has uncommitted changes. Re-running after `--apply` reports
`already_migrated` and writes nothing. Verify the result with `orbit doctor`.

```bash
orbit migrate model            # preview the plan; writes nothing
orbit migrate model --apply    # convert the resolved Board in place
orbit migrate model --dry-run  # same preview, explicit
orbit --dir <board> migrate model --apply   # migrate a specific Board directory
```

### `orbit doctor` — health check

Reports the discovery trace, legacy layout, half-moved Board (`AMBIGUOUS_BOARD`), orphan worktree (`ORPHAN_WORKTREE`), storage mismatch (`STORAGE_MISMATCH`), agent configs with an absolute Board path (`ABSOLUTE_BOARD_PATH`), and applied profiles whose requirements this binary does not meet (`PROFILE_UNMET`). After `orbit migrate`, run `doctor` from the Board anchor or a member repository to verify that the Board resolves and its migrated graph is healthy. Running without a Board is not itself a failure. When a Board resolves, it also checks malformed issues, dangling dependencies, missing or nested parents, parent/ID mismatches, cycles, orphan or corrupt comments, stale temp files, uncommitted changes, a wrong checked-out branch, and a Claude hook that cannot find the running Orbit binary on `PATH`. Exits `1` when it finds an issue.

From a Board anchor or any member repository, the report identifies the resolved Board, missing or malformed layout, undiscovered/unreachable/stale member repositories, interrupted migration state, duplicate GitHub identities, orphaned links, field-level sync conflicts, and missing or conflicting managed agent setup. Plain non-Git member directories such as `docs/` or `notes/` are informational warnings and do not make the Board unhealthy; a directory that used to be a repository is reported as a stale repository instead. Each finding carries a severity, affected repository/Issue/Issue, cause, and a safe next action.

`--fix` removes stale temp files and re-attaches an orphan `orbit` worktree whose `DIR` was deleted. It never resolves semantic conflicts, merges or deletes Issues, detaches repositories, closes Issues, or overwrites user-authored agent files. Doctor is read-only unless `--fix` is requested; legacy Board layouts are converted with `orbit migrate`, not by doctor.

**Good fit:** after a crash, a merge, a board branch switch, a suspicious `NOT_FOUND`/`CORRUPT_TASK` error, or before adopting the new layout; also as a CI check.

```bash
orbit doctor
orbit doctor --fix        # temp files and an orphan worktree
orbit migrate             # convert a legacy Board to the standalone layout
orbit doctor --json
```

### `orbit sync` — persist lease-only changes

Commits pending board changes, mostly heartbeats, to the board branch. Nothing is
pushed. This is not the GitHub toggle: use `orbit remote auto-push on|off` to control
automatic Issue publishing.

**Good fit:** a long session only heartbeated, or `doctor` reports uncommitted
changes.

```bash
orbit sync
```

### `orbit index` — the derived read index

Inspects or rebuilds the opt-in `[index]` module: a rebuildable cache at
`<orbit>/cache/index.json` that speeds up `list`, `ready`, `search`, and the
other read scans on large Boards. Enable it in `config.toml`:

```toml
[index]
enabled = true
```

The index only ever caches what the Issue files parse to; files and Git
history remain the source of truth and a missing or stale index is rebuilt
automatically. The module exists because the large-Board scan benchmark
(`docs/benchmarks.md`) measured `list`/`ready` past one second at 10k Issues.

```bash
orbit index status            # enabled? entries? fresh vs stale?
orbit index rebuild           # force a full rebuild from the Issue files
```

#### `[[hooks]]` — local commands fired on Board events

Board `config.toml` may also carry `[[hooks]]` rules binding feed event
kinds to shell commands. After a write commits, the events its commits
produced are replayed and every matching rule runs detached via `/bin/sh -c`
with `ORBIT_EVENT_JSON` and friends in the environment — hooks are
best-effort and can never block or fail a write. See `docs/SPEC.md` §6 for
the full contract.

```toml
[[hooks]]
events = ["issue.transitioned", "issue.created"]
command = "echo $ORBIT_EVENT_TYPE $ORBIT_EVENT_KEY >> /tmp/orbit.log"
```

### `orbit capabilities` — the contract handshake

Prints what this Board and this `orbit` binary share: the contract version the
binary speaks, the `contract` semver the Board declares in `config.toml`,
whether they are compatible, the `capabilities` this binary provides (the
vocabulary Profiles negotiate `requires`/`optional` against), the optional
modules the Board has enabled (`remote` with its projects and `auto_push`,
`workspace` scoping, `scheduler` with its strategy and `wip_limit`), the Board's applied `profiles`, and the Board's workflow
— template or custom, initial status, statuses with categories, and named
transitions.

Every other Board command refuses a Board whose contract is newer than the
binary's with `CONTRACT_VERSION_UNSUPPORTED`; `capabilities` reads
`config.toml` leniently and still answers, so scripts and agents can
negotiate before choosing commands. An older or equal contract is accepted, and
a Board without the field reads as the current contract.

**Good fit:** checking compatibility before scripting against a Board,
or inspecting a Board's workflow and modules.

```bash
orbit capabilities
orbit capabilities --json | jq '.data.workflow.initial'
```

### `orbit board` — host and synchronize the Board

A Board has no host by default — its history stays on this machine — or
exactly one host whose remote stores the Board's `orbit` branch (ADR 0019).
The host may be a member Repository name, a local repository path, or a git
URL. A member Repository name — or the `owner/repo` identity of its `origin`,
matched case-insensitively — resolves through the Board's member Repositories
to that Repository's `origin`, or to its local path when it has none. A local
repository path resolves to that repository's `origin` remote when
it has one, so `orbit board host ../hub` stores the Board on the hub's tracker
(for example GitHub) rather than in a local `orbit` branch of the checkout; a
path without `origin`, such as a bare repository, is used as is. One Board per host: when the host already serves an `orbit` branch it
must hold a Board with the same Slug; `orbit board sync` checks this again
before every merge (`BOARD_HOST_CONFLICT`). A host spec cannot start with `-`.

`orbit board host` shows the current host with no argument, sets it with a
repository path or URL, and clears it with `--none` — clearing never deletes
the remote `orbit` branch. `orbit board sync` commits pending changes,
fetches the host, merges Orbit-aware, and pushes: different Issues merge,
different fields of the same Issue merge, Comments union, the earlier Claim
wins and the loser is told on the Issue, an Issue merged to `done` or
`cancelled` keeps no Claim, and history is merged — never
rebased or force-pushed. A same-field conflict stops the sync with
`BOARD_SYNC_CONFLICT`; rerun with repeatable `--resolve FIELD=local|host`
to pick a side (`config` resolves `config.toml`, `<task-id>` a whole Issue
file, `<task-id>.comments` a comment sidecar).

Set `board.host_sync = "auto"` in the Board's `config.toml` to also sync at
`orbit prime` and after every committed mutation; automatic failures are
warnings, never errors, and `--no-sync` suppresses them.

Sync holds the Board lock only for the local commit and merge, never during
the fetch or push, so a slow host does not block other agents. Host git
commands never prompt: SSH runs in batch mode with a 15-second connect
timeout, unless `GIT_SSH_COMMAND`, `GIT_SSH` or `core.sshCommand` is set, and
an HTTP transfer that stalls for 30 seconds is aborted (set
`GIT_HTTP_LOW_SPEED_TIME` to change that). When a concurrent sync wins the
race to the host, sync retries once with a fresh fetch.

**Good fit:** keeping one Board's history on a shared repository so several
machines or clones work from the same Issues.

```bash
orbit board host                     # show the current host
orbit board host ../board-host       # set or change the host
orbit board host --none              # clear it; remote branch untouched
orbit board sync                     # fetch, merge, push
orbit board sync --resolve orb-a1b2c3.title=host --json
orbit board move issues              # relocate the Board folder
orbit board view                     # serve the Board in a browser
```

`orbit board move DIR` relocates the Board folder. The Board's Git history
moves with it in one rename, so DIR must be on the same filesystem and must
not exist or be an empty directory. Orbit rewrites the anchor's `.orbit/board`
pointer file to name DIR — or restores the hidden layout when DIR is
`.orbit/board` — moves the `.git/info/exclude` entry to the new folder, and
repoints explicit member Repository links. The Board host and `cache/` are
unchanged.

`orbit board view [--port N] [--by NAME] [--no-open]` serves the Board from a
foreground server on `127.0.0.1` (ADR 0023). It prints
`orbit board view: http://127.0.0.1:PORT/#token=TOKEN` (`{"ok":true,"data":{"url":…}}`
with `--json`) and opens it unless `--no-open` is given. The token lives in the
URL fragment; the API requires it plus a local `Host`/`Origin`. Routes:
`GET /api/snapshot` (annotated Issues, ETag/304) and `GET /api/show/ID`, plus
the writes `POST /api/move` (`{id, version, column | status, transition?,
confirmed?}`), `POST /api/takeover`, and `POST /api/release` (`{id, version,
confirmed?}`). The server re-plans each write on a fresh `orbit show` and fails
with `409`: `MOVE_REFUSED`, `CHOOSE_TRANSITION`, `CONFIRM_REQUIRED`, or
`VERSION_CONFLICT`. `--force` is passed only for Take over, Release, and an
active drop over a lease-free Assignee, always after that server-side check,
`confirmed: true`, and `--if-version`. Stop with Ctrl-C.

### `orbit remote` — GitHub Issue provider

GitHub sync and Triage run through the built-in GitHub Remote Tracker adapter
(ADR 0018); every release build includes it.

Connects the current checkout's origin repository, adopts or creates Issues, and
runs bidirectional sync with conflict detection. `orbit remote connect` checks
`gh` authentication and repository access. For an organization-owned repository,
it validates Issue Fields and writes the `[[remote.projects]]` and
`[remote.github]` configuration. For a personal
repository, it leaves configuration unchanged and reports the exact settings to
configure basic mode: add a `[[remote.projects]]` entry and `[remote.github]`
with `mode = "basic"` to
the Board's `config.toml` or merge them into an existing section while
preserving other Remote Projects; the response includes those exact entries
and links to the setup guide. Existing integrations can connect repositories
explicitly. Sync requires the `gh` CLI and a configured Remote Project in
`config.toml`; full mode uses organization Issue Fields, while basic mode limits
Triage to labels. `orbit remote status` is a read-only diagnostic that reports
authentication and scopes, linked repositories, Issue Field configuration when
applicable, linked Issue count, pending changes, conflicts, unsupported
relationships, and a recommended next action. `orbit doctor` checks GitHub
configuration when the provider is enabled. GitHub Projects are a downstream
view over Issues: Orbit does not synchronize ProjectV2 status or custom fields;
use Project auto-add and item added/closed/reopened workflows for reporting.
Only GitHub's separate Auto-close issue workflow can turn a card move into an
Issue close, which pull sync then sees as an Issue change. See
[github-sync.md](./github-sync.md).

Enable `[remote] auto_push` (or the legacy `[github] auto_push`) to publish
mapped changes after a local mutation.
Toggle it with `orbit remote auto-push on` or `orbit remote auto-push off`; the
command writes the Board config and commits it, reports the previous value, and
warns when no single repository can receive newly created Issues. It is
push-only: `orbit add` creates an Issue when exactly one repository is
available, title/body/label/status/`component`/`verification` changes are pushed, and
`orbit:in-progress` tracks assignments. It never pulls, never overwrites a remote
conflict, and never creates an Issue for an existing Local Issue. `done` closes
with `state_reason: completed`; `cancelled` with `not_planned`. Priority, type,
comments, assignments, checkpoints, and relationships do not trigger a push. Use
`orbit --no-sync <command>` or `ORBIT_NO_AUTO_SYNC=1` for a local-only mutation;
explicit `orbit remote sync` is unaffected. See
[github-sync.md#automatic-push](./github-sync.md#automatic-push).

```bash
orbit remote connect
orbit remote status
orbit remote status --json
orbit remote status --repo acme/widget
orbit remote connect acme/widget
orbit remote connect --fields
orbit remote connect --fields --create-fields       # explicitly create missing org Issue Fields
orbit remote link orb-a7b2c9 https://github.com/acme/widget/issues/42
orbit remote link orb-a7b2c9 acme/widget#42                 # shorthand identity
orbit remote create orb-a7b2c9                 # preview only; uses the only linked repository
orbit remote create orb-a7b2c9 --apply         # create and link after review
orbit remote create orb-a7b2c9 orb-d4e5f6 --project acme/widget --apply
orbit remote create orb-a7b2c9 --include-closed --apply
orbit remote sync orb-a7b2c9              # dry-run plan
orbit remote sync orb-a7b2c9 --apply      # apply pull/push plan
orbit remote auto-push on                 # publish mapped changes after mutations
orbit remote auto-push off                # return to explicit sync only
orbit --no-sync add "Local-only issue"     # skip auto-push for one command
```

`orbit remote connect --fields` only resolves existing fields and stores their ids. Creating
missing fields requires the explicit `--create-fields` option and organization
administrator access. See [github-sync.md](./github-sync.md) for field types,
permissions, and remediation.

### `orbit triage` — agent review and apply

Works on Local and Linked Issues. For a Linked Issue, only the Remote Project's
Mapped fields can be proposed, since those are the only fields a later sync
publishes. Review displays field-by-field changes and, when a
proposal file is used with stdin and stdout connected to terminals, prompts for
explicit approval or cancellation. Non-TTY and `--json` reviews only preview; they
never prompt or apply. `--proposal-file -` reads a proposal from stdin. Applying
changes Orbit; with Remote Tracker auto-push enabled, the changed mapped fields
are pushed immediately. Otherwise, publish approved Linked Issue
changes with an explicit Remote Tracker sync (`orbit remote sync`). Local Issues report `sync_required: false`. See
[agents/triage-workflow.md](./agents/triage-workflow.md).

Apply preserves metadata on disk if the automatic Git commit fails and reports
`COMMIT_FAILED` in the output warnings, as every other mutating command does
(including `orbit remote link`/`create`/`sync`). Run `orbit sync` to retry
the commit; do not repeat the proposal
merely to recover the commit.

```bash
orbit triage context orb-a7b2c9 --json
orbit triage review orb-a7b2c9 --proposal-file proposal.json
orbit triage review orb-a7b2c9 --proposal-file proposal.json --json
orbit triage review orb-a7b2c9 --proposal-file - --json < proposal.json
orbit triage apply orb-a7b2c9 --proposal-file proposal.json --approve --json
orbit remote sync orb-a7b2c9 --direction push --apply
```

## Scenarios

### A solo maintainer triages a new report

```bash
orbit add "Login rejects valid tokens" -p high --label bug --label needs-triage --body-file report.md
# ... investigate, then decide:
orbit label add orb-a7b2c9 ready-for-agent
orbit label remove orb-a7b2c9 needs-triage
orbit comment add orb-a7b2c9 --by rodrigo --body "Confirmed; add a regression test."
```

### An agent takes one issue and finishes it

```bash
orbit ready --claim --by claude --json      # receives one issue plus lease
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

Each claim is atomic; when several issues are ready, both workers get one. If two
workers target the same Issue, the loser gets `ALREADY_CLAIMED` with the holder and
lease.

### An agent crashes mid-issue

```bash
orbit prime --json                     # lists stale claims
orbit claim orb-a7b2c9 --by agent-b --steal
orbit heartbeat orb-a7b2c9 --by agent-b
```

Use `release --force` instead when the Issue should go back to the queue unassigned.

### A release train with ordered work

```bash
orbit add "Design the schema" -p highest --label ready-for-agent
# created orb-f14c02  highest  Design the schema
orbit add "Ship the API" -p high --label ready-for-agent
orbit depend orb-a7b2c9 orb-f14c02
# created orb-a7b2c9  high  Ship the API
orbit add "Publish the SDK" -p high --label ready-for-agent
orbit depend orb-9c8d7e orb-a7b2c9
```

`orbit ready` returns only the first Issue. Completing it makes the next one ready
in the same order.

### A feature split into sub-issues

```bash
# the feature
orbit add "SSO login across api, web, infra" --body-file feature.md
# created orb-a7b2c9  medium  SSO login across api, web, infra

# one sub-issue per repository — run each add inside its member Repository so
# `repository` fills in, or set it afterwards with `orbit update <id> --repository NAME`
orbit add "api: OIDC callback" --parent orb-a7b2c9
orbit add "web: login redirect" --parent orb-a7b2c9
orbit add "infra: register client" --parent orb-a7b2c9

# an agent in the api repository picks its piece
orbit list --parent orb-a7b2c9 --repository api --json
orbit claim orb-a7b2c9.b3c4d5 --by api-agent

# when every piece is done or cancelled, close the feature explicitly
orbit done orb-a7b2c9 --by coordinator --comment "SSO shipped"
```

Each sub-issue is claimed and completed independently. The Parent Issue only becomes
Ready when the last one is done or cancelled. `orbit done --force` on the parent
cancels the remaining sub-issues instead of leaving them open.

### A Board with its own Issue-ID prefix

```bash
orbit init --slug teramigo
orbit add "First issue" --json
# teramigo-32fdfd
```

The slug is adopted once at creation. Existing `orb-` Boards keep working
unchanged; migration is optional. To change a populated Board, preview
`orbit slug migrate <slug> --dry-run`, then apply it. `init --slug` still only
changes an empty Board; a populated Board returns `SLUG_LOCKED` and points at the
migration command.

### A Board spanning several repositories

```bash
# initialize the anchor folder and its Board
mkdir -p ~/src/acme && cd ~/src/acme
orbit init

# member repositories discover the Board without linking
cd ~/src/acme/api
orbit list

# repositories outside the anchor join explicitly
orbit repo add ~/src/acme/api --name api
```

All member repositories and their worktrees share one dependency graph. The
Board lives at `~/src/acme/.orbit/board`; the rebuildable member inventory is
at `~/src/acme/.orbit/cache/member-repositories.json`. Use
`orbit --dir ~/src/acme/.orbit/board list` for a one-off command against it
regardless of the current directory.

### Collect the board into a knowledge base

```bash
orbit export --json > board.json
```

The snapshot includes every issue, including done and cancelled work, with labels,
dependency and parent edges, comments, and derived readiness. It is ordered by
issue ID and contains no timestamp, so the same board state produces the same
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
orbit migrate           # convert a legacy Board to the standalone layout
orbit sync              # commit anything left uncommitted
```

`doctor` never rewrites issues or comments, so it is always safe to run.

## Common outcomes and errors

| Error | What to do |
| --- | --- |
| `NO_READY_TASKS` | Not an error: nothing is startable. Wait, triage, or remove a hold label. |
| `VERSION_CONFLICT` | The Issue changed since the `--if-version` you passed. Re-read it with `orbit show <id>` and retry with the current `updated_at`; the error is retryable and carries `details.actual_version`. |
| `IDEMPOTENCY_KEY_REUSED` | The `--idempotency-key` was already used for a different request; use a fresh key. |
| `MODEL_NOT_MIGRATED` | The Board's `config.toml` has no `contract` (or an older major); run `orbit migrate model`. |
| `INVALID_IDEMPOTENCY_KEY` | `--idempotency-key` must be 1–200 printable ASCII characters without spaces. |
| `ALREADY_CLAIMED` | Another agent holds a live lease. Pick other work, or `--steal` to take it over. |
| `CLAIM_STALE` | The holder's lease expired. Recover with `orbit claim <id> --steal`. |
| `DEPENDENCY_BLOCKED` | Finish or cancel the IDs in `error.details.blocking`. |
| `WAITING_EXTERNAL` | Resolve the blocker, then `orbit update <id> --clear-waiting`. |
| `LABEL_BLOCKED` | Remove the hold label (`needs-triage`, `needs-info`, `ready-for-human`). |
| `NOT_CLAIM_HOLDER` | Use the assignee's `--by`, or `--force` where the command allows it. |
| `SUBTASKS_OPEN` | Finish or cancel the IDs in `error.details.sub_issues`, or pass `--force` to `done` or `cancel`. |
| `SUBTASKS_EXIST` | The Parent Issue still has sub-issues. Use `delete --force` to remove the whole subtree, or delete the sub-issues first. |
| `PARENT_CYCLE` | The `parent` chain loops back on itself. Break the cycle by reparenting or clearing `parent`. |
| `INVALID_SLUG` | `--slug` and `slug migrate` take a bare 1–20 character lowercase word with alphanumeric ends and no `--`; use the normalized suggestion in the error. |
| `SLUG_LOCKED` | The Board has issues. Run `orbit slug migrate <slug>` to rewrite every ID, or keep the current prefix. |
| `MIGRATION_BLOCKED` | Migration refused because of live claims, an unhealthy Board, a dirty Board, or a concurrent Board move. Read `error.details.reason`; run `orbit doctor`, then retry with `--force` only for live claims. |
| `PREFIX_COLLISION` | Two old IDs would become the same new ID. No files were changed; choose another slug. |
| `PREFIX_MISMATCH` | `id.prefix` and the Issue filenames disagree. Restore the configured prefix first; migration refuses an inconsistent Board. |
| `DANGLING_PARENT` | Run `doctor`; the parent ID is missing. Recreate the parent or fix the sub-issue's frontmatter. |
| `INVALID_TYPE` | The Issue type is not in `board.types`, or the `parent` edge violates `board.hierarchy` (the child's type must sit strictly below the parent's). |
| `MISSING_REQUIRED_FIELD` | The Issue type's `required` list in `board.types` names fields the Issue lacks. |
| `POLICY_VIOLATION` | A `[[policy.rules]]` rule in `config.toml` (or contributed by an applied profile) failed. `error.details.violations` lists each failed check; fix the fields/properties it names or adjust the rule. |
| `ALREADY_INITIALIZED` | A Board already resolves for this working tree. Use it, run `orbit repo remove` to drop an explicit membership, move or migrate an ancestor/main/legacy Board, or pass a `DIR` outside every working tree. |
| `BOARD_NOT_FOUND` | Run `orbit init`, pass `--dir`/`ORBIT_DIR`, or `orbit repo add` an existing Board. |
| `CONFIG_ERROR` | The target `DIR` is not empty (apart from `.git`), or a configuration value is invalid. Use a fresh directory for a new standalone Board. |
| `LINK_NOT_FOUND` | The linked `DIR` is missing. Re-link it or restore the Board. |
| `BOARD_NOT_MIGRATED` | The resolved Board uses a pre-ADR-0019 layout. Run `orbit migrate` (preview first, then `--apply`) to convert it; Board commands refuse until then. |
| `LEGACY_LAYOUT` | A warning, not a failure: the Board still uses the old hidden, `<anchor>/board`, legacy pointer, or explicit old-board-root layout. It keeps working with the warning; run `orbit migrate` to move supported layouts to the current path. |
| `AMBIGUOUS_BOARD` | A new `.orbit` and a legacy Board both exist. Run `orbit migrate` to finish or resume the move. |
| `ORPHAN_WORKTREE` | Legacy `DIR` was deleted but the `orbit` branch remains. Discovery does not repair it. Run `orbit doctor --fix` to restore the legacy Board, then `orbit migrate` to convert it to a standalone Board folder. |
| `STORAGE_MISMATCH` | `config.toml` still records `board.storage` but the Board folder's Git shape disagrees. `orbit migrate` removes the retired key and normalizes the layout. |
| `ABSOLUTE_BOARD_PATH` | A managed agent config still records a Board path. Re-run `orbit setup` to rewrite it path-free. |
| `LOCKED` | Retryable: another writer holds the lock. Retry after a short backoff. |
| `CONTRACT_VERSION_UNSUPPORTED` | The Board's `contract` in `config.toml` is newer than this `orbit` supports. Upgrade `orbit`; `orbit capabilities` still reports the Board's contract and workflow. |
| `PROFILE_UNMET` | A Profile's `requires` capabilities or minimum `contract` exceed this `orbit`. Upgrade `orbit` or apply a profile whose requirements are met; `details` names what is missing. `orbit doctor` reports the same code for an applied `[[profiles]]` record. |

## Command reference

<!-- BEGIN GENERATED COMMANDS (orbit schema --docs; edit src/registry.rs) -->

| Command | MCP tool | Access |
| --- | --- | --- |
| `orbit init` | — | — (creates Boards and agent harnesses; not a Board operation) |
| `orbit repo add` | `repo_add` | write |
| `orbit repo remove` | `repo_remove` | write |
| `orbit repo list` | `repo_list` | read-only, idempotent |
| `orbit repo refresh` | `repo_refresh` | write, idempotent |
| `orbit slug show` | `slug_show` | read-only, idempotent |
| `orbit slug migrate` | `slug_migrate` | write, destructive |
| `orbit migrate` | `migrate` | write, destructive |
| `orbit migrate model` | `migrate_model` | write, destructive |
| `orbit add` | `create` | write |
| `orbit list` | `list` | read-only, idempotent |
| `orbit ready` | `ready` | write |
| `orbit show` | `get` | read-only, idempotent |
| `orbit tree` | `tree` | read-only, idempotent |
| `orbit claim` | `claim` | write |
| `orbit assign` | `assign` | write, idempotent |
| `orbit release` | `release` | write |
| `orbit heartbeat` | `heartbeat` | write |
| `orbit checkpoint` | `checkpoint` | write, idempotent |
| `orbit done` | `finish` | write |
| `orbit cancel` | `finish` | write |
| `orbit reopen` | `reopen` | write |
| `orbit transition` | `transition` | write |
| `orbit submit` | `submit` | write |
| `orbit review` | `review` | write |
| `orbit ask` | `ask` | write |
| `orbit answer` | `answer` | write |
| `orbit update` | `update` | write, idempotent |
| `orbit label add` | `label` | write |
| `orbit label remove` | `label` | write |
| `orbit label list` | `label` | write |
| `orbit batch` | `batch` | write |
| `orbit property set` | `property` | write, idempotent |
| `orbit property remove` | `property` | write, idempotent |
| `orbit comment add` | `comment` | write |
| `orbit comment list` | `comment` | write |
| `orbit depend` | `link` | write |
| `orbit undepend` | `link` | write |
| `orbit link add` | `link` | write |
| `orbit link remove` | `link` | write |
| `orbit link list` | `link` | write |
| `orbit delete` | `delete` | write, destructive |
| `orbit prime` | `prime` | read-only, idempotent |
| `orbit search` | `search` | read-only, idempotent |
| `orbit digest` | `digest` | read-only, idempotent |
| `orbit export` | `export` | read-only, idempotent |
| `orbit changes` | `changes` | read-only, idempotent |
| `orbit history` | `history` | read-only, idempotent |
| `orbit setup` | — | — (writes agent harness configuration and Board config.toml profiles; not a Board operation) |
| `orbit board view` | — | — (is a local web server, not a Board operation) |
| `orbit mcp` | — | — (is the MCP server itself) |
| `orbit doctor` | `doctor` | write, idempotent |
| `orbit sync` | `sync` | write, idempotent |
| `orbit index status` | `index_status` | read-only, idempotent |
| `orbit index rebuild` | `index_rebuild` | write, idempotent |
| `orbit capabilities` | `capabilities` | read-only, idempotent |
| `orbit schema` | — | — (publishes this registry; not a Board operation) |
| `orbit board host` | `board_host` | write |
| `orbit board sync` | `board_sync` | write |
| `orbit board move` | `board_move` | write, destructive |
| `orbit remote status` | `remote_status` | read-only, idempotent |
| `orbit remote connect` | `remote_connect` | write |
| `orbit remote disconnect` | `remote_disconnect` | write, destructive |
| `orbit remote list` | `remote_list` | read-only, idempotent |
| `orbit remote issues` | `remote_issues` | read-only, idempotent |
| `orbit remote issues refresh` | `remote_issues_refresh` | write, idempotent |
| `orbit remote issues list` | `remote_issues` | read-only, idempotent |
| `orbit remote issues show` | `remote_issues_show` | read-only, idempotent |
| `orbit remote import` | `remote_import` | write |
| `orbit remote link` | `remote_link` | write |
| `orbit remote create` | `remote_create` | write |
| `orbit remote detach` | `remote_detach` | write |
| `orbit remote reattach` | `remote_reattach` | write |
| `orbit remote sync` | `remote_sync` | write |
| `orbit remote auto-push` | `remote_auto_push` | write, idempotent |
| `orbit triage context` | `triage_context` | read-only, idempotent |
| `orbit triage review` | `triage_review` | read-only, idempotent |
| `orbit triage apply` | `triage_apply` | write |

<!-- END GENERATED COMMANDS -->


`orbit schema [COMMAND]` prints the same information as JSON;
`orbit schema --docs` regenerates this section. Meta commands (`init`,
`setup`, `mcp`, `schema`) are exempt from MCP exposure because they configure
the environment rather than operate on a Board.

## See also

- [README](../README.md) for the overview, quick start, and multi-repository Board model.
- [Architecture](./ARCHITECTURE.md) for the component map, state machine, command flow, and sequence diagrams.
- [Specification](./SPEC.md) for the canonical JSON contract and transition rules.
- [Glossary](./GLOSSARY.md) for the canonical vocabulary.

# GitHub Issue sync

Orbit ships a built-in **GitHub Remote Tracker adapter** (ADR 0018). The
provider-neutral sync engine owns baselines, three-way conflict detection,
previews, Auto-push, relationships, and the Remote Issue catalog; the GitHub
adapter translates GitHub Issues into Orbit field terms. `orbit remote`
commands are the GitHub-facing surface of that port — there is no optional
build feature and no minimum-version gate.

Orbit remains local-first: Task edits stay on the Board; Issue creation
occurs only with `orbit remote create --apply` or an opted-in auto-push
during `orbit add`, and Orbit persists the Issue link only after GitHub
confirms creation. Changes to an existing Issue are published only with
`orbit remote sync --apply` or an opted-in auto-push. See
[Automatic push](#automatic-push).

## Quick start

For an organization-owned repository with Issue Fields ready, create a fresh
Board in the repository checkout and authenticate GitHub CLI:

```bash
orbit init
gh auth login
orbit remote connect
orbit remote status
orbit add "First GitHub task" -p 1
# Copy the ID printed by `orbit add` and replace orb-a7b2c9 below.
orbit remote create orb-a7b2c9
orbit remote create orb-a7b2c9 --apply
orbit remote sync orb-a7b2c9        # inspect the plan; writes nothing
orbit remote sync orb-a7b2c9 --apply
```

## Provider modes

The GitHub adapter supports two modes, configured under `[remote.github]` on a
current Board (the legacy `[github]` section is still read until `orbit
migrate` converts it). Missing configuration defaults to `full`, so existing
organization setups keep their current behavior.

### Full mode (organization Issue Fields)

- Requires organization-level Issue Fields named **Area** and
  **Verification** (names are configurable under `[remote.github.issue_fields]`).
- Run `orbit remote connect --fields` to resolve and store the Issue Field GraphQL ids.
  Use `orbit remote connect --fields --validate-only` to check configuration without
  writing ids.
- Full mode syncs Orbit `area` and `verification` through those organization
  Issue Fields.

Connect an organization-owned repository from its checkout with
`orbit remote connect`. It checks `gh auth status` and repository access,
validates the organization fields, and writes the Remote Project and
`[remote.github]` configuration. It
preserves other Board settings and is safe to rerun. Use
`orbit remote connect --json` for machine-readable status. To configure the
Board manually, use:

```toml
[[remote.projects]]
tracker = "github"
project = "acme-corp/widget"

[remote.github]
mode = "full" # optional; full is the default
organization = "acme-corp"

[remote.github.issue_fields]
area_name = "Area"
verification_name = "Verification"
```

On an unmigrated Board the same settings live under `[github]` with
`repositories = ["acme-corp/widget"]`; `orbit migrate` converts them into the
shape above.

For manual configuration, run `orbit remote connect --fields` to resolve existing Issue
Fields and store their GraphQL ids. This default command does not create
fields. Use `orbit remote connect --fields --validate-only` to check configuration
without writing ids.

To let Orbit create missing fields, explicitly run
`orbit remote connect --fields --create-fields`. It creates **Area** as a single-select
field whose options are the Board's `board.areas` (or the canonical `board`,
`cli`, `mcp`, `setup`, `docs`, `release` set when the Board defines no Areas),
and **Verification** as a text field. Existing fields with those names are
reused and are not changed. Before creating anything, Orbit verifies that the
authenticated `gh` user has an active organization administrator membership.
If the membership check fails, refresh the `read:org` scope with
`gh auth refresh --hostname github.com --scopes read:org`. If field creation
is denied, confirm that `gh` is authenticated as an organization administrator
with permission to create organization Issue Fields. An administrator can also
create the fields in GitHub and rerun the default `orbit remote connect --fields` to
store their ids.

### Basic mode (personal repositories)

Basic mode supports personal repositories without organization Issue Fields
or `orbit remote connect --fields`:

```toml
[[remote.projects]]
tracker = "github"
project = "alice/widget"

[remote.github]
mode = "basic"
```

Link repositories, adopt or create Issues, and explicitly sync linked Tasks or
enable auto-push. Orbit owns title, body, status, and labels. A GitHub-only
edit can be pulled when the matching Orbit field still equals its saved
baseline; an Orbit-only edit is pushed. Concurrent edits to one field require
a reviewed resolution. Label order is not a change: Orbit compares labels as a
set, so a Task whose labels are listed in a different order than the Issue
neither pushes nor conflicts. GitHub assignees remain informational and are
never mapped to Orbit claims.

Basic mode does **not** support organization Issue Fields: `area` and
`verification` are neither imported from GitHub nor pushed to GitHub. Create
and push sync fail with a clear error when a Task has either value, rather
than silently dropping it. Pull-only sync preserves Orbit's local values.
Triage context reports these capabilities as unavailable; Triage review/apply
may change labels, but proposals that change `area` or `verification` are
rejected. `orbit remote connect --fields` is unavailable in basic mode. Do not set
`organization`, custom Issue Field names, or Issue Field ids with
`mode = "basic"`.

To upgrade, change `mode` to `"full"`, configure `organization` and the field
names, then run `orbit remote connect --fields`. Once the Issue Field ids are stored,
create and push sync can publish `area` and `verification`.

### Connect behavior and prerequisites

- GitHub CLI (`gh`) authenticated for the repositories you link.
- Organization-level Issue Fields only when using full mode.

`orbit remote connect` reads the current checkout's GitHub `origin`, checks
authentication and access, and detects whether the owner is an organization or
a personal account. For a personal-account repository it leaves configuration
unchanged and reports the exact `[[remote.projects]]` and `[remote.github]`
entries for the Board's `config.toml`
to configure basic mode; merge them with existing settings and preserve other
Remote Projects. For an organization repository, it validates and stores
the repository and Issue Field configuration. Missing authentication,
repository access, or fields are reported with remediation actions. JSON
output includes `owner_type`, `organization`, `repository`, `field_status`,
`next_action`, and whether the configuration changed. For a personal-account
origin, it also includes `next_actions` and `remediation` with those entries
and a link to the basic-mode setup instructions; human output shows the same
guidance. No configuration is changed by this response.

For manual full-mode configuration, run `orbit remote connect --fields` to resolve and
store the Issue Field GraphQL ids. Use `orbit remote connect --fields --validate-only` to
check configuration without writing ids.

## Diagnose the integration

`orbit remote status` checks the current `gh` authentication identity and
scopes, linked repository access, and—when using full mode—the configured
organization and whether its Area and Verification Issue Fields are present
with current stored ids. It also loads linked Tasks and previews their sync
plans to count pending pull and push changes and detect conflicts.
Cross-project SubTask and dependency links are reported as unsupported. The
command is read-only: it does not change Board configuration, Tasks, GitHub
Issues, or the fixture used by integration tests. In basic mode, it reports
the `repo` scope, skips organization checks, and marks Area and Verification
as not applicable.

Use `--json` for the stable diagnostic structure. `data.pending.pull` and
`data.pending.push` count field changes; `pull_tasks` and `push_tasks`
identify the affected Tasks and counts. `data.conflicts` and
`data.conflict_details` report conflicts, while `data.unsupported_relationships`
and `data.unsupported_details` report relationships Orbit cannot synchronize.
`data.next_action` recommends the next step, and `data.next_command` supplies
an executable command when one applies. `data.auth.scopes_known` distinguishes
an unreported scope list from an empty one. When the GitHub provider is
configured, `orbit doctor` also reports authentication, repository access, and
Issue Field configuration problems alongside normal Board diagnostics.

```bash
orbit remote status
orbit remote status --json
orbit doctor --json
```

### Tell Local and Linked work apart

`orbit list` prints Tasks only. Its `ISSUE` column is `-` for a Local Task,
`#N` for a Linked Task, and `~#N` for detached provenance, so `--linked` and
`--local` filter the table without opening each Task. `--local` includes
detached provenance, which Orbit no longer synchronizes.

`--json` carries the full identity: `github` is `null` for a Local Task and an
object with `repo`, `number`, and `last_sync` for a Linked Task (the
historical GitHub-shaped view; `remote` carries the provider-neutral link).

```bash
orbit list --linked
orbit list --local
orbit list --json | jq -r '.data.tasks[] |
  [.id, (if .github then "\(.github.repo)#\(.github.number)" else "local" end), .title] | @tsv'
```

Sync state is computed, not stored. `orbit remote status` compares each
sync baseline with the current Task and Issue and reports pending pulls,
pushes, and conflicts. `orbit remote sync <TASK>` previews one Task and
changes nothing without `--apply`.

## Link repositories

Remote Projects are connected explicitly; Orbit does not bulk-import an entire
repository. `orbit remote connect` writes a `[[remote.projects]]` entry for
the `github` tracker (plus `[remote.github]` adapter settings when needed);
on an unmigrated Board it appends to `[github] repositories`. Both shapes
feed the same adapter configuration.

```bash
orbit remote connect acme-corp/widget
orbit remote list
orbit remote disconnect acme-corp/widget
```

On a multi-Repository Board, member Repositories discovered by
`orbit repo refresh` are also valid Remote Projects: a member Repository's
`origin` (`owner/repo`) is implicitly available to adopt, create, status, and
sync without a separate `orbit remote connect`.

## Automatic push

Auto-push is provider-neutral: the flag lives in `[remote] auto_push` and
covers every configured Remote Tracker. `orbit remote auto-push on|off`
toggles it without editing TOML; the command writes the Board config,
commits it, and reports the previous value. The legacy `[github] auto_push`
key is still honored until migration rewrites it.

```bash
orbit remote auto-push on
orbit remote auto-push off
```

The equivalent Board configuration is:

```toml
[remote]
auto_push = true

[[remote.projects]]
tracker = "github"
project = "acme-corp/widget"

[remote.github]
mode = "basic"
```

The default is `false`. Auto-push is **push-only**: it never pulls, never
overwrites a field that changed on GitHub, and never creates an Issue for an
existing Local Task. It runs synchronously while the Board lock is held, so
an opted-in mutation waits for the `gh` calls to finish.

What triggers a push:

- Creating a Task: `orbit add` creates an Issue when exactly one repository
  is linked or implied. With zero or multiple repositories the Task stays
  local and the command warns. Use `orbit remote create` for the explicit
  path.
- A change to title, body, labels, status, `area`, or `verification`.
- Status transitions: `claim` adds the reserved `orbit:in-progress` label;
  `release`, `done`, `cancel`, and reopen remove it. `done` closes the Issue
  with GitHub `state_reason: completed`; `cancelled` closes it with
  `state_reason: not_planned`; reopen reopens it.

What does not trigger a push: priority, type, `waiting_on`, claims,
heartbeats, checkpoints, comments, and dependency relationships. Those
fields either have no GitHub representation or remain explicit. `orbit
remote *`, `orbit sync`, `orbit board *`, `doctor`, migrations, and `delete`
are also excluded. `orbit triage apply` is included because it changes
labels, `area`, and `verification`.

If a field changed on both sides, auto-push skips it and warns. Remote-only
changes produce a pending-change warning. Both leave the sync baseline
unchanged, so `orbit remote sync <task>` can still review and resolve the
difference. A failed push is also a warning — the local Task is never rolled
back. If the local commit failed, auto-push is skipped.

`orbit:in-progress` is an adapter-managed label, not part of the Task's user
Labels. The GitHub adapter reconciles it from the Orbit status on every
push: it is added while a Task is `in_progress` and removed otherwise. Orbit
never reads it back as a user label or a status. Orbit ensures the label
exists when a Remote Project is connected and when a Task is linked or its
Issue created; pull sync ignores it. Enabling `auto_push` backfills nothing: existing Local
Tasks still need `orbit remote create`, and existing `in_progress` Tasks
receive the label on their next mapped change or explicit sync. `orbit remote status` shows whether auto-push is enabled and how many local Tasks
have no Issue; `orbit doctor` reports a missing reserved label or no linked
repository.

Use `orbit --no-sync <command>` or `ORBIT_NO_AUTO_SYNC=1` for a one-off
local-only mutation. Explicit `orbit remote sync` is never suppressed.

## Adopt or create Issues

- **Adopt** an existing Issue from its URL:
  `orbit remote link <task-id> https://github.com/acme-corp/widget/issues/42`
- **Adopt** an Issue with the shorthand identity form:
  `orbit remote link <task-id> acme-corp/widget#42`
- **Preview** Issue creation for one or more Local Tasks with
  `orbit remote create <task-id> [<task-id> ...]`. Preview shows repository,
  title, body, labels, Area, Verification, Task status, and the resulting
  open/closed Issue state. It does not write to Orbit or GitHub.
- **Apply** a reviewed export with `orbit remote create <task-id> --apply`. A
  batch uses an explicit list of Task IDs; it never exports the whole Board.
  Done and cancelled Tasks require `--include-closed`; the former
  `--include-terminal` spelling is still accepted as a hidden alias.
- Full mode includes Task `area` and `verification` values as organization
  Issue Fields when configured with `orbit remote connect --fields`. Basic mode rejects
  Tasks with those values instead of dropping them.
- A repository is inferred only when exactly one repository is available. On
  a multi-Repository Board, member Repositories from `orbit repo refresh`
  count as available destinations. Pass `--project OWNER/REPO` when the
  destination is ambiguous.
- GitHub creates each Issue before Orbit persists the link and sync baseline.
  Batch output reports each success or failure. A failure after the Issue
  exists (the link could not be written, or a done/cancelled Task's Issue
  could not be closed) is reported per Task with `created: true` and the
  Issue number, and the batch continues; it never aborts the remaining Tasks.
  If the link could not be written, the result's `next_action` gives the
  `orbit remote link` command that links the created Issue. If a Task already
  has a link, retry is refused and directs you to `orbit remote sync`; this
  prevents a second Issue from being created.

Preview and apply examples:

```bash
orbit remote create orb-a7b2c9
orbit remote create orb-a7b2c9 --apply
orbit remote create orb-a7b2c9 orb-d4e5f6 --project acme-corp/widget --apply
orbit remote create orb-a7b2c9 --include-closed --apply
```

When using a URL, Orbit reads `owner/repo` and the issue number from the
standard GitHub Issue URL. Identities may also be written `PROJECT#ID`
(for example `acme-corp/widget#42`) or `TRACKER:PROJECT#ID` (for example
`github:acme-corp/widget#42`). Titles and bodies are not used as identifiers.

On a single-Repository Board, Orbit selects the only linked repository
automatically. On a multi-Repository Board, the member Repository catalog is
used. If zero or multiple repositories are available, link or include one, or
pass `--project OWNER/REPO`. An explicit destination must be a linked Remote
Project or a member Repository's `origin`.

## Sync behavior

Preview (default):

```bash
orbit remote sync <task-id>
```

The default preview is human-readable and does not write to Orbit or GitHub.
It shows each linked Task and Issue, the fields planned to be pulled into
Orbit or pushed to GitHub, and any conflicts with both competing values.
Empty plans say that no linked Tasks were found; a linked Task with no
pending changes is marked **No changes needed**. Use `--json` for the
machine-readable plan and conflict details.

Apply changes:

```bash
orbit remote sync <task-id> --apply
orbit remote sync --apply                 # all linked Tasks
orbit remote sync <task-id> --apply --direction pull
```

Rules:

| Field | Authority | Notes |
| --- | --- | --- |
| Title, body | Orbit | A remote-only edit is pulled if Orbit still matches the baseline; local edits push. Same-field edits conflict. |
| Status and open/closed | Orbit status | `todo`/`in_progress` stay open; `done`/`cancelled` close; remote state changes pull only if Orbit status is unchanged. GitHub `state_reason` drives the mapping: `completed` pulls as `done`, `not_planned` as `cancelled`; a closed Issue with the legacy `wontfix` label also maps to `cancelled`. |
| Labels | Orbit | Labels are synchronized as an exact field value, not merged. The adapter-managed `orbit:in-progress` marker rides along but is never compared as a user label. Remote-only edits pull; edits on both sides conflict. |
| `area`, `verification` | Orbit (via Issue Fields) | Full mode only; remote-only edits can pull; Orbit changes push; concurrent edits conflict. Unavailable in basic mode. |
| Assignees | GitHub only | Never mapped to Orbit claims |
| Claims, checkpoints | Orbit only | Never synchronized to GitHub |

Conflicts return `REMOTE_ISSUE_CONFLICT` with per-field details and both
values. No write occurs while any selected Task has an unresolved conflict.
Choose a side for each conflicting field with repeatable
`--resolve FIELD=orbit` or `--resolve FIELD=remote`, then require
`--apply` to write. `orbit` keeps the Orbit value and pushes it; `remote`
pulls the remote value. A resolution can be previewed without
writing:

```bash
orbit remote sync orb-a7b2c9 --resolve title=orbit
orbit remote sync orb-a7b2c9 --resolve title=orbit --resolve labels=remote --apply
```

For a batch, one resolution applies to every selected Task that has a
conflict in that field. Sync plans all selected Tasks before writing. Apply
then reports each Task's result independently; a failed Task keeps its
previous baseline, so it can be retried without repeating already successful
Tasks' writes.

GitHub command errors include a `Next:` action in human output. With
`--json`, the existing `{ "ok": false, "error": ... }` envelope is
unchanged and `error.details` includes the original `cause` and the same
`remediation`, plus the affected organization, Remote Project, Task, Issue,
or fields when known.

- `GITHUB_NOT_CONFIGURED`: run `orbit remote connect` from the checkout with
  the intended GitHub origin; authenticate first with `gh auth login` if
  needed.
- `GITHUB_SETUP_INCOMPLETE`: run `orbit remote connect --fields` to resolve field ids.
  If a named Issue Field is missing, create it under organization
  **Settings > Projects > Issue fields** or run
  `orbit remote connect --fields --create-fields` as an organization administrator. Run
  `orbit remote connect` if the organization is not configured.
- `REMOTE_PROJECT_NOT_LINKED`: the Remote Project (`owner/repo` on the
  `github` tracker) is not available to this Board. Link it with
  `orbit remote connect OWNER/REPO`, or run `orbit repo refresh` on a
  multi-Repository Board so a member Repository's `origin` counts.
- `REMOTE_ISSUE_CONFLICT`: the error names the Task, Remote Issue, and
  conflicting fields. Review both values with `orbit remote sync
  <task-id>`, choose each side with `--resolve FIELD=orbit|remote`, then
  apply with `--apply`.

## GitHub Projects

Orbit treats GitHub Projects as a downstream view over Issues. It does not
call the ProjectV2 API, require a `project` or `read:project` scope, or read
or write Project items, Status, views, or custom fields. Orbit's authority
ends at the Issue identified by `owner/repo#number`.

This boundary is deliberate. A Project can contain Issues from several
repositories, the same Issue can appear in several Projects, and each Project
has its own Status options and custom fields. Orbit keeps one Task link per
Remote Issue, so Project membership never creates duplicate Tasks or a
second source of truth.

| Concern | Authority | Orbit behavior |
| --- | --- | --- |
| Task lifecycle | Orbit | `todo`, `in_progress`, `done`, `cancelled` |
| Issue title, body, labels | Orbit | Explicit sync writes them to GitHub |
| Issue open/closed state | Orbit | `todo`/`in_progress` open; `done`/`cancelled` closed |
| `Area`, `Verification` | Orbit | Organization Issue Fields in full mode |
| Claims and Checkpoints | Orbit | Local only; never written to GitHub |
| Project membership | GitHub | Auto-add workflow or manual add |
| Project Status and custom fields | Project | Never read or written by Orbit |
| Project Status to Issue close | GitHub workflow | Only if **Auto-close issue** is enabled; the resulting Issue change is visible to pull sync |
| Project draft Issues | GitHub | Invisible to Orbit until converted to a repository Issue |

### Set up a Project for Orbit Issues

1. Create or open the Project that should report on the repository.
2. Link the Project to the repository or organization.
3. Open the Project menu, choose **Workflows**, and enable **Auto-add to
   project** for each repository Orbit exports from.
4. Configure the status workflows:
   - **Item added to project** sets `Status` to `Todo` for new items.
   - **Item closed** sets `Status` to `Done`. New Projects enable this by
     default; verify it is on.
   - **Item reopened** sets `Status` back to `Todo`.
5. Add views for the distinctions Project Status cannot represent, for
   example `label:wontfix` for cancelled work or `is:open` for active work.

The optional GitHub CLI equivalent for creating and linking a Project is:

```bash
gh auth refresh -s project       # only needed for gh project commands
gh project create --owner acme --title "Engineering delivery"
gh project link 1 --owner acme --repo acme/widget
gh project view 1 --owner acme --web
```

Auto-add and status workflows are configured in the Project UI; the
`gh project` command does not configure them. Orbit itself needs no Project
scope or configuration.

Auto-add only runs when an item is created or updated. It does not backfill
Issues that already exist. To add existing Issues, add them manually in the
Project, or use the GitHub CLI:

```bash
gh project item-add 1 --owner acme \
  --url https://github.com/acme/widget/issues/42
```

Do not enable **Auto-close issue** unless Project Status should be able to
close Issues. When that workflow is on, moving an item to `Done` closes the
Issue, and a later `orbit remote sync` can pull that close into the Task.
That is a GitHub workflow changing the Issue, not Orbit reading Project
Status.

### Example: create an Issue and let the Project pick it up

From a Board linked to `acme/widget`:

```bash
orbit init
gh auth login
orbit remote connect
orbit add "Rotate API tokens" -p 1
# Use the Task ID printed by `orbit add` in the next command.
orbit remote create orb-a7b2c9 --apply
```

The export output names the new Issue, for example `acme/widget#42`. With the
auto-add workflow enabled, the Issue appears in the Project without any
Orbit Project command. Verify it from the CLI:

```bash
gh issue view 42 --repo acme/widget --json state,labels
gh project item-list 1 --owner acme --field Status --format json
```

The Issue starts open. If the **Item added to project** workflow sets
`Status` to `Todo`, the item appears there; otherwise its Status may stay
empty until you set it. Project Status is not derived from the Issue's open
state unless you configure that workflow.

### Example: track an existing Issue

To attach an existing Issue to a Local Task you already have, adopt it by
URL:

```bash
orbit add "Rotate API tokens" -p 1
orbit remote link orb-a7b2c9 https://github.com/acme/widget/issues/42
```

Adopt copies the Issue's title, body, labels, and status onto the Task and
records the link. It does not create or modify the Issue.

### Example: finish work and move the Project card

Orbit's Project integration is event-driven through normal Issue changes.
Close the Issue with an explicit sync (or auto-push, when enabled) after
finishing the Task:

```bash
orbit claim orb-a7b2c9 --by agent
orbit done orb-a7b2c9 --by agent --comment "Shipped and verified"
orbit remote sync orb-a7b2c9 --apply
```

`orbit done` changes the local Task to `done`; it does not contact GitHub by
itself. An explicit `orbit remote sync` (or auto-push) closes Issue
`acme/widget#42`. If the Project has the **Item closed** workflow enabled,
GitHub then moves the item to its `Done` Status. Verify both sides:

```bash
gh issue view 42 --repo acme/widget --json state,labels
gh project item-list 1 --owner acme --query "status:Done" --format json
```

Reopening works in the other direction:

```bash
orbit reopen orb-a7b2c9
orbit remote sync orb-a7b2c9 --apply
```

The Issue reopens and the **Item reopened** workflow can return the Project
card to `Todo`. If the Task had been cancelled with `wontfix`, remove that
label explicitly before syncing; reopening does not remove labels:

```bash
orbit label remove orb-a7b2c9 wontfix
```

### Example: keep cancelled work distinct

Orbit closes a cancelled Task's Issue just as it closes a done Task. To
preserve the cancelled distinction in GitHub, add the `wontfix` label before
the sync:

```bash
orbit label add orb-a7b2c9 wontfix
orbit cancel orb-a7b2c9 --comment "Superseded by the new token service"
orbit remote sync orb-a7b2c9 --apply
```

The Issue is closed with `wontfix`. Orbit does not add that label
automatically. A simple **Item closed** workflow still shows the card as
`Done`, so add a Project view filtered by `label:wontfix` if cancelled work
needs its own column or report.

### What Project Status can and cannot show

A typical mapping looks like this:

| Orbit state | GitHub Issue | Typical Project Status | Limitation |
| --- | --- | --- | --- |
| `todo` | Open | `Todo` from **Item added to project** | None if the workflow is enabled; otherwise Status may be empty |
| `in_progress` | Open, `orbit:in-progress` label | Unchanged | No Issue event distinguishes it from `todo` beyond the managed label |
| `done` | Closed, `state_reason: completed` | `Done` from **Item closed** | None |
| `cancelled` | Closed, `state_reason: not_planned` | `Done` from **Item closed** | Distinguish with a label view |

Orbit deliberately does not invent a Project event for `in_progress`. If a
Project needs an `In Progress` column, use the `orbit:in-progress` label or
another Project workflow that you control, and keep in mind that a Project
Status change does not update Orbit.

The exception is GitHub's own **Auto-close issue** workflow. If you enable
it, a Project Status change to `Done` closes the Issue. The next pull sync
sees a closed Issue and can set the Task to `done` when Orbit's status has
not changed since the last sync. Even then, Orbit is reacting to the Issue,
not reading Project Status. Without that workflow, moving a card has no
effect on Orbit.

### Draft Issues, custom fields, and multiple Projects

- **Draft Issues are not repository Issues.** They have no `owner/repo#number`,
  so Orbit cannot list, import, or link them. Convert the draft to an Issue
  in its repository before adopting it.

- **Project custom fields are separate from organization Issue Fields.** A
  Project field named `Area` or `Verification` is not the organization field
  Orbit reads and writes. Renaming or editing the Project field does not
  change the Issue or the Task.

- **Multiple Projects are safe.** The same Issue can be in a delivery board,
  a roadmap, and a release view at the same time. Orbit still keeps one Task
  link for the Issue. Each Project may apply different workflows and show a
  different Status; Orbit does not choose between them.

- **Pull requests are not linked.** A Project can contain pull requests, but
  Orbit links only Issues. A Project item that is a pull request is outside
  the integration.

### Troubleshooting

- **A new Issue is missing from the Project:** confirm the Project is linked
  to the repository and that **Auto-add to project** includes that
  repository. Check the workflow's filter; an auto-add filter can exclude
  the Issue. Auto-add only handles new or updated items, so add existing
  Issues with `gh project item-add`.
- **Closing the Issue did not move the card:** run `orbit remote sync
  <task-id> --apply` after `orbit done` or `orbit cancel`. Then confirm the
  Project's **Item closed** workflow is enabled.
- **A card move did not update Orbit:** this is expected unless the Project
  has **Auto-close issue** enabled. Project Status is not an input to Orbit.
  Close or reopen the Issue instead.
- **A cancelled Task appears as Done:** add `wontfix` before syncing and use
  a `label:wontfix` view. A Project Status is a single value, not an Orbit
  status.
- **`gh project` fails with a scope error:** run `gh auth refresh -s project`.
  This is only for the GitHub CLI's Project commands; Orbit does not need
  it.
- **A Project field named Area is empty or stale:** Project custom fields do
  not mirror organization Issue Fields. Inspect or edit the Issue Field
  through the Issue and Orbit's sync workflow, not through the Project.

No additional Orbit configuration is required. See
[ADR 0015](./adr/0015-github-projects-downstream.md).

## Relationships

SubTasks and dependencies are previewed as GitHub sub-issues and blocked-by
relationships when both linked Issues live in the **same** Remote Project.
`--apply` adds supported relationships and removes a same-project
relationship only when it was present at the last successful sync baseline
and has since disappeared from the Task graph. Cross-project relationships
and remote-only additions that were never synced are listed as unsupported
and preserved; Orbit does not flatten them or guess which unknown
relationship to remove. GitHub assignees remain separate from Orbit claims.

## Task frontmatter

Linked Tasks store a provider-neutral `remote:` link (ADR 0018); `orbit
migrate` rewrites legacy `github:` links automatically, and older Task files
remain readable:

```yaml
area: backend
verification: cargo test
remote:
  tracker: github
  project: acme-corp/widget
  id: "42"
  url: https://github.com/acme-corp/widget/issues/42
  baseline:
    at: 2026-10-23T12:00:00Z
    remote_updated_at: 2026-10-23T11:55:00Z
    orbit_updated_at: 2026-10-23T11:50:00Z
    title: "Example"
    body: "Body"
    labels: [bug]
    area: backend
    verification: cargo test
    status: todo
    remote_status: todo
```

An active link omits `active` (older Task files remain active by default). A
detached Local Task keeps the same tracker, project, issue id, and baseline,
but stores `active: false`; Orbit treats that identity as provenance and
excludes it from synchronization until reattachment.

`--json` output keeps the historical `github` object (`repo`, `number`,
`last_sync`) next to the provider-neutral `remote` link for compatibility.

`area` values are free-form lowercase slugs on the Board; each Board
restricts them through `board.areas` (empty accepts any slug). A Board with
configured Areas should create its GitHub Area field with matching options
so pushed values resolve.

## Testing

Integration tests set `ORBIT_GITHUB_FIXTURE` to a JSON file with mock org
fields and issues so CI does not call GitHub. Basic mode ignores Issue Field
values in fixture Issues.

See [ADR 0014](./adr/0014-github-issue-provider.md) for the original provider
design and [ADR 0018](./adr/0018-remote-trackers-behind-an-orbit-port.md)
for the provider-neutral port this adapter implements.

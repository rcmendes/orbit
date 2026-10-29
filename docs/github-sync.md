# GitHub Issue sync

Orbit can link a Board to one or more GitHub repositories and keep durable Tasks aligned with GitHub Issues through explicit sync. Orbit remains local-first: Task edits stay on the Board; Issue creation occurs only with `orbit github create --apply`, and Orbit persists the Issue link only after GitHub confirms creation. Changes to an existing Issue are published only with `orbit github sync --apply`.

GitHub sync requires Orbit **v0.9.0 or newer**. That release includes `orbit
github` and `orbit triage` in its default binaries, shell/PowerShell installers,
and npm package. If a binary built without the optional `github-provider`
capability loads a configured Board, it stops with an upgrade message instead
of silently ignoring the GitHub integration.

## Quick start

For an organization-owned repository with Issue Fields ready, create a fresh
Board in the repository checkout and authenticate GitHub CLI:

```bash
orbit init
gh auth login
orbit github connect
orbit github status
orbit add "First GitHub task" -p 1
# Copy the ID printed by `orbit add` and replace orb-a7b2c9 below.
orbit github create orb-a7b2c9
orbit github create orb-a7b2c9 --apply
orbit github sync orb-a7b2c9        # inspect the plan; writes nothing
orbit github sync orb-a7b2c9 --apply
```

## Workspace Remote Issue catalog

A Workspace can keep a local catalog of Remote Issues without importing them as
claimable Tasks. From the Workspace root or a child repository, run:

```bash
orbit workspace issues refresh
orbit workspace issues list --repo acme/api --state open --label bug
orbit workspace issues list --text authentication --unlinked
orbit workspace issues inspect acme/api#42
orbit workspace issues import acme/api#42
orbit workspace issues import acme/api#42 acme/web#8 --apply
orbit workspace issues import acme/api#42 --link acme/api#42=orb-a7b2c9 --apply
```

Refresh explicitly reconciles direct child repositories, then paginates through
all open and closed GitHub Issues for included repositories. Pull requests are
excluded. The rebuildable summary cache is stored at
`<workspace>/.orbit/cache/issues.json`, separate from the Workspace Board. The
cache stores stable identities, labels, timestamps, URLs, and GitHub Issue Field
values (including Area and Verification when configured).
Repository failures are reported independently and preserve the previous local
snapshot as stale. Listing reads cached summaries only; inspect fetches the issue
body and comments from GitHub on demand. Catalog refresh performs no GitHub writes
and creates no Orbit Tasks. Catalog entries remain absent from `orbit list`,
`orbit ready`, claims, and Task export until explicitly imported.

Import accepts one or more explicit `OWNER/REPO#NUMBER` identities; it never
selects the whole catalog. Without `--apply`, it previews the Linked Tasks to
create, Local Tasks to link, terminal status mappings, duplicates, and errors.
Add `--apply` after reviewing the selection. New Tasks copy the Issue title,
body, labels, and configured fields, keep the stable repository/number identity,
and map closed Issues to `done` or `cancelled` when the `wontfix` label applies.
Repeat `--link OWNER/REPO#NUMBER=TASK` to pair a selected Issue with a specific
Local Task. Orbit values remain unchanged; the preview lists field differences
before confirmation. An Issue already linked elsewhere, or a Task already linked
to another Issue, is reported as a skipped duplicate. Matching uses repository
and Issue number, never title. Import fetches Issue details but performs no GitHub
writes; imported Tasks follow normal readiness, dependency, hold-label, and claim
rules.

## Detach and reattach

Detach is an explicit local-only lifecycle operation. Both repository and
individual Linked Task commands preview first; `--apply` is required to change
the Board or Workspace catalog:

```bash
orbit workspace detach acme/api
orbit workspace detach acme/api --apply
orbit workspace issues detach acme/api#42
orbit workspace issues detach acme/api#42 --apply
```

Repository detach previews all active Linked Tasks and cached Remote Issue
summaries for that repository. Applying it excludes the repository, changes its
Linked Tasks into ordinary Local Tasks, and moves cached summaries into the
`archived_issues` section of `<workspace>/.orbit/cache/issues.json`. Individual
detach archives only that Issue snapshot. Task title, body, status, labels,
relationships, comments, and last sync baseline remain intact. The stable
repository/Issue number stays in Task frontmatter with `active: false`; it is
inactive provenance, not a sync link. Archived summaries are not returned by
normal catalog listing, and they are never claimable Tasks. Neither detach path
closes, deletes, or otherwise writes to GitHub. Detach and reattach are safe to
repeat.

Reattach also previews first and matches only exact repository/Issue number
identities:

```bash
orbit workspace reattach acme/api
orbit workspace reattach acme/api --apply
orbit workspace issues reattach acme/api#42
orbit workspace issues reattach acme/api#42 --apply
```

Repository reattach previews current sync plans for its inactive Tasks and the
archived snapshots. Applying it re-includes the repository, restores snapshots,
and reactivates those exact links. Task reattach requires its repository to be
included and previews current GitHub values, planned changes, and field-level
conflicts for the matching Local Task. Applying reactivates the link but does not
pull, push, resolve conflicts, or write GitHub. Use `orbit github sync` as a
separate explicit operation to review and resolve the plan. A conflicting
identity on another Task blocks reattachment instead of creating duplicate
links.

`connect` detects the checkout's `origin`. Organization-owned repositories
configure full mode when their Issue Fields are ready. For personal-account
repositories, it leaves the Board unchanged and reports the exact `[github]`
entries to add or merge into the Board's `config.toml`, preserving other linked
repositories; apply the
[basic-mode configuration](#basic-mode-personal-repositories) before creating
or syncing Issues. Basic mode does not sync `area` or `verification`.

## Provider modes

Orbit supports two modes. Existing configuration defaults to `full`, so existing
organization setups keep their current behavior.

### Full mode (organization Issue Fields)

- Requires organization-level Issue Fields named **Area** and **Verification** (names are configurable under `[github.issue_fields]`).
- Run `orbit github setup` to resolve and store the Issue Field GraphQL ids. Use `orbit github setup --validate-only` to check configuration without writing ids.
- Full mode syncs Orbit `area` and `verification` through those organization Issue Fields.

Connect an organization-owned repository from its checkout with `orbit github
connect`. It checks `gh auth status` and repository access, validates the
organization fields, and writes the `[github]` configuration. It preserves other
Board settings and is safe to rerun. Use `orbit github connect --json` for
machine-readable status. To configure the Board manually, use:

```toml
[github]
mode = "full" # optional; full is the default
organization = "acme-corp"
repositories = ["acme-corp/widget"]

[github.issue_fields]
area_name = "Area"
verification_name = "Verification"
```

For manual configuration, run `orbit github setup` to resolve existing Issue Fields and store their GraphQL ids. This default command does not create fields. Use `orbit github setup --validate-only` to check configuration without writing ids.

To let Orbit create missing fields, explicitly run `orbit github setup --create-fields`. It creates **Area** as a single-select field with `board`, `cli`, `mcp`, `setup`, `docs`, and `release` options, and **Verification** as a text field. Existing fields with those names are reused and are not changed. Before creating anything, Orbit verifies that the authenticated `gh` user has an active organization administrator membership. If the membership check fails, refresh the `read:org` scope with `gh auth refresh --hostname github.com --scopes read:org`. If field creation is denied, confirm that `gh` is authenticated as an organization administrator with permission to create organization Issue Fields. An administrator can also create the fields in GitHub and rerun the default `orbit github setup` to store their ids.

### Basic mode (personal repositories)

Basic mode supports personal repositories without organization Issue Fields or
`orbit github setup`:

```toml
[github]
mode = "basic"
repositories = ["alice/widget"]
```

Link repositories, adopt or create Issues, and explicitly sync linked Tasks.
Orbit owns title, body, status, and labels. A GitHub-only edit can be pulled when
the matching Orbit field still equals its saved baseline; an Orbit-only edit is
pushed. Concurrent edits to one field require a reviewed resolution. GitHub
assignees remain informational and are never mapped to Orbit claims.

Basic mode does **not** support organization Issue Fields: `area` and
`verification` are neither imported from GitHub nor pushed to GitHub. Create
and push sync fail with a clear error when a Task has either value, rather than
silently dropping it. Pull-only sync preserves Orbit's local values. Triage
context reports these capabilities as unavailable; Triage review/apply may
change labels, but proposals that change `area` or `verification` are rejected.
`orbit github setup` is unavailable in basic mode. Do not set `organization`,
custom Issue Field names, or Issue Field ids with `mode = "basic"`.

To upgrade, change `mode` to `"full"`, configure `organization` and the field
names, then run `orbit github setup`. Once the Issue Field ids are stored,
create and push sync can publish `area` and `verification`.

### Connect behavior and prerequisites

- GitHub CLI (`gh`) authenticated for the repositories you link.
- Organization-level Issue Fields only when using full mode.

`orbit github connect` reads the current checkout's GitHub `origin`, checks
authentication and access, and detects whether the owner is an organization or a
personal account. For a personal-account repository it leaves configuration
unchanged and reports the exact `[github]` entries for the Board's `config.toml`
to configure basic mode; merge them with existing settings and preserve other
linked repositories. For an organization repository, it validates and
stores the repository and Issue Field configuration. Missing authentication,
repository access, or fields are reported with remediation actions. JSON output
includes `owner_type`, `organization`, `repository`, `field_status`,
`next_action`, and whether the configuration changed. For a personal-account
origin, it also includes `next_actions` and `remediation` with those entries and
a link to the basic-mode setup instructions; human output shows the same
guidance. No configuration is changed by this response.

For manual full-mode configuration, run `orbit github setup` to resolve and
store the Issue Field GraphQL ids. Use `orbit github setup --validate-only` to
check configuration without writing ids.

## Diagnose the integration

`orbit github status` checks the current `gh` authentication identity and scopes,
linked repository access, and—when using full mode—the configured organization
and whether its Area and Verification Issue Fields are present with current stored
ids. It also loads linked Tasks and previews their sync plans to count pending
pull and push changes and detect conflicts. Cross-repository SubTask and
dependency links are reported as unsupported. The command is read-only: it does
not change Board configuration, Tasks, GitHub Issues, or the fixture used by
integration tests. In basic mode, it reports the `repo` scope, skips organization
checks, and marks Area and Verification as not applicable.

Use `--json` for the stable diagnostic structure. `data.pending.pull` and
`data.pending.push` count field changes; `pull_tasks` and `push_tasks` identify the
affected Tasks and counts. `data.conflicts` and `data.conflict_details` report
conflicts, while `data.unsupported_relationships` and `data.unsupported_details`
report relationships Orbit cannot synchronize. `data.next_action` recommends the
next step, and `data.next_command` supplies an executable command when one applies.
`data.auth.scopes_known` distinguishes an unreported scope list from an empty one.
When the GitHub provider is configured,
`orbit doctor` also reports authentication, repository access, and Issue Field
configuration problems alongside normal Board diagnostics.

```bash
orbit github status
orbit github status --json
orbit doctor --json
```

### Tell Local, Linked, and Remote work apart

`orbit list` prints Tasks only. Its `ISSUE` column is `-` for a Local Task, `#N`
for a Linked Task, and `~#N` for detached provenance, so `--linked` and
`--local` filter the table without opening each Task. `--local` includes
detached provenance, which Orbit no longer synchronizes.

`--json` carries the full identity: `github` is `null` for a Local Task and an
object with `repo`, `number`, and `last_sync` for a Linked Task.

```bash
orbit list --linked
orbit list --local
orbit list --json | jq -r '.data.tasks[] |
  [.id, (if .github then "\(.github.repo)#\(.github.number)" else "local" end), .title] | @tsv'
```

Sync state is computed, not stored. `orbit github status` compares each
`last_sync` baseline with the current Task and Issue and reports pending pulls,
pushes, and conflicts. `orbit github sync <TASK>` previews one Task and changes
nothing without `--apply`.

Remote Issues are not Tasks. `orbit workspace issues list --json` prints the
cached catalog with a `linked` flag; `linked: false` means the Issue has no Orbit
Task yet.

## Link repositories

Repositories are linked explicitly; Orbit does not bulk-import an entire repository.

```bash
orbit github repo link acme-corp/widget
orbit github repo list
```

## Adopt or create Issues

- **Adopt** an existing Issue from its URL: `orbit github adopt <task-id> https://github.com/acme-corp/widget/issues/42`
- **Adopt** an Issue with the legacy form: `orbit github adopt <task-id> --repo acme-corp/widget --number 42`
- **Preview** Issue creation for one or more Local Tasks with `orbit github create <task-id> [<task-id> ...]`. Preview shows repository, title, body, labels, Area, Verification, Task status, and the resulting open/closed Issue state. It does not write to Orbit or GitHub.
- **Apply** a reviewed export with `orbit github create <task-id> --apply`. A batch uses an explicit list of Task IDs; it never exports the whole Board. Done and cancelled Tasks require `--include-terminal`.
- Full mode includes Task `area` and `verification` values as organization Issue Fields when configured with `orbit github setup`. Basic mode rejects Tasks with those values instead of dropping them.
- A repository is inferred only when exactly one repository is available. On a Workspace Board, run `orbit workspace refresh`; the included repository catalog is used. On a repository Board, link repositories with `orbit github repo link`. Pass `--repo OWNER/REPO` when the destination is ambiguous.
- GitHub creates each Issue before Orbit persists the stable `owner/repo#number` link and sync baseline. Batch output reports each success or failure. If a Task already has a link, retry is refused and directs you to `orbit github sync`; this prevents a second Issue from being created.
- On a Workspace Board, a successful apply also upserts the new Issue into `<workspace>/.orbit/cache/issues.json`. `orbit doctor` and `orbit workspace issues list --linked` recognize the link immediately without `orbit workspace issues refresh`. The upsert leaves `last_refreshed_at` unchanged; refresh remains the source of truth for catalog completeness.

Preview and apply examples:

```bash
orbit github create orb-a7b2c9
orbit github create orb-a7b2c9 --apply
orbit github create orb-a7b2c9 orb-d4e5f6 --repo acme-corp/widget --apply
orbit github create orb-terminal --include-terminal --apply
```

When using a URL, Orbit reads `owner/repo` and the issue number from the standard GitHub Issue URL. Optional `--repo` or `--number` values can confirm the URL; if provided, they must match it. Without a URL, both legacy options are required. Titles and bodies are not used as identifiers.

On a repository Board, Orbit selects the only linked repository automatically. On a Workspace Board, it selects from the included repositories in the last refreshed Workspace catalog. If zero or multiple repositories are available, refresh/include one or pass `--repo OWNER/REPO`. An explicit destination must be linked on a repository Board or included in the Workspace catalog.

## Sync behavior

Preview (default):

```bash
orbit github sync <task-id>
```

The default preview is human-readable and does not write to Orbit or GitHub. It
shows each linked Task and Issue, the fields planned to be pulled into Orbit or
pushed to GitHub, and any conflicts with both competing values. Empty plans say
that no linked Tasks were found; a linked Task with no pending changes is marked
**No changes needed**. Use `--json` for the existing machine-readable plan and
conflict details. On a Workspace Board, every repository included by the last
`orbit workspace refresh` is available to adopt, status, and sync without a
separate `orbit github repo link`.

Apply changes:

```bash
orbit github sync <task-id> --apply
orbit github sync --apply                 # all linked Tasks
orbit github sync <task-id> --apply --direction pull
```

Rules:

| Field | Authority | Notes |
| --- | --- | --- |
| Title, body | Orbit | A remote-only edit is pulled if Orbit still matches the baseline; local edits push. Same-field edits conflict. |
| Status and open/closed | Orbit status | `todo`/`in_progress` stay open; `done`/`cancelled` close; remote state changes pull only if Orbit status is unchanged. A closed Issue with `wontfix` maps to `cancelled`. |
| Labels | Orbit | Labels are synchronized as an exact field value, not merged. Remote-only edits pull; edits on both sides conflict. |
| `area`, `verification` | Orbit (via Issue Fields) | Full mode only; remote-only edits can pull; Orbit changes push; concurrent edits conflict. Unavailable in basic mode. |
| Assignees | GitHub only | Never mapped to Orbit claims |
| Claims, checkpoints | Orbit only | Never synchronized to GitHub |

Conflicts return `GITHUB_ISSUE_CONFLICT` with per-field details and both values.
No write occurs while any selected Task has an unresolved conflict. Choose a
side for each conflicting field with repeatable `--resolve FIELD=orbit` or
`--resolve FIELD=github`, then require `--apply` to write. `orbit` keeps the
Orbit value and pushes it; `github` pulls the GitHub value. A resolution can be
previewed without writing:

```bash
orbit github sync orb-a7b2c9 --resolve title=orbit
orbit github sync orb-a7b2c9 --resolve title=orbit --resolve labels=github --apply
```

For a batch, one resolution applies to every selected Task that has a conflict
in that field. Sync plans all selected Tasks before writing. Apply then reports
each Task's result independently; a failed Task keeps its previous baseline, so
it can be retried without repeating already successful Tasks' writes.

GitHub command errors include a `Next:` action in human output. With `--json`,
the existing `{ "ok": false, "error": ... }` envelope is unchanged and
`error.details` includes the original `cause` and the same `remediation`, plus
the affected organization, repository, Task, Issue, or fields when known.

- `GITHUB_NOT_CONFIGURED`: in a Workspace, run `orbit workspace refresh`; it
  infers the organization when every child repository shares one owner.
  Otherwise run `orbit github connect` from the checkout with the intended
  GitHub origin; authenticate first with `gh auth login` if needed.
- `GITHUB_SETUP_INCOMPLETE`: run `orbit github setup` to resolve field ids. If
  a named Issue Field is missing, create it under organization **Settings >
  Projects > Issue fields** or run `orbit github setup --create-fields` as an
  organization administrator. Run `orbit github connect` if the organization
  is not configured.
- `GITHUB_REPO_NOT_LINKED`: the error names `OWNER/REPO` and gives the exact
  `orbit github repo link OWNER/REPO` command.
- `GITHUB_ISSUE_CONFLICT`: the error names the Task, Issue, and conflicting
  fields. Review both values with `orbit github sync <task-id>`, choose each
  side with `--resolve FIELD=orbit|github`, then apply with `--apply`.

## GitHub Projects

Orbit treats GitHub Projects as a downstream view over Issues. It does not call
the ProjectV2 API, require a `project` or `read:project` scope, or read or write
Project items, Status, views, or custom fields. Orbit's authority ends at the
Issue identified by `owner/repo#number`.

This boundary is deliberate. A Project can contain Issues from several
repositories, the same Issue can appear in several Projects, and each Project has
its own Status options and custom fields. Orbit keeps one Task link per
`owner/repo#number`, so Project membership never creates duplicate Tasks or a
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
5. Add views for the distinctions Project Status cannot represent, for example
   `label:wontfix` for cancelled work or `is:open` for active work.

The optional GitHub CLI equivalent for creating and linking a Project is:

```bash
gh auth refresh -s project       # only needed for gh project commands
gh project create --owner acme --title "Engineering delivery"
gh project link 1 --owner acme --repo acme/widget
gh project view 1 --owner acme --web
```

Auto-add and status workflows are configured in the Project UI; the `gh project`
command does not configure them. Orbit itself needs no Project scope or
configuration.

Auto-add only runs when an item is created or updated. It does not backfill
Issues that already exist. To add existing Issues, add them manually in the
Project, or use the GitHub CLI:

```bash
gh project item-add 1 --owner acme \
  --url https://github.com/acme/widget/issues/42
```

Do not enable **Auto-close issue** unless Project Status should be able to close
Issues. When that workflow is on, moving an item to `Done` closes the Issue, and
a later `orbit github sync` can pull that close into the Task. That is a GitHub
workflow changing the Issue, not Orbit reading Project Status.

### Example: create an Issue and let the Project pick it up

From a Board linked to `acme/widget`:

```bash
orbit init
gh auth login
orbit github connect
orbit add "Rotate API tokens" -p 1
# Use the Task ID printed by `orbit add` in the next command.
orbit github create orb-a7b2c9 --apply
```

The export output names the new Issue, for example `acme/widget#42`. With the
auto-add workflow enabled, the Issue appears in the Project without any Orbit
Project command. Verify it from the CLI:

```bash
gh issue view 42 --repo acme/widget --json state,labels
gh project item-list 1 --owner acme --field Status --format json
```

The Issue starts open. If the **Item added to project** workflow sets `Status`
to `Todo`, the item appears there; otherwise its Status may stay empty until you
set it. Project Status is not derived from the Issue's open state unless you
configure that workflow.

### Example: track an existing Issue

To attach an existing Issue to a Local Task you already have, adopt it by URL:

```bash
orbit add "Rotate API tokens" -p 1
orbit github adopt orb-a7b2c9 https://github.com/acme/widget/issues/42
```

Adopt copies the Issue's title, body, labels, and status onto the Task and
records the link. It does not create or modify the Issue. To keep the Task's
Orbit values instead, preview a Workspace import link and apply it after review:

```bash
orbit workspace issues import acme/widget#42 --link acme/widget#42=orb-a7b2c9
orbit workspace issues import acme/widget#42 --link acme/widget#42=orb-a7b2c9 --apply
```

On a Workspace Board, an unlinked Issue can become a new Task through the
catalog:

```bash
orbit workspace issues refresh
orbit workspace issues list --repo acme/widget --state open
orbit workspace issues inspect acme/widget#42
orbit workspace issues import acme/widget#42 --apply
```

Import previews the selected Issue and writes only with `--apply`. It creates a
Linked Task with the Issue's title, body, labels, and status; closed Issues map
to `done`, or `cancelled` when labeled `wontfix`. Orbit values then become
authoritative for linked fields, so run `orbit github sync <task-id> --apply`
when you want to publish a local change back.

Auto-add does not backfill that Issue if it predates the workflow. Add it once:

```bash
gh project item-add 1 --owner acme \
  --url https://github.com/acme/widget/issues/42
```

### Example: finish work and move the Project card

Orbit's Project integration is event-driven through normal Issue changes. Close
the Issue with an explicit sync after finishing the Task:

```bash
orbit claim orb-a7b2c9 --by agent
orbit done orb-a7b2c9 --by agent --comment "Shipped and verified"
orbit github sync orb-a7b2c9 --apply
```

`orbit done` changes the local Task to `done`; it does not contact GitHub by
itself. The explicit `github sync` closes Issue `acme/widget#42`. If the Project
has the **Item closed** workflow enabled, GitHub then moves the item to its
`Done` Status. Verify both sides:

```bash
gh issue view 42 --repo acme/widget --json state,labels
gh project item-list 1 --owner acme --query "status:Done" --format json
```

Reopening works in the other direction:

```bash
orbit reopen orb-a7b2c9
orbit github sync orb-a7b2c9 --apply
```

The Issue reopens and the **Item reopened** workflow can return the Project card
to `Todo`. If the Task had been cancelled with `wontfix`, remove that label
explicitly before syncing; reopening does not remove labels:

```bash
orbit label remove orb-a7b2c9 wontfix
```

### Example: keep cancelled work distinct

Orbit closes a cancelled Task's Issue just as it closes a done Task. To preserve
the cancelled distinction in GitHub, add the `wontfix` label before the sync:

```bash
orbit label add orb-a7b2c9 wontfix
orbit cancel orb-a7b2c9 --comment "Superseded by the new token service"
orbit github sync orb-a7b2c9 --apply
```

The Issue is closed with `wontfix`. Orbit does not add that label automatically.
A simple **Item closed** workflow still shows the card as `Done`, so add a
Project view filtered by `label:wontfix` if cancelled work needs its own column
or report.

### What Project Status can and cannot show

A typical mapping looks like this:

| Orbit state | GitHub Issue | Typical Project Status | Limitation |
| --- | --- | --- | --- |
| `todo` | Open | `Todo` from **Item added to project** | None if the workflow is enabled; otherwise Status may be empty |
| `in_progress` | Open | Unchanged | No Issue event distinguishes it from `todo` |
| `done` | Closed | `Done` from **Item closed** | None |
| `cancelled` | Closed, optionally `wontfix` | `Done` from **Item closed** | Distinguish with a label view |

Orbit deliberately does not invent a Project event for `in_progress`. If a
Project needs an `In Progress` column, use a label or another Project workflow
that you control, and keep in mind that a Project Status change does not update
Orbit.

The exception is GitHub's own **Auto-close issue** workflow. If you enable it, a
Project Status change to `Done` closes the Issue. The next pull sync sees a
closed Issue and can set the Task to `done` when Orbit's status has not changed
since the last sync. Even then, Orbit is reacting to the Issue, not reading
Project Status. Without that workflow, moving a card has no effect on Orbit.

### Draft Issues, custom fields, and multiple Projects

- **Draft Issues are not repository Issues.** They have no `owner/repo#number`,
  so Orbit cannot list, import, or link them. Convert the draft to an Issue in
  its repository, then refresh the Workspace catalog before importing it:

  ```bash
  orbit workspace issues refresh
  orbit workspace issues import acme/widget#123
  ```

- **Project custom fields are separate from organization Issue Fields.** A
  Project field named `Area` or `Verification` is not the organization field
  Orbit reads and writes. Renaming or editing the Project field does not change
  the Issue or the Task.

- **Multiple Projects are safe.** The same Issue can be in a delivery board, a
  roadmap, and a release view at the same time. Orbit still keeps one Task link
  for the Issue. Each Project may apply different workflows and show a different
  Status; Orbit does not choose between them.

- **Pull requests are not linked.** A Project can contain pull requests, but
  Orbit links only Issues. A Project item that is a pull request is outside the
  integration.

### Troubleshooting

- **A new Issue is missing from the Project:** confirm the Project is linked to
  the repository and that **Auto-add to project** includes that repository.
  Check the workflow's filter; an auto-add filter can exclude the Issue.
  Auto-add only handles new or updated items, so add existing Issues with
  `gh project item-add`.
- **Closing the Issue did not move the card:** run `orbit github sync
  <task-id> --apply` after `orbit done` or `orbit cancel`. Then confirm the
  Project's **Item closed** workflow is enabled.
- **A card move did not update Orbit:** this is expected unless the Project has
  **Auto-close issue** enabled. Project Status is not an input to Orbit. Close or
  reopen the Issue instead.
- **A cancelled Task appears as Done:** add `wontfix` before syncing and use a
  `label:wontfix` view. A Project Status is a single value, not an Orbit status.
- **`gh project` fails with a scope error:** run `gh auth refresh -s project`.
  This is only for the GitHub CLI's Project commands; Orbit does not need it.
- **A Project field named Area is empty or stale:** Project custom fields do not
  mirror organization Issue Fields. Inspect or edit the Issue Field through the
  Issue and Orbit's sync workflow, not through the Project.

No additional Orbit configuration is required. `orbit workspace issues import`
accepts repository Issue identities, not Project URLs or Project item IDs. See
[ADR 0015](./adr/0015-github-projects-downstream.md).

## Relationships

SubTasks and dependencies are previewed as GitHub sub-issues and blocked-by
relationships when both linked Issues live in the **same** repository. `--apply`
adds supported relationships and removes a same-repository relationship only when
it was present at the last successful sync baseline and has since disappeared
from the Task graph. Cross-repository relationships and remote-only additions
that were never synced are listed as unsupported and preserved; Orbit does not
flatten them or guess which unknown relationship to remove. GitHub assignees
remain separate from Orbit claims.

## Task frontmatter

Linked Tasks store:

```yaml
area: backend
verification: cargo test
github:
  repo: acme-corp/widget
  number: 42
  last_sync:
    at: 2026-10-23T12:00:00Z
    issue_updated_at: 2026-10-23T11:55:00Z
    orbit_updated_at: 2026-10-23T11:50:00Z
    title: "Example"
    body: "Body"
    labels: [bug]
    area: backend
    verification: cargo test
    status: todo
    issue_state: open
```

An active link omits `active` (older Task files remain active by default). A
detached Local Task keeps the same repository, Issue number, and `last_sync`, but
stores `active: false`; Orbit treats that identity as provenance and excludes it
from synchronization until reattachment.

Controlled `area` values match `TaskArea`: `board`, `cli`, `mcp`, `setup`, `docs`, `release`.

## Testing

Integration tests set `ORBIT_GITHUB_FIXTURE` to a JSON file with mock org fields and issues so CI does not call GitHub. Basic mode ignores Issue Field values in fixture Issues.

See [ADR 0014](./adr/0014-github-issue-provider.md).

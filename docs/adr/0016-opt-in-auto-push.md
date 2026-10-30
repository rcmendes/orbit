# ADR 0016: Opt-in automatic push to GitHub Issues

## Context

The GitHub Issue provider is explicitly synchronized. Task edits stay on the
Board, Issue creation happens only with `orbit github create --apply`, and
changes to an existing Issue publish only with `orbit github sync --apply`.
ADR 0014 and the specification describe sync as explicit and list automatic
pushes to a remote as a non-goal.

Users still want ordinary lifecycle work reflected on GitHub without running a
second command: creating a Task, starting it, editing its title or body, and
finishing or cancelling it. A daemon or a fully bidirectional auto-sync would
make every local command depend on remote state, pull other people's edits into
an agent's working copy, and turn each mutation into a conflict surface.

## Decision

- A Board opts in with `[github] auto_push = true` in `config.toml`. The default
  is `false`. It applies to repository and Workspace Boards, in both `basic` and
  `full` modes.
- Auto-push is push-only and synchronous. It never pulls, never resolves a
  conflict by overwriting a remote value, and never creates an Issue for a Local
  Task that predates the current command.
- The trigger is a command that changed at least one GitHub-mapped scalar field:
  title, body, labels, status, `area`, or `verification`. Priority, type,
  `waiting_on`, claims, checkpoints, comments, and dependency relationships do
  not trigger a remote write.
- Detection happens in the command wrapper: `tasks/*.md` is snapshotted before
  and after a write command, and every changed Task with a non-empty mapped
  delta is pushed. Deletions are ignored with a warning when the Task was
  linked. The Board lock is held for the duration of the push.
- `orbit add` auto-creates an Issue when exactly one repository is available:
  linked on a repository Board, or included in a Workspace catalog. With zero or
  multiple repositories the Task stays local and the command warns. A failed
  creation is not retried automatically; `orbit github create <id> --repo ...`
  is the recovery.
- `todo` and `in_progress` both map to an open Issue. Claiming adds the reserved
  GitHub label `orbit:in-progress`; release, `done`, `cancel`, and reopen to
  `todo` remove it. The label is managed separately from user Labels, excluded
  from Label sync in both directions, and never read back as a Status.
- `done` closes with `state_reason: completed`; `cancelled` closes with
  `state_reason: not_planned`; reopening a terminal Task reopens the Issue.
  Native `state_reason` is authoritative. `wontfix` remains a pull fallback for
  older Issues and is never added automatically.
- A field changed on both sides is not pushed. Auto-push warns and leaves the
  sync baseline unadvanced, so the conflict stays visible to an explicit
  `orbit github sync`. The local mutation always succeeds; remote failures and
  conflicts are warnings, not command errors.
- The baseline advances only when the whole mapped field push succeeds. A
  reserved-label provisioning failure is warned and retried separately; it does
  not hold back a successful field push. If the local mutation's Git commit
  failed, auto-push is skipped rather than publishing state outside Board
  history.
- `orbit github *`, Workspace issue import/detach/reattach/include/exclude,
  `orbit sync`, `orbit doctor`, `orbit init`/`link`/`unlink`, migrations, and
  `orbit delete` do not auto-push. `orbit triage apply` does, because it changes
  mapped fields.
- Enabling the flag backfills nothing. Existing Local Tasks need
  `orbit github create`; existing `in_progress` Tasks receive the reserved label
  on their next mapped change or explicit sync. `orbit github status` reports
  pending changes and drift, and `orbit doctor` reports a missing reserved label
  or the absence of a linked repository.
- `--no-sync` and `ORBIT_NO_AUTO_SYNC=1` suppress auto-push. Explicit
  `orbit github sync` is never suppressed. CLI, MCP, and hooks share the same
  policy.

## Considered options

- **Bidirectional auto-sync.** Rejected: pulls remote edits into local work,
  makes every mutation conflict-prone, and is a much larger product.
- **A background watcher or queue.** Rejected: the specification excludes
  daemons and watchers; only synchronous inline push fits the current process
  model.
- **Auto-create on the next field change for an existing Local Task.** Rejected:
  retroactive Issue creation is surprising. Creation belongs to `orbit add` or
  explicit `orbit github create`.
- **Using `wontfix` as the cancellation signal.** Rejected in favor of native
  `state_reason`, which does not pollute user Labels.
- **Storing `orbit:in-progress` in Task labels.** Rejected: a remote Label edit
  could then contradict Task status, and `orbit label remove` could desync the
  two.
- **Mapping priority or type into GitHub labels.** Out of scope; those fields
  have no remote representation today.

## Consequences

- ADR 0014's "sync is explicit" invariant and the specification's automatic
  push non-goal are amended for opted-in Boards. Auto-push changes GitHub
  Issues only; the Board's `orbit` Git branch is never pushed automatically.
- Every opted-in mutation can hold the Board lock while `gh` calls run, which
  serializes other agents for the duration. Auto-push is opt-in, and a queue
  remains a non-goal.
- A successful push that advances the baseline produces a second Board commit.
  With `auto_commit = false`, only the files change.
- `orbit github status`, `doctor`, help text, `github-sync.md`, `commands.md`,
  the README, and the changelog must describe the opt-in behavior. `Linked Task`
  in the glossary no longer promises that GitHub stays unchanged until an
  explicit sync.

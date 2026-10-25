# 0012. One Orbit folder for every Board

- Status: Accepted
- Date: 2026-10-25

## Context

Orbit currently has two board shapes. A repository-local Board is a hidden
worktree at `<git-common-dir>/orbit/board`; a workspace Board is an independent
repository at `<workspace>/board`, created by `orbit workspace`. The location is
an implementation detail that leaks into every explanation, backup script, and
agent configuration. Editors and humans look for the board and find a directory
buried under `.git`. Agent configs record the absolute board path, so moving or
re-linking a board silently leaves every integration stale.

[ADR 0001](./0001-markdown-board-on-orphan-branch.md) still requires a dedicated
orphan branch, shared by every worktree, with task churn outside code PRs. [ADR
0002](./0002-single-writer-lock-and-atomic-rename.md) requires one stable lock
and atomic publication. [ADR 0006](./0006-shared-board-across-repositories.md)
allows a Board shared by several repositories through an absolute pointer. [ADR
0008](./0008-workspace-root-directory.md) introduced `orbit workspace` and a
second layout. The two creation commands, `init` and `workspace`, now do almost
the same job at different anchors and produce different shapes.

This decision replaces ADR 0008 and amends ADR 0001, ADR 0002, and ADR 0006. It
does not change Task IDs, Markdown-as-truth, the orphan branch, or the JSON
contract.

## Decision

**A Board lives in one delegated directory, DIR, called the Orbit folder.** DIR
contains `config.toml`, `tasks/`, and `comments/` at its root. How that directory
is backed by Git is private to Orbit. Every command takes DIR the same way: the
positional argument of `init`, the argument of `link`, and `--dir`/`ORBIT_DIR`.

### One creation command

```text
orbit init [DIR] [--slug SLUG] [--global]
           [--agents LIST|none|all] [--print]
```

- The default DIR is `./.orbit` in the current directory.
- `orbit workspace`, `init --board PATH`, and `init --local` are removed. Orbit
  is pre-alpha, so there is no compatibility alias. A workspace is now created
  from its folder with `orbit init`; `link` is unchanged as the joining step.
- `init --global` targets `~/.orbit`. It conflicts with a positional DIR and
  with `--dir`/`ORBIT_DIR`.
- `init` takes DIR from the positional argument, then `--dir`/`ORBIT_DIR`, then
  `./.orbit`. `link` uses only its positional DIR; the global overrides never
  retarget the pointer.
- A repository owns one local Board. A nested repository (for example a
  submodule) has its own resolution scope and may resolve its own nested Board,
  but a second `.orbit` in the same working tree is refused because the `orbit`
  branch can be checked out by only one worktree.
- `init` does not wire agents by default: it creates DIR, the Board, the slug,
  and the exclusion entry when DIR is inside a working tree, then prints the next
  step. It writes agent files only when the user confirms the prompt in a
  terminal or passes `--agents`.
- `--agents` accepts `none`, `all`, or a comma-separated client list. Without a
  flag, a TTY asks whether to wire detected harnesses; without a TTY, `init`
  prints the hint and writes nothing. `--print` writes nothing in either case.
- `init` refuses to create a Board that would compete with an existing
  repository-local resolution for DIR. If the target DIR already holds a Board,
  it reuses it; if a different repository-local Board already resolves for DIR,
  it returns `ALREADY_INITIALIZED` naming that Board before creating anything.
  `orbit unlink` clears a link; an ancestor, main-checkout, or legacy Board must
  be moved or migrated. `--global` and the `~/.orbit` fallback are exempt; the
  fallback never blocks a local Board. A DIR outside every working tree is a
  standalone Board; it becomes part of a repository only through `link`.

### Storage modes

| DIR location | Storage | What Orbit does |
| --- | --- | --- |
| Inside a Git working tree | `worktree` | Adds DIR as a worktree of the repository on the existing or newly created `orbit` branch. Every worktree of that repository shares the same Board. |
| Outside every Git working tree | `repository` | Makes DIR its own Git repository on the `orbit` branch. The global Board and shared Boards outside every working tree use this mode. |

The mode is recorded in `config.toml` as `board.storage = "worktree"` or
`"repository"`. Both modes expose the same files, the same `<DIR>/.lock`, the
same commands, and the same JSON contract; only Git plumbing differs. An
incoherent mode is a `doctor` finding (`STORAGE_MISMATCH`), not a second
behavior.

A worktree-mode `init` writes the DIR path to the containing repository's
`.git/info/exclude`. It never edits a tracked `.gitignore`, because committing
the ORBIT folder would put task churn on the code branch (ADR 0001). A forced
`git clean -xdff` can remove DIR, but the `orbit` branch still holds every Task.
The next `init` or `doctor --fix` prunes the stale worktree record and re-attaches
DIR with no task loss.

### Slug

The slug is derived from DIR, not from the shell's current directory:

| DIR | Slug |
| --- | --- |
| `.../<name>/.orbit` | `<name>`, normalized |
| `.../<name>` | `<name>`, normalized |
| `~/.orbit` or `--global` | `orb`, with a warning |

Explicit `--slug` keeps the strict validation from ADR 0011. Re-running `init`
preserves the stored prefix; an empty Board can change it, and a populated Board
still returns `SLUG_LOCKED` pointing at `orbit slug migrate`.

### Discovery

Every command resolves the Board in this order. The first match wins.

1. `--dir` or `ORBIT_DIR`, which names the Orbit folder itself (or a legacy
   Board root) directly. The resolver does not append `.orbit`.
2. The absolute pointer at `<git-common-dir>/orbit/link`. A legacy target — an
   old board root or a workspace root containing `board/` — resolves with a
   `LEGACY_LAYOUT` warning.
3. The nearest `.orbit` walking up from the current directory to the enclosing
   repository root, or to `$HOME` outside a repository, skipping `~/.orbit`
   itself.
4. The main checkout's `.orbit`. The main checkout is the parent of the Git
   common directory when that directory is literally `.git`; otherwise it is the
   repository root.
5. The legacy local layout at `<git-common-dir>/orbit/board`, with a
   `LEGACY_LAYOUT` warning.
6. `~/.orbit`, only when it already exists. Discovery never creates it.

A pointer whose target no longer holds a Board returns `LINK_NOT_FOUND`. A
missing Board with no pointer returns `BOARD_NOT_FOUND` and names `orbit init`.
When a new `.orbit` and a legacy local Board both exist, commands use the new
`.orbit` and emit an `AMBIGUOUS_BOARD` warning; `doctor` reports
`AMBIGUOUS_BOARD` until the half-moved state is resolved.

### Links and shared Boards

`orbit link DIR` writes the absolute DIR path to
`<git-common-dir>/orbit/link`. Linking does not delete a local Board; it shadows
it and warns. Because the pointer lives in the common directory, every linked
worktree resolves the same Board and serializes on the same `<DIR>/.lock`.
`orbit unlink` removes the pointer and returns the repository to the main
checkout's `.orbit`, if one exists.

A shared Board is therefore just a DIR outside every member repository plus one
pointer per repository. Cross-repository Dependencies keep working because every
member resolves the same task graph.

### Agent configuration is path-free

`orbit setup` gains `--scope project|user`, defaulting to `project`. Project scope
writes the existing repository-level files. User scope writes the harness's
user-level instruction and MCP locations; `--print` names the exact files. In
both scopes, generated configs no longer record the absolute Board path. They
rely on discovery from the project directory, or on `--dir`/`ORBIT_DIR` when an
editor launches the server elsewhere.

`doctor` reports older managed configs that still contain an absolute Board path
(`ABSOLUTE_BOARD_PATH`); re-running `setup` rewrites them path-free.

### Doctor

`doctor` runs without a Board. It reports the discovery trace and can diagnose:

| Finding | Meaning |
| --- | --- |
| `LEGACY_LAYOUT` | Resolution used an old hidden board, an old workspace shape, a legacy pointer target, or an old board root passed explicitly. |
| `AMBIGUOUS_BOARD` | Both a new `.orbit` and a legacy local Board exist; a migration was interrupted or duplicated. |
| `ORPHAN_WORKTREE` | The `orbit` branch still has a worktree record but DIR is missing. |
| `STORAGE_MISMATCH` | `board.storage` disagrees with DIR's actual Git shape. |
| `ABSOLUTE_BOARD_PATH` | A managed agent config still records a Board path. |

`doctor --fix` keeps its existing repairs (stale temp files) and re-attaches an
orphan worktree. `doctor --migrate` is the explicit, resumable legacy migration:
it re-attaches a local hidden Board at `<main>/.orbit`, moves a shared
`<workspace>/board` to `<workspace>/.orbit`, keeps an arbitrary `--board` root
outside every working tree and records `repository` mode, moves other roots to a
DIR that matches their storage mode, verifies every Task, removes the old
worktree or directory, and updates the current repository's pointer. Interrupting
it leaves at least one readable layout; re-running it resumes. Other linked
repositories may need to run `link DIR` again if their pointer named the old
root.

Legacy resolution is temporary. It is removed in the release after this change,
together with the old pointer shapes and the extra discovery step.

## Considered options

- **Keep `<git-common-dir>/orbit/board` and `<workspace>/board`.** Status quo.
  Two shapes, a hidden path that humans and editors cannot find, and agent
  configs that must hardcode it.
- **Put the Board at `<DIR>/.orbit/board`.** Keeps a visible anchor but adds a
  level that hides nothing, invalidates every existing pointer twice, and splits
  the notion of DIR between anchor and Board.
- **Commit `.orbit/` on the code branch.** Rejected by ADR 0001: task churn
  enters every PR and follows whichever branch is checked out.
- **A per-worktree `.orbit`.** Rejected by ADR 0001: agents in different
  worktrees diverge and stop coordinating.
- **Always use a standalone repository, even inside a project.** Loses automatic
  sharing across worktrees and forces a `link` step for the common case. The
  worktree mode is the reason a local Board works at all.
- **Keep `workspace` as an alias for `init DIR`.** Two names for one operation
  with different layouts; removing it in pre-alpha is cheaper than carrying the
  compatibility surface forever.
- **Make `~/.orbit` the default Board.** Turns every product's queue into global
  state and makes repository boundaries meaningless. It is opt-in and the last
  discovery fallback only.
- **Move `config.toml` outside DIR.** Breaks cloneable Boards and lets a linked
  repository disagree with the Board it uses; the configuration stays inside.
- **A registry of boards.** ADR 0006 rejected global state; the pointer plus the
  conventional `.orbit` name are enough.

## Consequences

- The default board path is visible and stable: `<project>/.orbit`. Backup,
  inspection, and documentation can name it without knowing Git internals.
- A fresh clone has no `.orbit` and no exclusion entry. `orbit init` recreates
  both; tasks return from the `orbit` branch only when the clone carries that
  branch.
- The `.git/info/exclude` entry is per clone and untracked. That is deliberate:
  no tracked file changes, no task churn in PRs.
- `git clean -xdff` can delete DIR. Task data is not lost because it lives on the
  branch; `init` or `doctor --fix` re-attaches it. `git clean -xdf` skips the
  nested repository, as the experiment showed.
- Moving DIR invalidates absolute link pointers. Re-link affected repositories;
  path-free agent configs no longer need refreshing.
- `orbit setup --scope user` makes a machine-wide integration possible without
  binding it to one Board.
- Scripts that used `orbit workspace`, `init --board`, `init --local`, or passed
  a workspace root to `link` must switch to `init DIR`, `init --global`, or
  `link DIR`. This is an intentional pre-alpha break.
- Legacy boards keep working with a warning until the following release, when
  the compatibility path is removed.

## Implementation note

The first implementation of unified `init` tightened the nested-scope wording:
one repository owns one local Board, because the `orbit` branch can be checked
out by only one worktree. A nested repository (such as a submodule) retains its
own resolution scope; a second `.orbit` in the same working tree returns
`ALREADY_INITIALIZED`.

## Amendments

- **ADR 0001:** the Board is still an orphan `orbit` branch checked out from a
  worktree shared by every working tree. Its path is now DIR instead of
  `<git-common-dir>/orbit/board`, and a standalone Board may be its own
  repository. `ORBIT_DIR` names DIR.
- **ADR 0002:** the single data lock moves to `<DIR>/.lock`, stable for the
  lifetime of the Board in both storage modes. Creation uses a separate
  short-lived bootstrap lock.
- **ADR 0006:** the link pointer and absolute target remain, but the target is
  DIR. The only user-level Board is the opt-in `~/.orbit` fallback.
- **ADR 0008:** replaced. There is no `orbit workspace` and no
  `<workspace>/board`; a workspace is a DIR plus the repositories linked to it.

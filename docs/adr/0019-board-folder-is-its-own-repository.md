---
status: accepted
---

# ADR 0019: Every Board folder is its own Git repository, with an optional Board host

A Board sits inside one repository, sits above several, or belongs to none, and
the user chooses per Board whether its history stays on the machine or is stored
in one chosen repository. We decided that every Board folder is a standalone Git
repository on branch `orbit`, and that storing it remotely means giving it one
Board host: a remote, pointing at one of its Repositories or another repository,
that Orbit pushes `orbit` to. This replaces the `worktree` storage mode of ADR 0001
and ADR 0012, where a Board inside a repository was an orphan branch of that code
repository.

Code repositories never change because of a Board: Orbit hides a Board folder
inside a working tree only through `.git/info/exclude`, never `.gitignore`.
Syncing with the Board host is explicit by default (`orbit board sync`), with
opt-in automatic sync. Divergent histories merge with Orbit's own rules:
different Tasks or fields merge, Comments union, a same-field conflict stops for
a per-field choice, the earlier of two Claims on one Task wins, and pushed
history is never rewritten. Claims are atomic per machine; across machines they
are as fresh as the last sync.

## Considered options

- **Keep the orphan branch inside the code repository.** Rejected: a local-only
  Board leaks with `git push --all` or `--mirror`, and single-repo and multi-repo
  Boards need two storage paths.
- **Commit the Board folder on the code branch.** Rejected: each worktree checks
  out its own copy, so two agents can claim the same Task. On a protected
  default branch, every Task change would need a pull request. Switching
  branches would rewrite the Board under running agents.
- **Task content on the code branch, claims kept outside Git.** Rejected: a
  different product in which planning needs pull requests and Tasks finish only
  when code merges.

## Consequences

- Task changes never enter code branches or code pull requests. A hosted Board
  is browsable on the host's `orbit` branch.
- `git log orbit` from a code repository no longer shows Board history; it lives
  in the Board folder.
- Existing worktree-mode Boards need a migration that copies their `orbit`
  branch into a standalone Board folder.
- The Storage mode concept is retired.

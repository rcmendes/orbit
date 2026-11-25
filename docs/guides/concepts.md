# Concepts

The mental model: what an Issue is, what Ready means, and why the board is just Markdown in Git.

Orbit keeps the vocabulary small on purpose. Everything else is derived.

| Concept | Meaning |
| --- | --- |
| **Issue** | One unit of work. A Markdown file under `tasks/`. This is what external trackers call an issue. |
| **sub-issue** | An Issue with a `parent`. It is claimable and completable in its own right, not a checklist item. |
| **Parent Issue** | An Issue that groups sub-issues. Its completion waits for its children. |
| **Type** | Issue type from the Board's `board.types` (`epic`, `story`, `task`, `bug`, `sub-task` by default). It decides where in `board.hierarchy` the Issue may sit. |
| **Status** | Stored lifecycle position: a name from the Board's workflow, bucketed into a category (`todo`, `active`, `done`, `canceled`). |
| **Priority** | `P0` (highest) through `P3` (lowest), stored as `0`–`3`. |
| **Label** | A lowercase string for filtering and workflow state. |
| **Hold Label** | A label that suspends readiness. Defaults: `needs-triage`, `needs-info`, `ready-for-human`. |
| **Dependency** | Another Issue in a closed status category before this one is Ready. |
| **Claim** | Exclusive, time-bounded responsibility for an Issue, held by one agent. |
| **Lease** | The time window during which a Claim is valid. Default: 30 minutes. |
| **Checkpoint** | The claim holder's latest handoff note: current progress, the next useful action, and any blocker. It survives release and takeover but never affects readiness. |
| **Ready** | Derived: in the workflow's ready status, unclaimed, not Waiting, no Hold Label, every Dependency resolved, and every sub-issue resolved. Never stored. |
| **Waiting** | An external blocker recorded as a blocking Link to an external reference. Not a Status. |
| **Comment** | An append-only, authored, timestamped note attached to an Issue. |

The full definitions, including the words Orbit avoids, are in [GLOSSARY.md](../../GLOSSARY.md).

## Board anatomy

The Board is real Git history in its own repository, anchored by the Orbit folder:

```text
<project>/.orbit/                       # the Orbit folder: anchors the Board
├── board/                              # DIR: a Git repo on branch `orbit`
│   ├── config.toml                     # branch, lease, priorities, hold labels
│   ├── issues/
│   │   ├── orb-a7b2c9.md               # a top-level Issue
│   │   └── orb-a7b2c9.d4b3df.md        # a sub-issue
│   └── comments/
│       └── orb-a7b2c9.md               # append-only thread
└── cache/                              # rebuildable; never in the Board repo
```

Every Board is a standalone Git repository on the `orbit` branch (ADR 0019). Inside a Git working tree the hidden Board lives at `./.orbit/board`, anchored by the `./.orbit` Orbit folder; outside Git an anchor folder stores it at `<anchor>/.orbit/board`. The Orbit folder also holds the rebuildable `cache/` and merged-child `backups/`. `--visible[=NAME]` instead places the Board in a visible folder that `.orbit/board` points to. All layouts expose the same Board files and CLI contract.

Issues are Markdown with YAML frontmatter:

```yaml
---
title: Build the CLI router
status: ready
type: issue
priority: 2
labels:
- ready-for-agent
links:
- type: blocks
  target: orb-f14c02
created_at: 2026-10-23T11:00:00Z
updated_at: 2026-10-23T12:00:00Z
---

Body text, acceptance criteria, and notes.
```

The filename is the ID. There is no `id` field in frontmatter. A sub-issue ID is the parent ID, a dot, and a fresh suffix (`orb-a7b2c9.d4b3df`), with one dot per level. A claimed Issue may also carry one `checkpoint` block with `by`, `at`, `progress`, and optional `next` and `blocker`; Orbit replaces it in place and it never changes readiness.

## Why a separate repository?

Issue churn does not belong in your feature PRs. Every Board is its own Git repository on the `orbit` branch, so an agent can update a ticket while a human reviews code without either change polluting the other. A Board created at the main checkout's `.orbit` is resolved by every linked worktree, so a claim made from a worktree and a claim made from the main checkout serialize on the same lock and see the same issues.

## Safety without a server

- **One writer at a time.** Mutations take a Board-wide exclusive lock; readers take it shared for a consistent snapshot.
- **Atomic publishes.** Each file is written to a temp file, fsynced, renamed, and the directory is fsynced on Unix. A crash cannot leave a half-written issue.
- **No persistent index.** Every command rebuilds the graph from Markdown. There is no cache to corrupt and no watcher to miss an event.
- **Auto-commit with an escape hatch.** Meaningful mutations commit to the `orbit` branch. Heartbeats are lease-only and wait for the next mutation or `orbit sync`. The branch is pushed only to a configured Board host (`orbit board sync` or `board.host_sync = "auto"`); opted-in Remote Issue auto-push is separate.
- **Typed failures.** `LOCKED` is retryable; dependency, claim, label, and parent failures carry the IDs you need to resolve them.

# Agent setup

Wire Claude Code, Cursor, Codex, OpenCode, and Devin to the board: instruction files, MCP servers, and SessionStart hooks.

`orbit init` creates a Board at `./.orbit/board` inside a Git repository or at `<anchor>/.orbit/board` outside one. It asks once, in a terminal, whether to wire the harnesses it detects; without a terminal it writes no agent files and prints the next step. On the first MCP setup it also asks whether generated configs should use an absolute path (the default) or the portable `orbit` command, which requires Orbit on the editor's `PATH`. Instruction files use managed markers and MCP configs are merged at the key level, so re-running is safe and `--remove` deletes only what Orbit added.

```bash
orbit init                    # repo Board, or anchor Board outside Git
orbit init --agents all       # wire every supported harness
orbit init --agents none      # Board only; no agent files
orbit init --print            # preview without writing
orbit init --portable-mcp     # MCP command `orbit`; editor PATH must find it
orbit init ~/src/acme          # create an anchor Board
(cd ~ && orbit init)         # ordinary Board at ~/.orbit/board
```

Use `orbit setup` to wire, repair, or adjust a harness later. `--scope project` (the default) writes repository-level files; `--scope user` writes the harness's user-level locations:

```bash
orbit setup claude --hooks   # CLAUDE.md + SessionStart hook + MCP
orbit setup cursor           # .cursor/rules/orbit.mdc + skill + MCP
orbit setup codex            # AGENTS.md + skill + MCP
orbit setup opencode         # AGENTS.md + MCP
orbit setup devin            # .devin/ rule + skill + MCP
orbit setup mattpocock       # docs/agents guides + mattpocock profile
orbit setup all --scope user # machine-wide instruction and MCP files
orbit setup all --check      # installed / missing / stale
orbit setup all --no-mcp     # instruction files only
orbit setup all --portable-mcp # MCP command `orbit`; editor PATH must find it
orbit setup all --remove     # remove only Orbit-managed content
```

`orbit setup mattpocock` writes `docs/agents/issue-tracker.md` and
`docs/agents/triage-labels.md` with the sections the upstream skills look up
by heading (Conventions, the publish/fetch sections, and Wayfinding
operations) inside a managed marker block; a hand-written file without the
marker is skipped, and the warning names the missing headings. Profiles are
also applied through `setup`: when a different profile already owns the
Board's `[workflow]`, applying another workflow-bearing profile fails with
`PROFILE_CONFLICT` naming it — nothing is written.

**Core `setup` parameters**

| Parameter | Use it when | Example |
| --- | --- | --- |
| `--scope project` | Default. Write instruction and MCP files for this repository. | `orbit setup claude --scope project` |
| `--scope user` | One machine-wide integration; writes the harness's user-level locations. | `orbit setup cursor --scope user` |
| `--check` | CI or a repair check: report installed, missing, or stale and exit non-zero on drift. | `orbit setup all --check` |
| `--remove` | Uninstall only what Orbit manages, leaving unrelated configuration alone. | `orbit setup all --remove` |
| `--no-mcp` | Write instruction and skill files without MCP entries. | `orbit setup codex --no-mcp` |
| `--hooks` | Add the Claude or Devin SessionStart hook. | `orbit setup claude --hooks` |
| `--portable-mcp` | Write bare `orbit` in MCP configs; the editor's PATH must resolve it. | `orbit setup opencode --portable-mcp` |
| `--print` | Preview the exact files that would be written or removed. | `orbit setup all --print` |

Generated configs record the Orbit binary but not the Board path, so moving the Orbit folder does not leave them stale. MCP entries use an absolute binary path by default; `--portable-mcp` writes `orbit` instead, which only works when the editor's `PATH` resolves it. `doctor` reports an older config that still contains an absolute Board path.

A hand-written Matt Pocock guide is respected. If `docs/agents/issue-tracker.md` or `docs/agents/triage-labels.md` exists without Orbit's managed marker, setup reports it as `skipped` and leaves it untouched; `--remove` does not delete it.

CI runs `orbit setup opencode --no-mcp --check` on every push and pull request, so the checked-in `AGENTS.md` managed block cannot drift from the generator. `--no-mcp` keeps the machine-local `opencode.json` out of the check.

## SessionStart hooks

`orbit setup claude --hooks` installs a SessionStart hook in `.claude/settings.json`; `orbit setup devin --hooks` installs one in `.devin/hooks.v1.json`. Each hook records the absolute Orbit binary and runs:

```bash
orbit prime --hook-json --by claude   # or --by devin
```

`prime` prints the agent workflow, board counts, top ready issues, close candidates, and stale claims — about 1k tokens of starting context. `--hook-json` wraps the same text in the SessionStart envelope both harnesses read. `--max-tokens N` caps the rendered context at roughly N tokens (~4 characters each), trimming listings first; the JSON output reports `budget.truncated` when the fixed instructions alone exceed the budget.

Because generated MCP commands are machine-specific, keep those config files untracked when possible. If one is committed, regenerate it after installing Orbit elsewhere, or use `--portable-mcp` so the committed command stays `orbit`.

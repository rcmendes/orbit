# Orbit: an AI-native issue tracker

**Status:** pre-alpha; M0 (core CLI), M1 (agent setup), M2 (MCP), and M3 (release tooling) are implemented. No public release has been tagged yet. The canonical design is in [SPEC.md](./SPEC.md); vocabulary is in [GLOSSARY.md](./GLOSSARY.md). Requires Git 2.31+ (Rust 1.89+ only when building from source).

### Brief description

Orbit is a zero-daemon, AI-native local issue tracker built in Rust. Tasks are plain Markdown files on a dedicated Git branch, served through a strict `--json` CLI and an optional MCP server. A single locked writer and atomic renames make concurrent agents safe without a database server.

### The goal

Eliminate context-window bloat and pipeline freezes in multi-agent workflows. Orbit gives agents a deterministic ready queue with atomic claims, expiring leases, dependency validation, and a strict error contract — while keeping every task change human-readable and reviewable in Git.

### The pitch

When building micro-SaaS products at maximum velocity, your autonomous toolchain — Cursor, Claude Code, Codex, OpenCode, Superpowers — needs a central nervous system. Standard issue trackers force LLMs to burn thousands of tokens parsing flat files or opaque databases, and database-backed trackers put task state in a binary that code review cannot see.

Orbit keeps the board as Markdown, but compiles it into an in-process graph on every command. Agents ask `orbit ready --claim` for exactly one piece of work, hold a time-bounded lease, and get structured errors when a dependency is unfinished or another agent already owns the task. Writes serialize on one repo-wide lock and publish through temp-file plus rename, so a crash cannot truncate a task and two agents cannot corrupt the queue. Installation is a single binary, with shell/PowerShell installers and an npm wrapper once a release is tagged. There is no daemon, no watcher, and no Dolt.

### Quick start

```bash
cargo install --path .
orbit init
orbit setup claude --hooks
orbit setup claude --mcp
orbit add "Build the CLI router" -p 1
orbit ready --claim --by claude
orbit heartbeat orb-a7b2c9 --by claude
orbit done orb-a7b2c9 --by claude
```

### Install

Prebuilt binaries and installers are produced by the release workflow; these links work once the first `v*` tag is published.

```bash
# macOS / Linux
curl --proto '=https' --tlsv1.2 -LsSf https://github.com/rcmendes/orbit/releases/latest/download/orbit-tracker-installer.sh | sh

# Windows PowerShell
irm https://github.com/rcmendes/orbit/releases/latest/download/orbit-tracker-installer.ps1 | iex

# npm / npx
npx @rcmendes/orbit --version
npm install -g @rcmendes/orbit
```

The npm package is `@rcmendes/orbit`; the shell and PowerShell installers use the Cargo package name in their filenames (`orbit-tracker-installer.*`). See [RELEASING.md](./RELEASING.md) for the release process.

### Agent setup

`orbit setup` writes project-local instruction files with managed markers, so re-running it is safe and `--remove` removes only what Orbit added.

```bash
orbit setup claude --hooks   # CLAUDE.md + SessionStart hook
orbit setup cursor           # .cursor/rules/orbit.mdc + skill
orbit setup codex            # AGENTS.md + skill
orbit setup opencode         # AGENTS.md
orbit setup mattpocock       # docs/agents/issue-tracker.md
orbit setup all --hooks --mcp  # everything, including MCP configs
orbit setup all --print      # preview without writing
orbit setup all --remove     # remove only Orbit-managed content
```

`setup --mcp` writes `.mcp.json` (Claude), `.cursor/mcp.json` (Cursor), `.codex/config.toml` (Codex), and `opencode.json` (OpenCode) with an `orbit` server entry. The server entry passes `--dir <board>` when the board is external. The Claude hook runs `orbit prime --hook-json --by claude`, which injects a compact workflow reminder and the current ready queue at session start. It assumes `orbit` is on `PATH`.

### MCP

```bash
orbit mcp
```

`orbit mcp` speaks JSON-RPC 2.0 over stdio and exposes 11 tools: `ready`, `list`, `get`, `create`, `update`, `claim`, `release`, `heartbeat`, `finish`, `link`, `delete`. It also serves `orbit://tasks/{id}` as a Markdown resource. Tool calls reuse the CLI's lock, atomic writes, auto-commit, and error envelope. See [SPEC.md](./SPEC.md) section 9.

### Why not Beads or Beans?

- **Beans** proved that Markdown-as-truth is pleasant for humans, but it has no atomic claims, leases, or dependency-aware ready queue.
- **Beads** proved that agents need claims, ready-work detection, priorities, and hash IDs, but its source of truth is a Dolt database and its multi-writer mode needs a server.

Orbit is the narrow overlap: Beans' human-readable storage with Beads' queue semantics, in one static Rust binary with no daemon.

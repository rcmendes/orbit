#!/usr/bin/env bash
# Install smoke: exercise detected agent setup and MCP with a real binary.
set -euo pipefail

script_dir="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
handshake_input="$script_dir/onboarding-smoke-mcp.jsonl"

resolve_binary() {
    local candidate="$1"
    if [[ "$candidate" != */* ]]; then
        candidate="$(command -v "$candidate")" || {
            echo "onboarding smoke: $1 is not on PATH" >&2
            exit 1
        }
    fi
    if [[ "$candidate" != /* ]]; then
        candidate="$(cd "$(dirname "$candidate")" && pwd)/$(basename "$candidate")"
    fi
    printf '%s' "$candidate"
}

orbit="$(resolve_binary "${ORBIT_BIN:-orbit}")"
if [[ ! -x "$orbit" ]]; then
    echo "onboarding smoke: $orbit is not executable" >&2
    exit 1
fi

workdir="$(mktemp -d)"
smoke_home="$(mktemp -d)"
cleanup() {
    rm -rf "$workdir" "$smoke_home"
}
trap cleanup EXIT

# Keep detection and setup away from the developer's real agent configuration.
export HOME="$smoke_home"
export USERPROFILE="$smoke_home"
unset ORBIT_DIR
cd "$workdir"

git init -q .
git config user.email smoke@orbit.invalid
git config user.name "Orbit Smoke"

# A project-local marker guarantees at least one detected harness without
# depending on the isolated home directory.
mkdir -p .claude

"$orbit" --version
"$orbit" init --agents auto --slug orb --json > init.json
grep -q '"claude"' init.json
"$orbit" add "Smoke task" --json > /dev/null
"$orbit" ready --json > /dev/null

# Check re-resolves the running binary, so a stale or foreign MCP command fails here.
"$orbit" setup claude --check --json > /dev/null

orbit_entry="$(
    awk '
        /"mcpServers"[[:space:]]*:[[:space:]]*\{/ { servers = 1 }
        servers && /"orbit"[[:space:]]*:[[:space:]]*\{/ { capture = 1 }
        capture {
            print
            for (i = 1; i <= length($0); i++) {
                character = substr($0, i, 1)
                if (character == "{") depth++
                else if (character == "}") depth--
            }
            if (depth == 0) exit
        }
    ' .mcp.json
)"
if [[ -z "$orbit_entry" ]]; then
    echo "onboarding smoke: .mcp.json has no mcpServers.orbit entry" >&2
    exit 1
fi

mcp_command="$(printf '%s\n' "$orbit_entry" | sed -n 's/^[[:space:]]*"command"[[:space:]]*:[[:space:]]*"\([^"]*\)".*/\1/p' | head -n 1)"
if [[ -z "$mcp_command" ]]; then
    echo "onboarding smoke: .mcp.json has no MCP command" >&2
    exit 1
fi
case "$(basename "$mcp_command")" in
    orbit | orbit.exe) ;;
    *)
        echo "onboarding smoke: MCP command $mcp_command is not an orbit executable" >&2
        exit 1
        ;;
esac
case "$mcp_command" in
    /* | orbit | orbit.exe) ;;
    *)
        echo "onboarding smoke: MCP command $mcp_command is not absolute or portable" >&2
        exit 1
        ;;
esac

mcp_args=()
while IFS= read -r arg; do
    mcp_args+=("$arg")
done < <(
    printf '%s\n' "$orbit_entry" |
        sed -n '/"args"[[:space:]]*:[[:space:]]*\[/,/\]/p' |
        sed -n -e 's/^[[:space:]]*"\([^"]*\)"[[:space:]]*$/\1/p' -e 's/^[[:space:]]*"\([^"]*\)"[[:space:]]*,[[:space:]]*$/\1/p'
)
if [[ "${#mcp_args[@]}" -ne 1 || "${mcp_args[0]}" != "mcp" ]]; then
    echo "onboarding smoke: expected MCP args [mcp], got [${mcp_args[*]:-}]" >&2
    exit 1
fi

"$mcp_command" "${mcp_args[@]}" < "$handshake_input" > mcp-handshake.jsonl

grep -q '"serverInfo":{"name":"orbit"' mcp-handshake.jsonl
grep -q '"name":"ready"' mcp-handshake.jsonl

echo "onboarding smoke passed"

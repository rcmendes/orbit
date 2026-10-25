#!/usr/bin/env bash
# Install smoke: exercise detected agent setup and MCP with a real binary.
set -euo pipefail

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

# A project-local marker makes the detected-harness path deterministic without
# depending on host tools or the isolated home directory.
mkdir -p .claude

"$orbit" --version
"$orbit" init --agents auto --slug orb --json > init.json
grep -q '"claude"' init.json
"$orbit" add "Smoke task" --json > /dev/null
"$orbit" ready --json > /dev/null

# Check re-resolves the running binary and fails when .mcp.json points elsewhere.
"$orbit" setup claude --check --json > /dev/null

mcp_command="$(sed -n 's/^[[:space:]]*"command"[[:space:]]*:[[:space:]]*"\([^"]*\)".*/\1/p' .mcp.json | head -n 1)"
if [[ -z "$mcp_command" ]]; then
    echo "onboarding smoke: .mcp.json has no MCP command" >&2
    exit 1
fi
case "$mcp_command" in
    "$orbit" | orbit | orbit.exe) ;;
    *)
        echo "onboarding smoke: .mcp.json points at $mcp_command, not $orbit" >&2
        exit 1
        ;;
esac

mcp_args=()
while IFS= read -r arg; do
    mcp_args+=("$arg")
done < <(
    sed -n '/"args"[[:space:]]*:[[:space:]]*\[/,/\]/p' .mcp.json |
        sed -n -e 's/^[[:space:]]*"\([^"]*\)"[[:space:]]*$/\1/p' -e 's/^[[:space:]]*"\([^"]*\)"[[:space:]]*,[[:space:]]*$/\1/p'
)
if [[ "${#mcp_args[@]}" -ne 1 || "${mcp_args[0]}" != "mcp" ]]; then
    echo "onboarding smoke: expected MCP args [mcp], got [${mcp_args[*]:-}]" >&2
    exit 1
fi

{
    printf '%s\n' '{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-06-18","capabilities":{},"clientInfo":{"name":"orbit-onboarding-smoke","version":"0"}}}'
    printf '%s\n' '{"jsonrpc":"2.0","method":"notifications/initialized"}'
    printf '%s\n' '{"jsonrpc":"2.0","id":2,"method":"tools/list","params":{}}'
} | "$mcp_command" "${mcp_args[@]}" > mcp-handshake.jsonl

grep -q '"serverInfo":{"name":"orbit"' mcp-handshake.jsonl
grep -q '"name":"ready"' mcp-handshake.jsonl

echo "onboarding smoke passed"

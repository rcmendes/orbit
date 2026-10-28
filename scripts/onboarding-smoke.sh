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
mkdir -p "$smoke_home/bin"
cat > "$smoke_home/bin/gh" <<'SH'
#!/bin/sh
echo "onboarding smoke: unexpected gh call; use ORBIT_GITHUB_FIXTURE" >&2
exit 97
SH
chmod +x "$smoke_home/bin/gh"
export PATH="$smoke_home/bin:$PATH"
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
task_id="$("$orbit" add "Smoke task" --json | jq -r '.data.id')"
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

# The release artifact must include both commands and exercise GitHub entirely
# through the local fixture client; this smoke must never call GitHub's network.
"$orbit" github --help > /dev/null
"$orbit" triage --help > /dev/null
git remote add origin https://github.com/alice/widget.git
cat > github-fixture.json <<'JSON'
{
  "authenticated": true,
  "auth_message": "Logged in to github.com account smoke (fixture)",
  "auth_username": "smoke",
  "auth_scopes": ["repo"],
  "repositories": {
    "alice/widget": { "owner_type": "User", "accessible": true }
  },
  "org_fields": {},
  "issues": {
    "alice/widget#42": {
      "repo": "alice/widget",
      "number": 42,
      "title": "Fixture issue",
      "body": "Created by the offline release smoke.",
      "state": "open",
      "labels": ["bug"],
      "assignees": [],
      "updated_at": "2026-10-23T13:00:00Z"
    }
  }
}
JSON
export ORBIT_GITHUB_FIXTURE="$workdir/github-fixture.json"
"$orbit" github connect --json > github-connect.json
jq -e '.data.next_action == "choose_personal_account_path"' github-connect.json > /dev/null
cat >> .orbit/config.toml <<'TOML'

[github]
mode = "basic"
repositories = ["alice/widget"]
TOML
"$orbit" github status --json > github-status.json
jq -e '.data.mode == "basic" and .data.auth.authenticated == true' github-status.json > /dev/null
"$orbit" doctor --json > doctor.json
jq -e '.data.healthy == true' doctor.json > /dev/null

"$orbit" github adopt "$task_id" https://github.com/alice/widget/issues/42 --json > github-adopt.json
jq -e '.data.github.number == 42' github-adopt.json > /dev/null
jq '.issues["alice/widget#42"].title = "Updated in fixture" | .issues["alice/widget#42"].updated_at = "2026-10-24T13:00:00Z"' \
    "$ORBIT_GITHUB_FIXTURE" > github-fixture.updated.json
mv github-fixture.updated.json "$ORBIT_GITHUB_FIXTURE"
"$orbit" github sync "$task_id" --json > github-sync-preview.json
jq -e '.data.apply == false' github-sync-preview.json > /dev/null
grep -q 'Updated in fixture' github-sync-preview.json
"$orbit" github sync "$task_id" --direction pull --apply --json > github-sync-apply.json
jq -e '.data.apply == true' github-sync-apply.json > /dev/null
"$orbit" show "$task_id" --json | jq -e '.data.title == "Updated in fixture"' > /dev/null

cat > triage-proposal.json <<'JSON'
{
  "rationale": "Offline release smoke review",
  "labels_add": ["ready-for-agent"],
  "labels_remove": ["bug"]
}
JSON
"$orbit" triage context "$task_id" --json > triage-context.json
"$orbit" triage review "$task_id" --proposal-file triage-proposal.json --json > triage-review.json
jq -e '.data.apply_ready == true' triage-review.json > /dev/null
"$orbit" triage apply "$task_id" --proposal-file triage-proposal.json --approve --json > triage-apply.json
jq -e '.data.applied == true' triage-apply.json > /dev/null
"$orbit" github sync "$task_id" --direction push --apply --json > github-sync-push.json

created_id="$("$orbit" add "Create fixture issue" --json | jq -r '.data.id')"
"$orbit" github create "$created_id" --json > github-create.json
jq -e '.data.github.repo == "alice/widget"' github-create.json > /dev/null

echo "onboarding smoke passed"

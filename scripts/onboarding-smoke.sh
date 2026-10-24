#!/usr/bin/env bash
# Install smoke: exercise the onboarding path with a real binary.
set -euo pipefail

ORBIT="${ORBIT_BIN:-orbit}"
workdir="$(mktemp -d)"
trap 'rm -rf "$workdir"' EXIT

cd "$workdir"
git init -q .
git config user.email smoke@orbit.invalid
git config user.name "Orbit Smoke"

"$ORBIT" --version
"$ORBIT" init --agents none --json
"$ORBIT" add "Smoke task" --json > /dev/null
"$ORBIT" ready --json > /dev/null
"$ORBIT" setup claude --print --json > /dev/null

echo "onboarding smoke passed"

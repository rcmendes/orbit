# Install smoke: exercise the onboarding path with a real binary.
$ErrorActionPreference = "Stop"

$orbit = if ($env:ORBIT_BIN) { $env:ORBIT_BIN } else { "orbit" }
$workdir = Join-Path ([System.IO.Path]::GetTempPath()) ("orbit-smoke-" + [guid]::NewGuid())
New-Item -ItemType Directory -Path $workdir | Out-Null
Set-Location $workdir

function Invoke-Orbit {
    param([Parameter(ValueFromRemainingArguments = $true)][string[]]$OrbitArgs)
    & $orbit @OrbitArgs
    if ($LASTEXITCODE -ne 0) {
        throw "orbit $OrbitArgs failed with exit code $LASTEXITCODE"
    }
}

git init -q .
git config user.email smoke@orbit.invalid
git config user.name "Orbit Smoke"

Invoke-Orbit --version
Invoke-Orbit init --agents none --slug orb --json | Out-Null
Invoke-Orbit add "Smoke task" --json | Out-Null
Invoke-Orbit ready --json | Out-Null
Invoke-Orbit setup claude --print --json | Out-Null

Write-Output "onboarding smoke passed"

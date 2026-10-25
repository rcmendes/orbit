# Install smoke: exercise detected agent setup and MCP with a real binary.
$ErrorActionPreference = "Stop"

$handshakeInput = Join-Path $PSScriptRoot "onboarding-smoke-mcp.jsonl"

function Resolve-OrbitBinary {
    param([string]$Candidate)
    if ($Candidate -match '[\\/]') {
        $resolved = (Resolve-Path -LiteralPath $Candidate).Path
    } else {
        $resolved = (Get-Command $Candidate -CommandType Application -ErrorAction Stop |
            Select-Object -First 1).Source
    }
    return $resolved
}

function Invoke-Orbit {
    param([Parameter(ValueFromRemainingArguments = $true)][string[]]$OrbitArgs)
    $output = & $orbit @OrbitArgs
    if ($LASTEXITCODE -ne 0) {
        throw "orbit $OrbitArgs failed with exit code $LASTEXITCODE"
    }
    return $output
}

$binary = if ($env:ORBIT_BIN) { $env:ORBIT_BIN } else { "orbit" }
$orbit = Resolve-OrbitBinary $binary

$workdir = Join-Path ([System.IO.Path]::GetTempPath()) ("orbit-smoke-" + [guid]::NewGuid())
$smokeHome = Join-Path ([System.IO.Path]::GetTempPath()) ("orbit-smoke-home-" + [guid]::NewGuid())
New-Item -ItemType Directory -Path $workdir, $smokeHome | Out-Null
$originalLocation = Get-Location
$previousHome = $env:HOME
$previousUserProfile = $env:USERPROFILE
$previousOrbitDir = $env:ORBIT_DIR

try {
    # Keep detection and setup away from the developer's real agent configuration.
    $env:HOME = $smokeHome
    $env:USERPROFILE = $smokeHome
    Remove-Item Env:ORBIT_DIR -ErrorAction SilentlyContinue
    Set-Location $workdir

    git init -q .
    git config user.email smoke@orbit.invalid
    git config user.name "Orbit Smoke"

    # A project-local marker guarantees at least one detected harness without
    # depending on the isolated home directory.
    New-Item -ItemType Directory -Path ".claude" | Out-Null

    Invoke-Orbit --version | Out-Null
    $init = Invoke-Orbit init --agents auto --slug orb --json | ConvertFrom-Json
    if ($init.data.agents -notcontains "claude") {
        throw "orbit init did not detect the claude harness: $($init | ConvertTo-Json -Depth 5)"
    }
    Invoke-Orbit add "Smoke task" --json | Out-Null
    Invoke-Orbit ready --json | Out-Null

    # Check re-resolves the running binary, so a stale or foreign MCP command fails here.
    Invoke-Orbit setup claude --check --json | Out-Null

    $config = Get-Content ".mcp.json" -Raw | ConvertFrom-Json
    $entry = $config.mcpServers.orbit
    $mcpCommand = [string]$entry.command
    $mcpArgs = @($entry.args)

    $mcpName = [System.IO.Path]::GetFileName($mcpCommand)
    $portable = $mcpCommand -eq "orbit" -or $mcpCommand -eq "orbit.exe"
    if ($mcpName -ne "orbit" -and $mcpName -ne "orbit.exe") {
        throw "onboarding smoke: MCP command $mcpCommand is not an orbit executable"
    }
    if (-not ($portable -or [System.IO.Path]::IsPathRooted($mcpCommand))) {
        throw "onboarding smoke: MCP command $mcpCommand is not absolute or portable"
    }
    if ($mcpArgs.Count -ne 1 -or $mcpArgs[0] -ne "mcp") {
        throw "onboarding smoke: expected MCP args [mcp], got [$($mcpArgs -join ', ')]"
    }

    $handshake = Get-Content $handshakeInput | & $mcpCommand @mcpArgs
    if ($LASTEXITCODE -ne 0) {
        throw "the generated MCP command failed with exit code $LASTEXITCODE"
    }

    $responses = @($handshake | ForEach-Object { $_ | ConvertFrom-Json })
    $initialize = $responses | Where-Object { $_.id -eq 1 }
    $tools = $responses | Where-Object { $_.id -eq 2 }
    if (-not $initialize -or $initialize.result.serverInfo.name -ne "orbit") {
        throw "MCP initialize did not identify orbit: $handshake"
    }
    if (-not $tools -or @($tools.result.tools | ForEach-Object { $_.name }) -notcontains "ready") {
        throw "MCP tools/list did not include the ready tool: $handshake"
    }

    Write-Output "onboarding smoke passed"
} finally {
    Set-Location $originalLocation
    $env:HOME = $previousHome
    $env:USERPROFILE = $previousUserProfile
    $env:ORBIT_DIR = $previousOrbitDir
    Remove-Item -Recurse -Force $workdir, $smokeHome -ErrorAction SilentlyContinue
}

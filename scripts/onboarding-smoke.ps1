# Install smoke: exercise detected agent setup and MCP with a real binary.
$ErrorActionPreference = "Stop"

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

try {
    # Keep detection and setup away from the developer's real agent configuration.
    $env:HOME = $smokeHome
    $env:USERPROFILE = $smokeHome
    Remove-Item Env:ORBIT_DIR -ErrorAction SilentlyContinue
    Set-Location $workdir

    git init -q .
    git config user.email smoke@orbit.invalid
    git config user.name "Orbit Smoke"

    # A project-local marker makes the detected-harness path deterministic without
    # depending on host tools or the isolated home directory.
    New-Item -ItemType Directory -Path ".claude" | Out-Null

    Invoke-Orbit --version | Out-Null
    $init = Invoke-Orbit init --agents auto --slug orb --json | ConvertFrom-Json
    if ($init.data.agents -notcontains "claude") {
        throw "orbit init did not detect the claude harness: $($init | ConvertTo-Json -Depth 5)"
    }
    Invoke-Orbit add "Smoke task" --json | Out-Null
    Invoke-Orbit ready --json | Out-Null

    # Check re-resolves the running binary and fails when .mcp.json points elsewhere.
    Invoke-Orbit setup claude --check --json | Out-Null

    $config = Get-Content ".mcp.json" -Raw | ConvertFrom-Json
    $entry = $config.mcpServers.orbit
    $mcpCommand = [string]$entry.command
    $mcpArgs = @($entry.args)

    $portable = $mcpCommand -eq "orbit" -or $mcpCommand -eq "orbit.exe"
    $installed = $mcpCommand -eq $orbit
    if (-not ($portable -or $installed)) {
        throw "onboarding smoke: .mcp.json points at $mcpCommand, not $orbit"
    }
    if ($mcpArgs.Count -ne 1 -or $mcpArgs[0] -ne "mcp") {
        throw "onboarding smoke: expected MCP args [mcp], got [$($mcpArgs -join ', ')]"
    }

    $handshake = @(
        '{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-06-18","capabilities":{},"clientInfo":{"name":"orbit-onboarding-smoke","version":"0"}}}',
        '{"jsonrpc":"2.0","method":"notifications/initialized"}',
        '{"jsonrpc":"2.0","id":2,"method":"tools/list","params":{}}'
    ) | & $mcpCommand @mcpArgs
    if ($LASTEXITCODE -ne 0) {
        throw "the generated MCP command failed with exit code $LASTEXITCODE"
    }

    $responses = @($handshake | ForEach-Object { $_ | ConvertFrom-Json })
    $initialize = $responses | Where-Object { $_.id -eq 1 }
    $tools = $responses | Where-Object { $_.id -eq 2 }
    if ($initialize.result.serverInfo.name -ne "orbit") {
        throw "MCP initialize did not identify orbit: $handshake"
    }
    if (@($tools.result.tools | ForEach-Object { $_.name }) -notcontains "ready") {
        throw "MCP tools/list did not include the ready tool: $handshake"
    }

    Write-Output "onboarding smoke passed"
} finally {
    Set-Location $originalLocation
    Remove-Item -Recurse -Force $workdir, $smokeHome -ErrorAction SilentlyContinue
}

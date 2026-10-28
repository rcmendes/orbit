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
$previousGitHubFixture = $env:ORBIT_GITHUB_FIXTURE
$previousPath = $env:PATH

try {
    # Keep detection and setup away from the developer's real agent configuration.
    $env:HOME = $smokeHome
    $env:USERPROFILE = $smokeHome
    $smokeBin = Join-Path $smokeHome "bin"
    New-Item -ItemType Directory -Path $smokeBin | Out-Null
    [System.IO.File]::WriteAllText(
        (Join-Path $smokeBin "gh.cmd"),
        "@echo off`r`necho onboarding smoke: unexpected gh call; use ORBIT_GITHUB_FIXTURE 1>&2`r`nexit /b 97`r`n"
    )
    $env:PATH = "$smokeBin$([System.IO.Path]::PathSeparator)$previousPath"
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
    $taskId = (Invoke-Orbit add "Smoke task" --json | ConvertFrom-Json).data.id
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

    # The release artifact must include both commands and exercise GitHub
    # entirely through the local fixture client; this never calls GitHub.
    Invoke-Orbit github --help | Out-Null
    Invoke-Orbit triage --help | Out-Null
    git remote add origin https://github.com/alice/widget.git
    $fixturePath = Join-Path $workdir "github-fixture.json"
    $fixtureJson = @'
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
'@
    [System.IO.File]::WriteAllText($fixturePath, $fixtureJson, [System.Text.UTF8Encoding]::new($false))
    $env:ORBIT_GITHUB_FIXTURE = $fixturePath

    $connect = Invoke-Orbit github connect --json | ConvertFrom-Json
    if ($connect.data.next_action -ne "choose_personal_account_path") {
        throw "GitHub connect did not identify the personal-account setup path: $($connect | ConvertTo-Json -Depth 5)"
    }
    Add-Content -Path ".orbit/config.toml" -Value "`n[github]`nmode = `"basic`"`nrepositories = [`"alice/widget`"]"
    $status = Invoke-Orbit github status --json | ConvertFrom-Json
    if ($status.data.mode -ne "basic" -or -not $status.data.auth.authenticated) {
        throw "GitHub status did not report the configured fixture: $($status | ConvertTo-Json -Depth 5)"
    }
    $doctor = Invoke-Orbit doctor --json | ConvertFrom-Json
    if (-not $doctor.data.healthy) {
        throw "doctor reported an issue for the configured fixture: $($doctor | ConvertTo-Json -Depth 5)"
    }

    $adopt = Invoke-Orbit github adopt $taskId https://github.com/alice/widget/issues/42 --json | ConvertFrom-Json
    if ($adopt.data.github.number -ne 42) { throw "GitHub adopt did not link issue 42" }
    $fixture = Get-Content $fixturePath -Raw | ConvertFrom-Json
    $fixture.issues.'alice/widget#42'.title = "Updated in fixture"
    $fixture.issues.'alice/widget#42'.updated_at = "2026-10-24T13:00:00Z"
    $updatedFixture = ConvertTo-Json -InputObject $fixture -Depth 20
    [System.IO.File]::WriteAllText($fixturePath, $updatedFixture, [System.Text.UTF8Encoding]::new($false))

    $preview = Invoke-Orbit github sync $taskId --json | ConvertFrom-Json
    $previewText = $preview.data | ConvertTo-Json -Depth 10
    if ($preview.data.apply -or -not $previewText.Contains("Updated in fixture")) {
        throw "GitHub sync preview did not show the fixture change: $($preview | ConvertTo-Json -Depth 10)"
    }
    $sync = Invoke-Orbit github sync $taskId --direction pull --apply --json | ConvertFrom-Json
    if (-not $sync.data.apply) { throw "GitHub sync apply did not apply the pull" }
    $show = Invoke-Orbit show $taskId --json | ConvertFrom-Json
    if ($show.data.title -ne "Updated in fixture") { throw "GitHub sync did not update the Task title" }

    $proposalPath = Join-Path $workdir "triage-proposal.json"
    $proposal = @'
{
  "rationale": "Offline release smoke review",
  "labels_add": ["ready-for-agent"],
  "labels_remove": ["bug"]
}
'@
    [System.IO.File]::WriteAllText($proposalPath, $proposal, [System.Text.UTF8Encoding]::new($false))
    Invoke-Orbit triage context $taskId --json | Out-Null
    $review = Invoke-Orbit triage review $taskId --proposal-file $proposalPath --json | ConvertFrom-Json
    if (-not $review.data.apply_ready) { throw "Triage review did not validate its proposal" }
    $triage = Invoke-Orbit triage apply $taskId --proposal-file $proposalPath --approve --json | ConvertFrom-Json
    if (-not $triage.data.applied) { throw "Triage apply did not apply the reviewed proposal" }
    Invoke-Orbit github sync $taskId --direction push --apply --json | Out-Null

    $createdId = (Invoke-Orbit add "Create fixture issue" --json | ConvertFrom-Json).data.id
    $created = Invoke-Orbit github create $createdId --json | ConvertFrom-Json
    if ($created.data.github.repo -ne "alice/widget") { throw "GitHub create did not use the linked fixture repository" }

    Write-Output "onboarding smoke passed"
} finally {
    Set-Location $originalLocation
    $env:HOME = $previousHome
    $env:USERPROFILE = $previousUserProfile
    $env:ORBIT_DIR = $previousOrbitDir
    $env:ORBIT_GITHUB_FIXTURE = $previousGitHubFixture
    $env:PATH = $previousPath
    Remove-Item -Recurse -Force $workdir, $smokeHome -ErrorAction SilentlyContinue
}

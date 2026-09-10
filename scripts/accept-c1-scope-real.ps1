param(
    [string]$Server = "D:\experience_codex\experience-main\dist\Experience\experience-server.exe",
    [string]$CodexDirectory = "D:\experience_codex\codex-main\codex-rs\target\debug",
    [string]$CodexHome = "D:\experience_codex\codex-main\.codex-exp-home",
    [int]$Port = 18793
)
# Stage C scope-isolation real regression (1 codex session):
#   store: create_a (ACTIVE, scene-a, pattern "create probe a") and
#          create_b (ACTIVE, scene-b, pattern "create probe b")
#   task : mentions BOTH patterns
#   session scope=scene-a -> only scene-a ACTIVE may execute first.
# Asserts:
#   1. probe_a.txt exists immediately after task entry (execute-first)
#   2. probe_b.txt is ABSENT at task entry (scene-b never woke)
#   3. session completes
#   4. ledger has exactly one experience_execution (create_a success) and
#      none for create_b; delegate + delegation_completed (v2) present
$ErrorActionPreference = "Stop"

$cfg = Join-Path $HOME ".codex\config.toml"
$token = $null
$inSection = $false
foreach ($line in Get-Content $cfg) {
    if ($line -match '^\s*\[model_providers\.deepseek\]') { $inSection = $true; continue }
    if ($inSection -and $line -match '^\s*\[') { break }
    if ($inSection -and $line -match 'experimental_bearer_token\s*=\s*"([^"]+)"') { $token = $Matches[1] }
}
if (-not $token) { Write-Error "no deepseek token in $cfg"; exit 1 }
$env:DEEPSEEK_API_KEY = $token

$phaseHome = Join-Path $env:TEMP ("exp-c1-real-" + [guid]::NewGuid().ToString('N'))
New-Item -ItemType Directory -Path $phaseHome -Force | Out-Null
Copy-Item "D:\experience_codex\experience-main\crates\experience-core\tests\fixtures\c1\real\store.json" (Join-Path $phaseHome "store.json")
$workspace = Join-Path $env:TEMP ("exp-c1-real-ws-" + [guid]::NewGuid().ToString('N'))
New-Item -ItemType Directory -Path $workspace -Force | Out-Null
$utf8NoBom = New-Object System.Text.UTF8Encoding($false)
$agents = @{
    schema_version = 2
    agents = @(
        @{
            id = "codex"
            label = "codex (fork session)"
            kind = "codex_cli"
            mode = "managed"
            directory = $CodexDirectory
            channel = "session"
            codex_home = $CodexHome
        }
    )
} | ConvertTo-Json -Depth 5
[System.IO.File]::WriteAllText((Join-Path $phaseHome "agents.json"), $agents, $utf8NoBom)

$proc = Start-Process -FilePath $Server -ArgumentList @("--home", $phaseHome, "--port", "$Port") -WindowStyle Hidden -PassThru
$stamp = Get-Date -Format 'yyyyMMdd-HHmmss'
$artifactDir = "D:\experience_codex\experience-main\scripts\accept-artifacts\c1-real-$stamp"
New-Item -ItemType Directory -Path $artifactDir -Force | Out-Null
try {
    $ready = $false
    for ($i = 0; $i -lt 20; $i++) {
        try { $null = Invoke-RestMethod -Uri "http://127.0.0.1:$Port/api/health" -TimeoutSec 2; $ready = $true; break } catch { Start-Sleep -Milliseconds 500 }
    }
    if (-not $ready) { throw "server not ready" }

    $task = "create probe a and create probe b in the current directory, using exec_command; verify then stop."
    $body = @{ agent_id = "codex"; task = $task; cwd = $workspace; scope = "scene-a" } | ConvertTo-Json
    $created = Invoke-RestMethod -Uri "http://127.0.0.1:$Port/api/sessions" -Method Post -Body $body -ContentType "application/json"
    $sid = $created.id

    $probeA = Join-Path $workspace "probe_a.txt"
    $probeB = Join-Path $workspace "probe_b.txt"
    $earlyA = Test-Path $probeA
    $earlyB = Test-Path $probeB
    Write-Output "early_a=$earlyA early_b=$earlyB"
    if (-not $earlyA -or $earlyB) { throw "scope isolation broken at task entry" }

    $s = $null
    for ($i = 0; $i -lt 240; $i++) {
        $s = Invoke-RestMethod -Uri "http://127.0.0.1:$Port/api/sessions/$sid"
        if ($s.status -ne "running") { break }
        Start-Sleep -Seconds 2
    }
    if ($null -eq $s -or $s.status -ne "completed") { Write-Output "status=$($s.status)"; throw "session did not complete" }
    Write-Output "session_completed=$($s.status)"

    $records = @((Get-Content (Join-Path $phaseHome "learning-l1.json") -Raw | ConvertFrom-Json).records)
    $execA = @($records | Where-Object { $_.record_type -eq "experience_execution" -and $_.candidate_name -eq "create_a" -and $_.outcome -eq "success" })
    $execB = @($records | Where-Object { $_.record_type -eq "experience_execution" -and $_.candidate_name -eq "create_b" })
    $delegate = @($records | Where-Object { $_.record_type -eq "delegate" })
    $completed = @($records | Where-Object { $_.record_type -eq "delegation_completed" })
    Write-Output "exec_a=$($execA.Count) exec_b=$($execB.Count) delegate=$($delegate.Count) completed=$($completed.Count)"

    Copy-Item (Join-Path $phaseHome "sessions.json") $artifactDir
    Copy-Item (Join-Path $phaseHome "learning-l1.json") $artifactDir
    Copy-Item (Join-Path $phaseHome "store.json") $artifactDir
    if ($execA.Count -eq 1 -and $execB.Count -eq 0 -and $delegate.Count -ge 1 -and $completed.Count -ge 1) {
        Write-Output "C1 SCOPE ISOLATION REAL ACCEPTANCE: PASS (scene-a only)"
        Write-Output "artifacts: $artifactDir"
    } else {
        Write-Output "C1 SCOPE ISOLATION REAL ACCEPTANCE: FAIL"
        exit 1
    }
} finally {
    Stop-Process -Id $proc.Id -Force -ErrorAction SilentlyContinue
    Remove-Item -Recurse -Force $phaseHome, $workspace -ErrorAction SilentlyContinue
}

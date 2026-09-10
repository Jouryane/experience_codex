param(
    [string]$Server = "D:\experience_codex\experience-main\dist\Experience\experience-server.exe",
    [string]$CodexDirectory = "D:\experience_codex\codex-main\codex-rs\target\debug",
    [string]$CodexHome = "D:\experience_codex\codex-main\.codex-exp-home",
    [int]$PortExecute = 18783,
    [int]$PortMatchMiss = 18784
)
# L3 v2 execute-first real acceptance (fork codex + real API), two phases:
#   execute   : ACTIVE create_probe_file whose trigger pattern matches the
#               task text -> server runs the write_file workflow FIRST (probe
#               exists before the codex host starts), then delegates the
#               original task + completed steps/verified state. Asserts
#               experience_execution(success) -> delegate(executed_first) ->
#               delegation_completed(v2_executed_then_delegate) in the ledger
#               and a structured plan field on the delegate trace event.
#   match-miss: ACTIVE seed whose pattern does NOT match the task -> v1
#               semantics: delegate event/plan still present, but NO local
#               execution (no experience_execution record, no probe.txt).
# Artifacts are copied to scripts/accept-artifacts/l3-exec-<ts>/ for review.
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

$stamp = Get-Date -Format 'yyyyMMdd-HHmmss'
$artifactRoot = "D:\experience_codex\experience-main\scripts\accept-artifacts\l3-exec-$stamp"
New-Item -ItemType Directory -Path $artifactRoot -Force | Out-Null
$utf8NoBom = New-Object System.Text.UTF8Encoding($false)

$seedStore = @'
{
  "schema_version": 1,
  "experiences": [
    {
      "name": "create_probe_file",
      "trigger": {
        "tool": "exec_command",
        "command_pattern": "create probe file"
      },
      "preconditions": [
        {
          "key": "cwd.exists",
          "expected": true
        }
      ],
      "workflow": [
        {
          "action": "write_file",
          "args": {
            "path": "probe.txt",
            "content": "EXPERIENCE_GATE_SUCCESS"
          }
        }
      ],
      "postconditions": [
        {
          "key": "file:probe.txt.exists",
          "expected": true
        },
        {
          "key": "file:probe.txt.content",
          "expected": "EXPERIENCE_GATE_SUCCESS"
        }
      ],
      "verification": [],
      "failure_policy": "stop_and_report",
      "undo": "unsupported",
      "status": "active"
    }
  ]
}
'@

function New-PhaseHome([string]$phase, [int]$port) {
    $phaseHome = Join-Path $env:TEMP ("exp-l3-exec-$phase-" + [guid]::NewGuid().ToString('N'))
    New-Item -ItemType Directory -Path $phaseHome -Force | Out-Null
    [System.IO.File]::WriteAllText((Join-Path $phaseHome "store.json"), $seedStore, $utf8NoBom)
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
    $workspace = Join-Path $env:TEMP ("exp-l3-exec-ws-$phase-" + [guid]::NewGuid().ToString('N'))
    New-Item -ItemType Directory -Path $workspace -Force | Out-Null
    return @{ home = $phaseHome; workspace = $workspace }
}

function Poll-Session([string]$id, [int]$port) {
    for ($i = 0; $i -lt 240; $i++) {
        $s = Invoke-RestMethod -Uri "http://127.0.0.1:$port/api/sessions/$id"
        if ($s.status -ne "running") { return $s }
        Start-Sleep -Seconds 2
    }
    return $null
}

function Get-Ledger([string]$homeDir) {
    $path = Join-Path $homeDir "learning-l1.json"
    if (-not (Test-Path $path)) { return @() }
    return @((Get-Content $path -Raw | ConvertFrom-Json).records)
}

function Run-Phase([string]$phase, [int]$port, [string]$task, [bool]$expectExecute) {
    $paths = New-PhaseHome $phase $port
    $tempHome = $paths.home
    $workspace = $paths.workspace
    $proc = Start-Process -FilePath $Server -ArgumentList @("--home", $tempHome, "--port", "$port") -WindowStyle Hidden -PassThru
    try {
        $ready = $false
        for ($i = 0; $i -lt 20; $i++) {
            try { $null = Invoke-RestMethod -Uri "http://127.0.0.1:$port/api/health" -TimeoutSec 2; $ready = $true; break } catch { Start-Sleep -Milliseconds 500 }
        }
        if (-not $ready) { throw "server not ready" }

        $body = @{ agent_id = "codex"; task = $task; cwd = $workspace } | ConvertTo-Json
        $created = Invoke-RestMethod -Uri "http://127.0.0.1:$port/api/sessions" -Method Post -Body $body -ContentType "application/json"
        $sid = $created.id

        $probeBefore = Join-Path $workspace "probe.txt"
        if ($expectExecute) {
            if (-not (Test-Path $probeBefore)) { throw "execute-first: probe.txt must exist right after task entry" }
        }

        $s = Poll-Session $sid $port
        if ($null -eq $s) { throw "session poll timeout (phase $phase)" }
        Write-Output "[$phase] status=$($s.status) thread=$($s.thread_id) traceN=$($s.trace.Count)"
        if ($s.status -ne "completed") {
            Write-Output "[$phase] summary=$($s.summary)"
            throw "phase '$phase' session did not complete"
        }

        $records = Get-Ledger $tempHome
        $delegateEvents = @($s.trace | Where-Object { $_.kind -eq "delegate" })
        if ($delegateEvents.Count -ne 1) { throw "expected exactly one delegate trace event" }
        $delegateEvent = $delegateEvents[0]
        if (-not @($delegateEvent.plan).Contains("create_probe_file")) {
            throw "delegate trace event missing structured plan field"
        }
        if ($delegateEvent.task_summary -notlike "*[plan: create_probe_file]*") {
            throw "delegate task_summary missing plan label"
        }

        if ($expectExecute) {
            $execs = @($records | Where-Object { $_.record_type -eq "experience_execution" -and $_.session_id -eq $sid })
            if ($execs.Count -ne 1) { throw "expected one experience_execution record" }
            if ($execs[0].candidate_name -ne "create_probe_file") { throw "execution candidate mismatch" }
            if ($execs[0].outcome -ne "success") { throw "execution outcome mismatch: $($execs[0].outcome)" }
            $decisions = @($records | Where-Object { $_.record_type -eq "delegate" -and $_.session_id -eq $sid })
            if ($decisions[0].reason -notlike "*executed_first:create_probe_file:success*" -or $decisions[0].reason -notlike "*scope=l3_v2*") {
                throw "delegate reason missing executed_first/scope: $($decisions[0].reason)"
            }
            $completions = @($records | Where-Object { $_.record_type -eq "delegation_completed" -and $_.session_id -eq $sid })
            if ($completions.Count -ne 1 -or $completions[0].reason -ne "v2_executed_then_delegate") {
                throw "delegation_completed scope reason mismatch"
            }
            $probeContent = if (Test-Path $probeBefore) { (Get-Content $probeBefore -Raw).Trim() } else { "" }
            if ($probeContent -ne "EXPERIENCE_GATE_SUCCESS") { throw "probe.txt content mismatch" }
            $orderOk = $records.IndexOf($execs[0]) -lt $records.IndexOf($decisions[0]) -and
                       $records.IndexOf($decisions[0]) -lt $records.IndexOf($completions[0])
            if (-not $orderOk) { throw "ledger order broken: execution/delegate/completion" }
            Write-Output "[$phase] execute_first_ok=true"
        } else {
            $execs = @($records | Where-Object { $_.record_type -eq "experience_execution" -and $_.session_id -eq $sid })
            if ($execs.Count -ne 0) { throw "match-miss phase must have no experience_execution record" }
            if (Test-Path $probeBefore) { throw "match-miss phase must not execute write_file workflow" }
            $decisions = @($records | Where-Object { $_.record_type -eq "delegate" -and $_.session_id -eq $sid })
            if ($decisions[0].reason -notlike "*policy_off*" -or $decisions[0].reason -like "*executed_first*") {
                throw "match-miss delegate reason must stay v1: $($decisions[0].reason)"
            }
            $completions = @($records | Where-Object { $_.record_type -eq "delegation_completed" -and $_.session_id -eq $sid })
            if ($completions.Count -ne 1 -or $completions[0].reason -ne "v1_plan_aware") {
                throw "match-miss delegation_completed reason mismatch"
            }
            Write-Output "[$phase] match_miss_v1_ok=true"
        }

        $artifactDir = Join-Path $artifactRoot $phase
        New-Item -ItemType Directory -Path $artifactDir -Force | Out-Null
        Copy-Item (Join-Path $tempHome "sessions.json") $artifactDir
        Copy-Item (Join-Path $tempHome "learning-l1.json") $artifactDir
        Copy-Item (Join-Path $tempHome "store.json") $artifactDir
        Write-Output "[$phase] artifacts=$artifactDir"
        return $true
    } finally {
        Stop-Process -Id $proc.Id -Force -ErrorAction SilentlyContinue
        Remove-Item -Recurse -Force $tempHome, $workspace -ErrorAction SilentlyContinue
    }
}

$passExecute = $false
try {
    $nonce = [guid]::NewGuid().ToString('N').Substring(0, 12)
    $taskExecute = "create probe file in the current directory: write probe.txt whose content is exactly EXPERIENCE_GATE_SUCCESS using exec_command, verify, then stop."
    $passExecute = Run-Phase "execute" $PortExecute $taskExecute $true
} catch { Write-Output "L3 EXECUTE PHASE ERROR: $_" }
Write-Output "L3 EXECUTE-FIRST PHASE: $(if ($passExecute) { 'PASS' } else { 'FAIL' })"

$passMiss = $false
try {
    $nonce = [guid]::NewGuid().ToString('N').Substring(0, 12)
    $taskMiss = "create file l3-miss-$nonce.txt in the current directory whose content is exactly $nonce using exec_command, verify, then stop."
    $passMiss = Run-Phase "match-miss" $PortMatchMiss $taskMiss $false
} catch { Write-Output "L3 MATCH-MISS PHASE ERROR: $_" }
Write-Output "L3 MATCH-MISS PHASE: $(if ($passMiss) { 'PASS' } else { 'FAIL' })"

if (-not $passExecute -or -not $passMiss) { exit 1 }
Write-Output "APP L3 EXECUTE-FIRST ACCEPTANCE: PASS (execute-first + match-miss v1)"
Write-Output "artifacts: $artifactRoot"

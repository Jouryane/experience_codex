param(
    [string]$Server = "D:\experience_codex\experience-main\dist\Experience\experience-server.exe",
    [string]$WorkspaceRoot = "D:\experience_codex\experience-main\accept-l3-workspace",
    [string]$CodexDirectory = "D:\experience_codex\codex-main\codex-rs\target\debug",
    [string]$CodexHome = "D:\experience_codex\codex-main\.codex-exp-home",
    [int]$PortSeed = 18781,
    [int]$PortNoSeed = 18782
)
# L3 v1 plan-aware delegation real acceptance (fork codex + real API).
#   seed   : store starts with one ACTIVE experience -> delegate decision
#            (ledger record_type=delegate, reason policy_off;no_in_process_
#            executor_v1) + TraceEvent::Delegate at task entry; after the
#            delegated session completes -> delegation_completed.
#   no-seed: empty store -> pure delegation stays event-free: no delegate
#            event in trace, no delegate/delegation_completed ledger records.
# Artifacts are copied to scripts/accept-artifacts/l3-<ts>/ for review.
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
$artifactRoot = "D:\experience_codex\experience-main\scripts\accept-artifacts\l3-$stamp"
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
      "preconditions": [],
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

function New-TestHome([string]$phase, [int]$port, [bool]$seed) {
    $testHome = Join-Path $env:TEMP ("exp-l3-$phase-" + [guid]::NewGuid().ToString('N'))
    New-Item -ItemType Directory -Path $testHome -Force | Out-Null
    if ($seed) {
        [System.IO.File]::WriteAllText((Join-Path $testHome "store.json"), $seedStore, $utf8NoBom)
    }
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
    [System.IO.File]::WriteAllText((Join-Path $testHome "agents.json"), $agents, $utf8NoBom)
    return $testHome
}

function Poll-Session([string]$id, [int]$port) {
    for ($i = 0; $i -lt 240; $i++) {
        $s = Invoke-RestMethod -Uri "http://127.0.0.1:$port/api/sessions/$id"
        if ($s.status -ne "running") { return $s }
        Start-Sleep -Seconds 2
    }
    return $null
}

function Run-Phase([string]$phase, [int]$port, [bool]$seed) {
    $tempHome = New-TestHome $phase $port $seed
    $proc = Start-Process -FilePath $Server -ArgumentList @("--home", $tempHome, "--port", "$port") -WindowStyle Hidden -PassThru
    $ok = $false
    try {
        $ready = $false
        for ($i = 0; $i -lt 20; $i++) {
            try { $null = Invoke-RestMethod -Uri "http://127.0.0.1:$port/api/health" -TimeoutSec 2; $ready = $true; break } catch { Start-Sleep -Milliseconds 500 }
        }
        if (-not $ready) { throw "server not ready" }

        $nonce = [guid]::NewGuid().ToString('N').Substring(0, 12)
        $fileName = "l3-$phase-$nonce.txt"
        $task = "Create file $fileName in this directory with content $nonce using exec_command, verify, then stop."
        $body = @{ agent_id = "codex"; task = $task; cwd = $WorkspaceRoot } | ConvertTo-Json
        $created = Invoke-RestMethod -Uri "http://127.0.0.1:$port/api/sessions" -Method Post -Body $body -ContentType "application/json"
        $sid = $created.id

        $s = Poll-Session $sid $port
        if ($null -eq $s) { throw "session poll timeout (phase $phase)" }
        Write-Output "[$phase] status=$($s.status) thread=$($s.thread_id) traceN=$($s.trace.Count)"
        if ($s.status -ne "completed") {
            Write-Output "[$phase] summary=$($s.summary)"
            throw "phase '$phase' session did not complete"
        }

        $target = Join-Path $WorkspaceRoot $fileName
        $sideEffectOk = (Test-Path $target) -and ((Get-Content $target -Raw).Trim() -eq $nonce)
        Write-Output "[$phase] side_effect_ok=$sideEffectOk"
        if (-not $sideEffectOk) { throw "phase '$phase' side effect missing/mismatched" }

        $ledgerPath = Join-Path $tempHome "learning-l1.json"
        $ledgerText = if (Test-Path $ledgerPath) { Get-Content $ledgerPath -Raw } else { "" }
        $records = @()
        if ($ledgerText) {
            $parsed = $ledgerText | ConvertFrom-Json
            $records = @($parsed.records)
        }

        if ($seed) {
            $delegateEvents = @($s.trace | Where-Object { $_.kind -eq "delegate" })
            if ($delegateEvents.Count -ne 1) { throw "expected exactly one delegate trace event, got $($delegateEvents.Count)" }
            $delegateEvent = $delegateEvents[0]
            if ($delegateEvent.agent -ne "codex") { throw "delegate agent mismatch: $($delegateEvent.agent)" }
            if (-not $delegateEvent.task_summary.Contains("[plan: create_probe_file]")) {
                throw "delegate task_summary missing plan context: $($delegateEvent.task_summary)"
            }
            if ($s.trace[0].kind -ne "delegate") {
                throw "delegate event must be written at task entry (trace[0]), got $($s.trace[0].kind)"
            }

            $decisions = @($records | Where-Object { $_.record_type -eq "delegate" -and $_.session_id -eq $sid })
            if ($decisions.Count -ne 1) { throw "expected one ledger delegate decision, got $($decisions.Count)" }
            $decision = $decisions[0]
            if ($decision.candidate_name -ne "create_probe_file") { throw "delegate candidate_name mismatch: $($decision.candidate_name)" }
            if ($decision.reason -notlike "*policy_off*") { throw "delegate reason missing policy_off: $($decision.reason)" }
            if ($decision.outcome -ne "delegated") { throw "delegate outcome mismatch: $($decision.outcome)" }
            if ($decision.action -ne "delegate") { throw "delegate action mismatch: $($decision.action)" }

            $completions = @($records | Where-Object { $_.record_type -eq "delegation_completed" -and $_.session_id -eq $sid })
            if ($completions.Count -ne 1) { throw "expected one delegation_completed ledger record, got $($completions.Count)" }
            $completion = $completions[0]
            if ($completion.candidate_name -ne "create_probe_file") { throw "delegation_completed candidate_name mismatch: $($completion.candidate_name)" }
            if ($completion.outcome -ne "completed") { throw "delegation_completed outcome mismatch: $($completion.outcome)" }
            Write-Output "[$phase] delegate_trace=$($delegateEvent.task_summary)"
            Write-Output "[$phase] delegate_reason=$($decision.reason)"
            Write-Output "[$phase] delegation_completed_reason=$($completion.reason)"
        } else {
            $delegateEvents = @($s.trace | Where-Object { $_.kind -eq "delegate" })
            if ($delegateEvents.Count -ne 0) { throw "no-seed phase must stay delegate-event-free, got $($delegateEvents.Count)" }
            $l3Records = @($records | Where-Object { $_.record_type -eq "delegate" -or $_.record_type -eq "delegation_completed" -or $_.record_type -eq "delegation_failed" })
            if ($l3Records.Count -ne 0) { throw "no-seed phase must have no delegate ledger records, got $($l3Records.Count)" }
            Write-Output "[$phase] delegate_free_ok=true"
        }

        $artifactDir = Join-Path $artifactRoot $phase
        New-Item -ItemType Directory -Path $artifactDir -Force | Out-Null
        Copy-Item (Join-Path $tempHome "sessions.json") $artifactDir -ErrorAction SilentlyContinue
        Copy-Item (Join-Path $tempHome "learning-l1.json") $artifactDir -ErrorAction SilentlyContinue
        Copy-Item (Join-Path $tempHome "store.json") $artifactDir -ErrorAction SilentlyContinue
        $ok = $true
        Write-Output "[$phase] artifacts=$artifactDir"
        return $true
    } finally {
        Stop-Process -Id $proc.Id -Force -ErrorAction SilentlyContinue
        Remove-Item -Recurse -Force $tempHome -ErrorAction SilentlyContinue
    }
    return $ok
}

New-Item -ItemType Directory -Path $WorkspaceRoot -Force | Out-Null

$passSeed = $false
try { $passSeed = Run-Phase "seed" $PortSeed $true }
catch { Write-Output "L3 SEED PHASE ERROR: $_" }
Write-Output "L3 SEED PHASE: $(if ($passSeed) { 'PASS' } else { 'FAIL' })"

$passNoSeed = $false
try { $passNoSeed = Run-Phase "no-seed" $PortNoSeed $false }
catch { Write-Output "L3 NO-SEED PHASE ERROR: $_" }
Write-Output "L3 NO-SEED PHASE: $(if ($passNoSeed) { 'PASS' } else { 'FAIL' })"

if (-not $passSeed -or -not $passNoSeed) { exit 1 }

Write-Output "APP L3 DELEGATE ACCEPTANCE: PASS (seed delegate + no-seed delegate-free)"
Write-Output "artifacts: $artifactRoot"

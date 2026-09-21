param(
    [string]$Server = "D:\experience_codex\experience-main\dist\Experience\experience-server.exe",
    [string]$CodexDirectory = "D:\experience_codex\codex-main\codex-rs\target\debug",
    [string]$CodexHome = "D:\experience_codex\codex-main\.codex-exp-home",
    [int]$Port = 18785
)
# L3 outer-loop real acceptance (WP4): two identical execute-first sessions
# on ONE home/workspace. Verifies the closed loop:
#   r1/r2  : experience_execution(success) -> delegate(executed_first,
#            scope=l3_v2) -> injection(omitted, policy_off) ->
#            delegation_completed(v2_executed_then_delegate)
#   usage  : GET /api/usage shows successes 1 -> 2, usage_count 1 -> 2,
#            score rising (evidence/activity observable, no decay)
#   repeat : second identical task is rejected by the L1 sink
#            (repeat_no_improvement), no duplicate candidate
# Real budget: exactly 2 codex sessions.
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
$artifactDir = "D:\experience_codex\experience-main\scripts\accept-artifacts\l3-loop-$stamp"
New-Item -ItemType Directory -Path $artifactDir -Force | Out-Null
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

$phaseHome = Join-Path $env:TEMP ("exp-l3-loop-" + [guid]::NewGuid().ToString('N'))
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
$workspace = Join-Path $env:TEMP ("exp-l3-loop-ws-" + [guid]::NewGuid().ToString('N'))
New-Item -ItemType Directory -Path $workspace -Force | Out-Null

$proc = Start-Process -FilePath $Server -ArgumentList @("--home", $phaseHome, "--port", "$Port") -WindowStyle Hidden -PassThru

function Poll-Session([string]$id) {
    for ($i = 0; $i -lt 240; $i++) {
        $s = Invoke-RestMethod -Uri "http://127.0.0.1:$Port/api/sessions/$id"
        if ($s.status -ne "running") { return $s }
        Start-Sleep -Seconds 2
    }
    return $null
}

function Get-LedgerRecords {
    $path = Join-Path $phaseHome "learning-l1.json"
    if (-not (Test-Path $path)) { return @() }
    return @((Get-Content $path -Raw | ConvertFrom-Json).records)
}

function Get-Usage {
    return Invoke-RestMethod -Uri "http://127.0.0.1:$Port/api/usage"
}

try {
    $ready = $false
    for ($i = 0; $i -lt 20; $i++) {
        try { $null = Invoke-RestMethod -Uri "http://127.0.0.1:$Port/api/health" -TimeoutSec 2; $ready = $true; break } catch { Start-Sleep -Milliseconds 500 }
    }
    if (-not $ready) { throw "server not ready" }

    $task = "create probe file in the current directory: write probe.txt whose content is exactly EXPERIENCE_GATE_SUCCESS using exec_command, verify, then stop."
    $probe = Join-Path $workspace "probe.txt"

    $body = @{ agent_id = "codex"; task = $task; cwd = $workspace } | ConvertTo-Json
    $sessions = @()

    for ($round = 1; $round -le 2; $round++) {
        $created = Invoke-RestMethod -Uri "http://127.0.0.1:$Port/api/sessions" -Method Post -Body $body -ContentType "application/json"
        $sid = $created.id
        if (-not (Test-Path $probe)) { throw "round ${round}: execute-first probe.txt missing after task entry" }
        $s = Poll-Session $sid
        if ($null -eq $s) { throw "round ${round} poll timeout" }
        if ($s.status -ne "completed") { Write-Output "round ${round} summary=$($s.summary)"; throw "round ${round} failed" }
        Write-Output "[round$round] status=$($s.status) traceN=$($s.trace.Count)"
        $sessions += $s
    }

    $records = Get-LedgerRecords
    $usage = Get-Usage
    $entry = $usage.entries.create_probe_file
    Write-Output "usage successes=$($entry.evidence.successes) usage_count=$($entry.activity.usage_count) score=$($entry.score)"

    $execs = @($records | Where-Object { $_.record_type -eq "experience_execution" -and $_.outcome -eq "success" })
    $decisions = @($records | Where-Object { $_.record_type -eq "delegate" -and $_.reason -like "*executed_first:create_probe_file:success*" })
    $injections = @($records | Where-Object { $_.record_type -eq "injection" -and $_.outcome -eq "omitted" -and $_.reason -eq "policy_off" })
    $completions = @($records | Where-Object { $_.record_type -eq "delegation_completed" -and $_.reason -eq "v2_executed_then_delegate" })
    $decayed = @($records | Where-Object { $_.record_type -eq "decayed" })
    $rejections = @($records | Where-Object { $_.outcome -like "rejected:*" })
    $toolCalls = @($sessions | ForEach-Object { $_.trace } | Where-Object { $_.kind -eq "toolCall" })
    $redoWrites = @($toolCalls | Where-Object {
        $_.name -in @("commandExecution", "exec_command") -and
        $_.args_summary -match "probe\.txt" -and
        $_.args_summary -match "(Set-Content|Out-File|Add-Content|WriteAllText|New-Item)"
    })

    $ok = $true
    if ($execs.Count -ne 2) { Write-Output "FAIL execs=$($execs.Count)"; $ok = $false }
    if ($decisions.Count -ne 2) { Write-Output "FAIL decisions=$($decisions.Count)"; $ok = $false }
    if ($injections.Count -ne 2) { Write-Output "FAIL injections=$($injections.Count)"; $ok = $false }
    if ($completions.Count -ne 2) { Write-Output "FAIL completions=$($completions.Count)"; $ok = $false }
    if ($decayed.Count -ne 0) { Write-Output "FAIL unexpected decay"; $ok = $false }
    if ($rejections.Count -lt 1) { Write-Output "FAIL repeat sink rejection missing"; $ok = $false }
    if ($redoWrites.Count -ne 0) { Write-Output "FAIL known step re-executed by delegated agent ($($redoWrites.Count) write tool calls)"; $ok = $false }
    if ($entry.evidence.successes -ne 2 -or $entry.activity.usage_count -ne 2) { Write-Output "FAIL usage counters"; $ok = $false }
    if ($entry.score -le 0.5) { Write-Output "FAIL score did not rise"; $ok = $false }
    if ((Get-Content $probe -Raw).Trim() -ne "EXPERIENCE_GATE_SUCCESS") { Write-Output "FAIL probe content"; $ok = $false }

    Copy-Item (Join-Path $phaseHome "sessions.json") $artifactDir
    Copy-Item (Join-Path $phaseHome "learning-l1.json") $artifactDir
    Copy-Item (Join-Path $phaseHome "usage.json") $artifactDir
    Copy-Item (Join-Path $phaseHome "store.json") $artifactDir

    if (-not $ok) { Write-Output "APP L3 LOOP ACCEPTANCE: FAIL"; exit 1 }
    Write-Output "APP L3 LOOP ACCEPTANCE: PASS (execute-first x2 + usage observable + no known-step redo)"
    Write-Output "artifacts: $artifactDir"
} finally {
    Stop-Process -Id $proc.Id -Force -ErrorAction SilentlyContinue
    Remove-Item -Recurse -Force $phaseHome, $workspace -ErrorAction SilentlyContinue
}

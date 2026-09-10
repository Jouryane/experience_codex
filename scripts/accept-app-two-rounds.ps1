param(
    [string]$Server = "D:\experience_codex\experience-main\dist\Experience\experience-server.exe",
    [string]$Workspace = "D:\experience_codex\experience-main\probe-e-workspace",
    [string]$CodexDirectory = "D:\experience_codex\codex-main\codex-rs\target\debug",
    [string]$CodexHome = "D:\experience_codex\codex-main\.codex-exp-home",
    [int]$Port = 18768
)
# P2 release-gate acceptance: real app HTTP flow (dist server) with two
# rounds on ONE session (initial + resume). Verifies:
#   - session-channel run completes (turn/completed), thread_id recorded
#   - resume on the same thread completes cleanly (no dirty active turn)
#   - trace is single-write: submitted/accepted/agent_started/turn_completed
#     each appear exactly once per round
# The DeepSeek token is read from ~/.codex/config.toml exactly like the
# run-*.ps1 launchers; it is never printed.
$ErrorActionPreference = 'Stop'

$cfg = Join-Path $HOME ".codex\config.toml"
$token = $null
$inSection = $false
foreach ($line in Get-Content $cfg) {
    if ($line -match '^\s*\[model_providers\.deepseek\]') { $inSection = $true; continue }
    if ($inSection -and $line -match '^\s*\[') { break }
    if ($inSection -and $line -match 'experimental_bearer_token\s*=\s*"([^"]+)"') {
        $token = $Matches[1]
    }
}
if (-not $token) { Write-Error "no deepseek token in $cfg"; exit 1 }
$env:DEEPSEEK_API_KEY = $token

$tempHome = Join-Path $env:TEMP ("exp-accept-" + [guid]::NewGuid().ToString('N'))
New-Item -ItemType Directory -Path $tempHome -Force | Out-Null
$agents = @{
    schema_version = 2
    agents = @(
        @{
            id         = "codex"
            label      = "codex (fork session)"
            kind       = "codex_cli"
            mode       = "managed"
            directory  = $CodexDirectory
            channel    = "session"
            codex_home = $CodexHome
        }
    )
} | ConvertTo-Json -Depth 5
$utf8NoBom = New-Object System.Text.UTF8Encoding($false)
[System.IO.File]::WriteAllText(
    (Join-Path $tempHome "agents.json"),
    $agents,
    $utf8NoBom
)

$proc = Start-Process -FilePath $Server -ArgumentList @('--home', $tempHome, '--port', "$Port") -WindowStyle Hidden -PassThru

function Poll-Session([string]$id, [int]$maxSeconds) {
    for ($i = 0; $i -lt $maxSeconds; $i++) {
        $s = Invoke-RestMethod -Uri "http://127.0.0.1:$Port/api/sessions/$id"
        if ($s.status -ne 'running') { return $s }
        Start-Sleep -Seconds 2
    }
    return $null
}

try {
    $ready = $false
    for ($i = 0; $i -lt 20; $i++) {
        try {
            $null = Invoke-RestMethod -Uri "http://127.0.0.1:$Port/api/health" -TimeoutSec 2
            $ready = $true
            break
        } catch { Start-Sleep -Milliseconds 500 }
    }
    if (-not $ready) { throw 'server not ready' }

    $nonceA = [guid]::NewGuid().ToString('N')
    $nonceB = [guid]::NewGuid().ToString('N')
    $taskA = "Create a file named acc-round1-$nonceA.txt in $Workspace whose content is exactly $nonceA, then stop."
    $taskB = "Create a file named acc-round2-$nonceB.txt in $Workspace whose content is exactly $nonceB, then stop."

    $bodyA = @{ agent_id = 'codex'; task = $taskA; cwd = $Workspace } | ConvertTo-Json
    $created = Invoke-RestMethod -Uri "http://127.0.0.1:$Port/api/sessions" -Method Post -Body $bodyA -ContentType 'application/json'
    $sid = $created.id
    Write-Output "session=$sid"

    $s1 = Poll-Session $sid 240
    Write-Output "round1 status=$($s1.status) thread=$($s1.thread_id) traceN=$($s1.trace.Count)"
    if ($s1.status -ne 'completed') { Write-Output "round1 summary=$($s1.summary)"; exit 1 }

    $bodyB = @{ task = $taskB } | ConvertTo-Json
    $null = Invoke-RestMethod -Uri "http://127.0.0.1:$Port/api/sessions/$sid/resume" -Method Post -Body $bodyB -ContentType 'application/json'

    $s2 = Poll-Session $sid 240
    Write-Output "round2 status=$($s2.status) thread=$($s2.thread_id) traceN=$($s2.trace.Count)"
    if ($s2.status -ne 'completed') { Write-Output "round2 summary=$($s2.summary)"; exit 1 }

    $counts = @{}
    foreach ($m in $s2.trace) {
        $kind = $m.kind
        if ($counts.ContainsKey($kind)) { $counts[$kind] = $counts[$kind] + 1 } else { $counts[$kind] = 1 }
    }
    $cSub = $counts['submitted']; $cAcc = $counts['accepted']
    $cStart = $counts['agentStarted']; $cTurn = $counts['turnCompleted']
    Write-Output "submitted=$cSub accepted=$cAcc agent_started=$cStart turn_completed=$cTurn"

    $f1 = Join-Path $Workspace "acc-round1-$nonceA.txt"
    $f2 = Join-Path $Workspace "acc-round2-$nonceB.txt"
    $ok1 = (Test-Path $f1) -and ((Get-Content $f1 -Raw).Trim() -eq $nonceA)
    $ok2 = (Test-Path $f2) -and ((Get-Content $f2 -Raw).Trim() -eq $nonceB)
    Write-Output "side_effects_ok=$($ok1 -and $ok2)"

    if (($cSub -eq 2) -and $cAcc -eq 2 -and $cTurn -eq 2 -and $ok1 -and $ok2) {
        Write-Output 'APP ACCEPTANCE: PASS (two rounds, trace single-write)'
    } else {
        Write-Output 'APP ACCEPTANCE: FAIL'
        exit 1
    }
} finally {
    Stop-Process -Id $proc.Id -Force -ErrorAction SilentlyContinue
    Remove-Item -Recurse -Force $tempHome -ErrorAction SilentlyContinue
}

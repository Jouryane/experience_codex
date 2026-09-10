param(
    [string]$Server = "D:\experience_codex\experience-main\dist\Experience\experience-server.exe",
    [string]$Workspace = "D:\experience_codex\experience-main\probe-e-workspace",
    [int]$Port = 18773
)
# L1 sink real acceptance (P2-3): three phases on one server/home.
#   cold  : exec_command task -> exactly one CANDIDATE (canonical, no hex)
#   repeat: same task text again -> ledger rejected:repeat_no_improvement
#   noop  : no-tool task -> ledger rejected:no_action_evidence (or terminal)
# Artifacts are copied to scripts/accept-artifacts/l1-<ts>/ for review; a
# normalized copy is committed as crates/experience-core/tests/fixtures/...
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
$tempHome = Join-Path $env:TEMP "exp-l1-accept-$stamp"
$artifactDir = "D:\experience_codex\experience-main\scripts\accept-artifacts\l1-$stamp"
New-Item -ItemType Directory -Path $tempHome, $artifactDir -Force | Out-Null
$utf8NoBom = New-Object System.Text.UTF8Encoding($false)
$agents = @{
    schema_version = 2
    agents = @(
        @{
            id = "codex"
            label = "codex (fork session)"
            kind = "codex_cli"
            mode = "managed"
            directory = "D:\experience_codex\codex-main\codex-rs\target\debug"
            channel = "session"
            codex_home = "D:\experience_codex\codex-main\.codex-exp-home"
        }
    )
} | ConvertTo-Json -Depth 5
[System.IO.File]::WriteAllText((Join-Path $tempHome "agents.json"), $agents, $utf8NoBom)

$proc = Start-Process -FilePath $Server -ArgumentList @("--home", $tempHome, "--port", "$Port") -WindowStyle Hidden -PassThru

function Poll-Session([string]$id) {
    for ($i = 0; $i -lt 180; $i++) {
        $s = Invoke-RestMethod -Uri "http://127.0.0.1:$Port/api/sessions/$id"
        if ($s.status -ne "running") { return $s }
        Start-Sleep -Seconds 2
    }
    return $null
}

function Ledger-Lines {
    $ledger = Join-Path $tempHome "learning-l1.json"
    if (Test-Path $ledger) { return (Get-Content $ledger -Raw) }
    return ""
}

try {
    $ready = $false
    for ($i = 0; $i -lt 20; $i++) {
        try { $null = Invoke-RestMethod -Uri "http://127.0.0.1:$Port/api/health" -TimeoutSec 2; $ready = $true; break } catch { Start-Sleep -Milliseconds 500 }
    }
    if (-not $ready) { throw "server not ready" }

    $taskCold = "Use the exec_command tool to run a PowerShell command that creates a file named accept-l1-sink.txt in $Workspace whose content is exactly L1_SINK_COLD_OK, then read it back with Get-Content to verify, then stop."
    $taskNoop = "Reply with the single word READY. Do not use any tools and do not create or modify any file."

    function Run-Session([string]$task) {
        $body = @{ agent_id = "codex"; task = $task; cwd = $Workspace } | ConvertTo-Json
        $created = Invoke-RestMethod -Uri "http://127.0.0.1:$Port/api/sessions" -Method Post -Body $body -ContentType "application/json"
        $s = Poll-Session $created.id
        if ($null -eq $s) { throw "session poll timeout" }
        return $s
    }

    # Phase 1: cold sink.
    $cold = Run-Session $taskCold
    Write-Output "cold status=$($cold.status)"
    if ($cold.status -ne "completed") { throw "cold round failed" }
    $exps = Invoke-RestMethod -Uri "http://127.0.0.1:$Port/api/experiences"
    Write-Output "cold candidates=$(@($exps).Count)"
    if (@($exps).Count -ne 1) { throw "expected exactly one candidate" }
    $exp = $exps[0]
    if ($exp.status -ne "candidate" -or $exp.trigger_tool -ne "exec_command") { throw "candidate not inert/canonical" }

    # Phase 2: identical task text -> repeat rejection, no duplicate.
    $null = Run-Session $taskCold
    $exps2 = Invoke-RestMethod -Uri "http://127.0.0.1:$Port/api/experiences"
    if (@($exps2).Count -ne 1) { throw "repeat created a duplicate" }

    # Phase 3: no-tool round -> rejected (no_action_evidence expected).
    $null = Run-Session $taskNoop

    $ledger = Ledger-Lines
    if ($ledger -notmatch "written") { throw "ledger missing written" }
    if ($ledger -notmatch "rejected:repeat_no_improvement") { throw "ledger missing repeat rejection" }
    if ($ledger -notmatch "rejected:no_action_evidence" -and $ledger -notmatch "rejected:terminal_not_success") {
        throw "ledger missing no-op rejection"
    }

    Copy-Item (Join-Path $tempHome "store.json") $artifactDir
    Copy-Item (Join-Path $tempHome "learning-l1.json") $artifactDir
    Write-Output "APP L1 SINK ACCEPTANCE: PASS (cold + repeat + noop)"
    Write-Output "artifacts: $artifactDir"
} finally {
    Stop-Process -Id $proc.Id -Force -ErrorAction SilentlyContinue
    Remove-Item -Recurse -Force $tempHome -ErrorAction SilentlyContinue
}

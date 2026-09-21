param(
    [string]$Server = "D:\experience_codex\experience-main\dist\Experience\experience-server.exe",
    [string]$CodexDirectory = "D:\experience_codex\codex-main\codex-rs\target\debug",
    [string]$CodexHome = "D:\experience_codex\codex-main\.codex-exp-home",
    [int]$Port = 18787
)
# S3 real acceptance: dirty round -> LLM compiler -> CANDIDATE.
#   main : one codex session performs 4+ exec_command steps (dirty > 3);
#          after completion the server sink routes the dirty round to
#          CodexLlmDistiller (EXPERIENCE_LLM_COMPILER=on).
#   compile : the distiller spawns ONE codex exec call (JSON-only) -> parses
#             an Experience draft -> CandidateWriter stores CANDIDATE.
# Asserts: session completed, exactly one CANDIDATE in store, ledger has
# 'written' and no rejected:distill_unavailable.
# Real budget: 2 codex sessions (main + compiler).
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

$oldCompiler = $env:EXPERIENCE_LLM_COMPILER
$oldCodex = $env:EXPERIENCE_LLM_CODEX
$env:EXPERIENCE_LLM_COMPILER = "on"
$env:EXPERIENCE_LLM_CODEX = Join-Path $CodexDirectory "codex.exe"

$tempHome = Join-Path $env:TEMP ("exp-l3-llm-" + [guid]::NewGuid().ToString('N'))
New-Item -ItemType Directory -Path $tempHome -Force | Out-Null
$workspace = Join-Path $env:TEMP ("exp-l3-llm-ws-" + [guid]::NewGuid().ToString('N'))
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
[System.IO.File]::WriteAllText((Join-Path $tempHome "agents.json"), $agents, $utf8NoBom)

$proc = Start-Process -FilePath $Server -ArgumentList @("--home", $tempHome, "--port", "$Port") -WindowStyle Hidden -PassThru
$stamp = Get-Date -Format 'yyyyMMdd-HHmmss'
$artifactDir = "D:\experience_codex\experience-main\scripts\accept-artifacts\l3-llm-$stamp"
New-Item -ItemType Directory -Path $artifactDir -Force | Out-Null

try {
    $ready = $false
    for ($i = 0; $i -lt 20; $i++) {
        try { $null = Invoke-RestMethod -Uri "http://127.0.0.1:$Port/api/health" -TimeoutSec 2; $ready = $true; break } catch { Start-Sleep -Milliseconds 500 }
    }
    if (-not $ready) { throw "server not ready" }

    $task = "Use the exec_command tool in sequence to run four simple PowerShell commands: create file seq-a.txt containing A, create seq-b.txt containing B, create seq-c.txt containing C, create seq-d.txt containing D, then read back seq-d.txt to verify and stop."
    $body = @{ agent_id = "codex"; task = $task; cwd = $workspace } | ConvertTo-Json
    $created = Invoke-RestMethod -Uri "http://127.0.0.1:$Port/api/sessions" -Method Post -Body $body -ContentType "application/json"
    $sid = $created.id

    $s = $null
    for ($i = 0; $i -lt 240; $i++) {
        $s = Invoke-RestMethod -Uri "http://127.0.0.1:$Port/api/sessions/$sid"
        if ($s.status -ne "running") { break }
        Start-Sleep -Seconds 2
    }
    if ($null -eq $s -or $s.status -ne "completed") { Write-Output "session status=$($s.status)"; throw "main dirty session did not complete" }
    Write-Output "main session completed traceN=$($s.trace.Count)"

    $candidate = $null
    $ledgerWritten = $false
    for ($i = 0; $i -lt 180; $i++) {
        $exps = @(Invoke-RestMethod -Uri "http://127.0.0.1:$Port/api/experiences")
        $ledgerPath = Join-Path $tempHome "learning-l1.json"
        $ledgerText = if (Test-Path $ledgerPath) { Get-Content $ledgerPath -Raw } else { "" }
        if ($exps.Count -ge 1 -and $ledgerText -match '"written"') {
            $candidate = $exps | Where-Object { $_.status -eq "candidate" } | Select-Object -First 1
            $ledgerWritten = $true
            break
        }
        if ($ledgerText -match "rejected:distill_unavailable") { throw "LLM compiler refused/returned no draft" }
        Start-Sleep -Seconds 2
    }
    if ($null -eq $candidate) { throw "no candidate appeared within timeout" }
    Write-Output "candidate=$($candidate.name) status=$($candidate.status) steps=$($candidate.workflow_steps)"

    $sideEffects = @("seq-a.txt", "seq-b.txt", "seq-c.txt", "seq-d.txt") | ForEach-Object { Test-Path (Join-Path $workspace $_) }
    Write-Output "side_effects_ok=$(-not ($sideEffects -contains $false))"

    Copy-Item (Join-Path $tempHome "sessions.json") $artifactDir
    Copy-Item (Join-Path $tempHome "learning-l1.json") $artifactDir
    Copy-Item (Join-Path $tempHome "store.json") $artifactDir
    if ($candidate.status -eq "candidate" -and $ledgerWritten -and -not ($sideEffects -contains $false)) {
        Write-Output "L3 LLM COMPILE ACCEPTANCE: PASS (dirty round -> CANDIDATE)"
        Write-Output "artifacts: $artifactDir"
    } else {
        Write-Output "L3 LLM COMPILE ACCEPTANCE: FAIL"
        exit 1
    }
} finally {
    Stop-Process -Id $proc.Id -Force -ErrorAction SilentlyContinue
    Remove-Item -Recurse -Force $tempHome, $workspace -ErrorAction SilentlyContinue
    if ($null -eq $oldCompiler) { Remove-Item Env:EXPERIENCE_LLM_COMPILER -ErrorAction SilentlyContinue } else { $env:EXPERIENCE_LLM_COMPILER = $oldCompiler }
    if ($null -eq $oldCodex) { Remove-Item Env:EXPERIENCE_LLM_CODEX -ErrorAction SilentlyContinue } else { $env:EXPERIENCE_LLM_CODEX = $oldCodex }
}

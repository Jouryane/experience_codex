param(
    [string]$Server = "D:\experience_codex\experience-main\target\debug\experience-server.exe",
    [string]$CodexDirectory = "D:\experience_codex\codex-main\codex-rs\target\debug",
    [string]$CodexHome = "D:\experience_codex\codex-main\.codex-exp-home",
    [int]$Port = 18920,
    [int]$TimeoutSecs = 240
)
# S2-d real acceptance (1 session): an allowlisted native program runs inside
# L3, its exit code is the postcondition verdict, and the delegated agent must
# not redo the work.
$ErrorActionPreference = "Stop"
$env:Path = "$env:USERPROFILE\.cargo\bin;$env:Path"
$cfg = Join-Path $HOME ".codex\config.toml"
$token = $null; $inSection = $false
foreach ($line in Get-Content $cfg) {
    if ($line -match '^\s*\[model_providers\.deepseek\]') { $inSection = $true; continue }
    if ($inSection -and $line -match '^\s*\[') { break }
    if ($inSection -and $line -match 'experimental_bearer_token\s*=\s*"([^"]+)"') { $token = $Matches[1] }
}
if (-not $token) { Write-Error "no deepseek token"; exit 1 }
$env:DEEPSEEK_API_KEY = $token

$utf8NoBom = New-Object System.Text.UTF8Encoding($false)
$stamp = Get-Date -Format 'yyyyMMdd-HHmmss'
$root = "D:\experience_codex\experience-main\scripts\accept-artifacts\s2d-$stamp"
New-Item -ItemType Directory -Path $root -Force | Out-Null

$sceneHome = Join-Path $env:TEMP ("exp-s2d-" + [guid]::NewGuid().ToString('N'))
$ws = Join-Path $sceneHome 'ws'
$runRoot = Join-Path $sceneHome 'run'
New-Item -ItemType Directory -Path $ws, $runRoot -Force | Out-Null

$src = Join-Path $sceneHome 'probe.rs'
[System.IO.File]::WriteAllText($src, 'fn main() { println!("PROBE_OK"); }', $utf8NoBom)
& rustc $src -o (Join-Path $runRoot 'probe.exe')
if ($LASTEXITCODE -ne 0) { throw 'rustc failed to build probe.exe' }

$experience = @{
    name = 's2d_exec_probe'
    trigger = @{ tool = 'exec_command'; command_pattern = 's2d exec probe' }
    preconditions = @(@{ key = 'cwd.exists'; expected = $true })
    workflow = @(@{ action = 'exec'; args = @{ program = 'probe.exe'; args = @() } })
    postconditions = @(@{ key = 'process.exit_code:exec#0'; expected = 0 })
    verification = @()
    failure_policy = 'stop_and_report'
    undo = 'unsupported'
    status = 'active'
}
$policy = @{
    __global__ = @{
        fs_read = @{ enabled = $true; workspace_only = $true; max_bytes = 20480 }
        fs_write = 'workspace_only'
        fs_delete = 'deny'
        exec = @{ mode = 'allowlist'; allow = @('probe.exe'); timeout_secs = 30; output_cap = 20000; allow_legacy_shell = $false }
        network = @{ mode = 'off'; allow = @() }
    }
}
$store = @{
    schema_version = 1
    experiences = @($experience)
    pinned = @(); scopes = @{}; display_names = @{}; user_usage = @{}
    user_confidence = @{}; references = @{}; scope_policies = $policy
}
[System.IO.File]::WriteAllText((Join-Path $sceneHome 'store.json'), ($store | ConvertTo-Json -Depth 16), $utf8NoBom)
$agents = @{
    schema_version = 2
    agents = @(@{
        id='codex'; label='codex'; kind='codex_cli'; mode='managed'
        directory=$CodexDirectory; channel='session'; codex_home=$CodexHome
    })
}
[System.IO.File]::WriteAllText((Join-Path $sceneHome 'agents.json'), ($agents | ConvertTo-Json -Depth 6), $utf8NoBom)

$proc = Start-Process -FilePath $Server -ArgumentList @('--home', $sceneHome, '--port', "$Port") -WindowStyle Hidden -PassThru
$failed = @()
try {
    $ready = $false
    for ($i = 0; $i -lt 25; $i++) {
        try { $null = Invoke-RestMethod "http://127.0.0.1:$Port/api/health" -TimeoutSec 2; $ready = $true; break }
        catch { Start-Sleep -Milliseconds 500 }
    }
    if (-not $ready) { throw 'server not ready' }
    $task = 's2d exec probe. The command work is already done and verified; do NOT run any command or edit any file. Just report the exit-code evidence and stop.'
    $created = Invoke-RestMethod "http://127.0.0.1:$Port/api/sessions" -Method Post `
        -Body (@{ agent_id='codex'; task=$task; cwd=$ws } | ConvertTo-Json) -ContentType 'application/json'
    $state = $null
    $deadline = (Get-Date).AddSeconds($TimeoutSecs)
    while ((Get-Date) -lt $deadline) {
        $state = Invoke-RestMethod "http://127.0.0.1:$Port/api/sessions/$($created.id)"
        if ($state.status -ne 'running') { break }
        Start-Sleep -Seconds 2
    }
    $ledger = @((Get-Content (Join-Path $sceneHome 'learning-l1.json') -Raw -Encoding UTF8 | ConvertFrom-Json).records)
    $exec = @($ledger | Where-Object { $_.record_type -eq 'experience_execution' })
    $execOk = @($exec | Where-Object { $_.outcome -eq 'success' })
    $stdoutInLedger = @($exec | Where-Object { $_.reason -match 'PROBE_OK' })
    $delegated = @($ledger | Where-Object { $_.record_type -eq 'delegate' })
    $pass = $state.status -eq 'completed' -and $execOk.Count -eq 1 -and $stdoutInLedger.Count -eq 1 -and $delegated.Count -ge 1
    Write-Output "s2d status=$($state.status) exec_success=$($execOk.Count) stdout_evidence=$($stdoutInLedger.Count) delegate=$($delegated.Count) => $(if($pass){'PASS'}else{'FAIL'})"
    if (-not $pass) { $failed += 's2d'; $exec | ForEach-Object { Write-Output "  reason=$($_.outcome):$($_.reason)" } }
    Copy-Item (Join-Path $sceneHome 'learning-l1.json') $root -ErrorAction SilentlyContinue
    Copy-Item (Join-Path $sceneHome 'sessions.json') $root -ErrorAction SilentlyContinue
} finally {
    Stop-Process -Id $proc.Id -Force -ErrorAction SilentlyContinue
    Remove-Item -Recurse -Force $sceneHome -ErrorAction SilentlyContinue
}
Write-Output "S2-D REAL: $(if($failed.Count -eq 0){'PASS'}else{'FAIL'})"
Write-Output "artifacts: $root"
if ($failed.Count -gt 0) { exit 1 }

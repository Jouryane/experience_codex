param(
    [string]$Server = "D:\experience_codex\experience-main\target\debug\experience-server.exe",
    [string]$Fixtures = "D:\experience_codex\experience-main\experiences\scenes\s1d_presets.json",
    [string]$CodexDirectory = "D:\experience_codex\codex-main\codex-rs\target\debug",
    [string]$CodexHome = "D:\experience_codex\codex-main\.codex-exp-home",
    [string]$Only = "",
    [int]$TimeoutSecs = 240
)
# S1-d real acceptance (real model, 6 sessions): Tier1 execution happens
# in-process before delegation, and the delegated agent must not redo or
# damage the artifacts. Backup/restore is exercised with the real agent in
# the loop.
$ErrorActionPreference = "Stop"
$cfg = Join-Path $HOME ".codex\config.toml"
$token = $null; $inSection = $false
foreach ($line in Get-Content $cfg) {
    if ($line -match '^\s*\[model_providers\.deepseek\]') { $inSection = $true; continue }
    if ($inSection -and $line -match '^\s*\[') { break }
    if ($inSection -and $line -match 'experimental_bearer_token\s*=\s*"([^"]+)"') { $token = $Matches[1] }
}
if (-not $token) { Write-Error "no deepseek token"; exit 1 }
$env:DEEPSEEK_API_KEY = $token

$fixture = Get-Content $Fixtures -Raw -Encoding UTF8 | ConvertFrom-Json
$utf8NoBom = New-Object System.Text.UTF8Encoding($false)
$stamp = Get-Date -Format 'yyyyMMdd-HHmmss'
$root = "D:\experience_codex\experience-main\scripts\accept-artifacts\s1d-$stamp"
New-Item -ItemType Directory -Path $root -Force | Out-Null

function New-GlobalPolicy([int]$ReadMax, [int]$Grant) {
    $fsRead = @{ enabled = $true; workspace_only = $true; max_bytes = $ReadMax }
    if ($Grant -gt 0) { $fsRead['special_grant_bytes'] = $Grant }
    return @{
        __global__ = @{
            fs_read = $fsRead
            fs_write = 'workspace_only'
            fs_delete = 'deny'
            exec = @{ mode = 'off'; allow = @(); timeout_secs = 60; output_cap = 20000; allow_legacy_shell = $false }
            network = @{ mode = 'off'; allow = @() }
        }
    }
}

function New-DeleteGrantPolicy {
    return @{
        __global__ = @{
            fs_read = @{ enabled = $true; workspace_only = $true; max_bytes = 20480 }
            fs_write = 'workspace_only'
            fs_delete = 'workspace_only'
            exec = @{ mode = 'off'; allow = @(); timeout_secs = 60; output_cap = 20000; allow_legacy_shell = $false }
            network = @{ mode = 'off'; allow = @() }
        }
    }
}

function Invoke-Session([int]$Port, [string]$Task, [string]$Workspace) {
    $body = @{ agent_id = 'codex'; task = $Task; cwd = $Workspace } | ConvertTo-Json
    $created = Invoke-RestMethod "http://127.0.0.1:$Port/api/sessions" -Method Post -Body $body -ContentType 'application/json'
    $sid = $created.id
    $state = $null
    $deadline = (Get-Date).AddSeconds($TimeoutSecs)
    while ((Get-Date) -lt $deadline) {
        $state = Invoke-RestMethod "http://127.0.0.1:$Port/api/sessions/$sid"
        if ($state.status -ne 'running') { break }
        Start-Sleep -Seconds 2
    }
    return @{ id = $sid; state = $state }
}

function Get-Ledger([string]$SceneHome) {
    $path = Join-Path $SceneHome 'learning-l1.json'
    if (-not (Test-Path $path)) { return @() }
    return @((Get-Content $path -Raw -Encoding UTF8 | ConvertFrom-Json).records)
}

function Add-ObserverRule([string]$Task) {
    return $Task + " The file work is already done and verified; do NOT create, edit, rewrite, delete or revert any file. Just report the current state and stop."
}

$scenes = @(
    @{
        id='D1'; exp='s1d_bundle'; task=(Add-ObserverRule 's1d tier1 bundle.')
        seed=@(@{ p='existing.txt'; text='base' })
        policy=$null
    },
    @{
        id='D2'; exp='s1d_deny_delete'; task=(Add-ObserverRule 's1d deny delete.')
        seed=@(@{ p='keep.txt'; text='keep-me' })
        policy=$null
    },
    @{
        id='D3'; exp='s1d_granted_delete'; task=(Add-ObserverRule 's1d granted delete.')
        seed=@(@{ p='victim.txt'; text='delete-me' })
        policy='delete'
    },
    @{
        id='D4'; exp='s1d_overwrite'; task=(Add-ObserverRule 's1d overwrite file.')
        seed=@(@{ p='existing.txt'; text='original-content' })
        policy=$null
    },
    @{
        id='D5a'; exp='s1d_read_large'; task=(Add-ObserverRule 's1d read large file.')
        seed=@(); bigFile=$true; policy='read-grant'
    },
    @{
        id='D5b'; exp='s1d_read_large'; task=(Add-ObserverRule 's1d read large file.')
        seed=@(); bigFile=$true; policy=$null
    },
    @{
        id='D6'; exp='s1d_half_auto'; task='s1d half auto, then append the line AGENT-ADDED to notes.md without removing the existing heading, then stop.'
        seed=@(@{ p='notes.md'; text="# Notes`n" })
        policy=$null
    }
)

$failures = 0
$port = 18880
foreach ($scene in $scenes) {
    if ($Only -and $scene.id -ne $Only) { continue }
    $port++
    $sceneHome = Join-Path $env:TEMP ("exp-s1d-$($scene.id)-" + [guid]::NewGuid().ToString('N'))
    $ws = Join-Path $env:TEMP ("exp-s1d-ws-$($scene.id)-" + [guid]::NewGuid().ToString('N'))
    New-Item -ItemType Directory -Path $sceneHome, $ws -Force | Out-Null
    foreach ($item in $scene.seed) {
        $target = Join-Path $ws $item.p
        $parent = Split-Path $target -Parent
        if (-not (Test-Path $parent)) { New-Item -ItemType Directory -Path $parent -Force | Out-Null }
        [System.IO.File]::WriteAllText($target, $item.text, $utf8NoBom)
    }
    if ($scene.bigFile) {
        $big = 'B' * 65536
        [System.IO.File]::WriteAllText((Join-Path $ws 'big.txt'), $big, $utf8NoBom)
    }

    $experience = $fixture.experiences | Where-Object { $_.name -eq $scene.exp }
    $scopePolicies = switch ($scene.policy) {
        'delete' { New-DeleteGrantPolicy }
        'read-grant' { New-GlobalPolicy 4096 1048576 }
        default { @{} }
    }
    $store = @{
        schema_version = 1
        experiences = @($experience)
        pinned = @()
        scopes = @{}
        display_names = @{}
        user_usage = @{}
        user_confidence = @{}
        references = @{}
        scope_policies = $scopePolicies
    } | ConvertTo-Json -Depth 16
    [System.IO.File]::WriteAllText((Join-Path $sceneHome 'store.json'), $store, $utf8NoBom)
    $agents = @{
        schema_version = 2
        agents = @(@{
            id='codex'; label='codex'; kind='codex_cli'; mode='managed'
            directory=$CodexDirectory; channel='session'; codex_home=$CodexHome
        })
    } | ConvertTo-Json -Depth 6
    [System.IO.File]::WriteAllText((Join-Path $sceneHome 'agents.json'), $agents, $utf8NoBom)

    $errLog = Join-Path $sceneHome 'server.err.log'
    $proc = Start-Process -FilePath $Server -ArgumentList @('--home', $sceneHome, '--port', "$port") -WindowStyle Hidden -PassThru -RedirectStandardError $errLog
    $art = Join-Path $root $scene.id
    New-Item -ItemType Directory -Path $art -Force | Out-Null
    try {
        $ready = $false
        for ($i = 0; $i -lt 20; $i++) {
            try { $null = Invoke-RestMethod "http://127.0.0.1:$port/api/health" -TimeoutSec 2; $ready = $true; break }
            catch { Start-Sleep -Milliseconds 500 }
        }
        if (-not $ready) { throw 'server not ready' }

        $run = Invoke-Session $port $scene.task $ws
        $ledger = Get-Ledger $sceneHome
        $executions = @($ledger | Where-Object { $_.record_type -eq 'experience_execution' })
        $delegates = @($ledger | Where-Object { $_.record_type -eq 'delegate' })
        $pass = $false
        $detail = ""

        switch ($scene.id) {
            'D1' {
                $newFile = Join-Path $ws 'out/new.txt'
                $existing = Get-Content (Join-Path $ws 'existing.txt') -Raw -Encoding UTF8
                $moved = Join-Path $ws 'made/moved.txt'
                $okFiles = (Test-Path $newFile) `
                    -and ((Get-Content $newFile -Raw -Encoding UTF8) -eq 'NEW') `
                    -and ($existing -eq 'base-appended') `
                    -and (Test-Path (Join-Path $ws 'made/sub')) `
                    -and (Test-Path $moved) `
                    -and ((Get-Content $moved -Raw -Encoding UTF8) -eq 'base-appended') `
                    -and -not (Test-Path (Join-Path $ws 'made/copied.txt'))
                $okLedger = @($executions | Where-Object { $_.outcome -eq 'success' }).Count -eq 1
                $pass = $run.state.status -eq 'completed' -and $okFiles -and $okLedger -and $delegates.Count -ge 1
                $detail = "files=$okFiles exec_success=$okLedger delegates=$($delegates.Count)"
            }
            'D2' {
                $keep = Join-Path $ws 'keep.txt'
                $okKeep = (Test-Path $keep) -and ((Get-Content $keep -Raw -Encoding UTF8) -eq 'keep-me')
                $invalid = @($executions | Where-Object { $_.outcome -eq 'invalid' -and $_.reason -match 'fs_delete=deny' })
                $pass = $okKeep -and $invalid.Count -eq 1 -and $run.state.status -eq 'completed'
                $detail = "keep=$okKeep invalid=$($invalid.Count)"
            }
            'D3' {
                $victim = Join-Path $ws 'victim.txt'
                $deleted = -not (Test-Path $victim)
                $backups = Invoke-RestMethod "http://127.0.0.1:$port/api/backups?session=$($run.id)"
                $snapshots = @($backups.backups)
                $manifestHit = $false
                if ($snapshots.Count -ge 1) {
                    $entries = @($snapshots[0].manifest.entries)
                    $manifestHit = @($entries | Where-Object { $_.rel_path -eq 'victim.txt' }).Count -ge 1
                }
                $pass = $deleted -and $manifestHit -and $run.state.status -eq 'completed'
                $detail = "deleted=$deleted backup_entry=$manifestHit snapshots=$($snapshots.Count)"
            }
            'D4' {
                $path = Join-Path $ws 'existing.txt'
                $overwritten = (Get-Content $path -Raw -Encoding UTF8) -eq 'OVERWRITTEN'
                $backups = Invoke-RestMethod "http://127.0.0.1:$port/api/backups?session=$($run.id)"
                $snapshots = @($backups.backups)
                $restoreOk = $false
                if ($snapshots.Count -ge 1) {
                    $body = @{ snapshot = $snapshots[0].snapshot; actor = 'accept-s1d' } | ConvertTo-Json
                    $restore = Invoke-RestMethod "http://127.0.0.1:$port/api/backups/$($run.id)/restore" -Method Post -Body $body -ContentType 'application/json'
                    $restoreOk = $restore.ok -eq $true -and ((Get-Content $path -Raw -Encoding UTF8) -eq 'original-content')
                }
                $restoreAudit = @((Get-Ledger $sceneHome) | Where-Object { $_.record_type -eq 'backup_restored' })
                $pass = $overwritten -and $restoreOk -and $restoreAudit.Count -ge 1
                $detail = "overwritten=$overwritten restored=$restoreOk audit=$($restoreAudit.Count)"
            }
            'D5a' {
                $exec = @($executions | Where-Object { $_.outcome -eq 'success' })
                $pass = $run.state.status -eq 'completed' -and $exec.Count -eq 1
                $detail = "read_success=$($exec.Count)"
            }
            'D5b' {
                $invalid = @($executions | Where-Object { $_.outcome -eq 'invalid' -and $_.reason -match 'above the read cap' })
                $pass = $invalid.Count -eq 1 -and $run.state.status -eq 'completed'
                $detail = "invalid_read=$($invalid.Count)"
            }
            'D6' {
                $notes = Join-Path $ws 'notes.md'
                $content = Get-Content $notes -Raw -Encoding UTF8
                $seeded = $content -match [regex]::Escape('# Notes')
                $agentAdded = $content -match 'AGENT-ADDED'
                $pass = $seeded -and $agentAdded -and $run.state.status -eq 'completed'
                $detail = "seeded=$seeded agent_added=$agentAdded"
            }
        }

        Write-Output "$($scene.id) status=$($run.state.status) $detail => $(if($pass){'PASS'}else{'FAIL'})"
        if (-not $pass) {
            # Keep the final workspace state for diagnosis on failure.
            $snap = Join-Path $art 'workspace'
            New-Item -ItemType Directory -Path $snap -Force | Out-Null
            Copy-Item (Join-Path $ws '*') $snap -Recurse -Force -ErrorAction SilentlyContinue
        }
        if (-not $pass) { $failures++ }
        Copy-Item (Join-Path $sceneHome 'sessions.json') $art -ErrorAction SilentlyContinue
        Copy-Item (Join-Path $sceneHome 'learning-l1.json') $art -ErrorAction SilentlyContinue
        if (Test-Path $errLog) { Copy-Item $errLog $art -ErrorAction SilentlyContinue }
    } catch {
        Write-Output "$($scene.id) ERROR $($_.Exception.Message)"
        $failures++
    } finally {
        Stop-Process -Id $proc.Id -Force -ErrorAction SilentlyContinue
        Remove-Item -Recurse -Force $sceneHome, $ws -ErrorAction SilentlyContinue
    }
}
Write-Output "S1-D REAL: $(if($failures -eq 0){'PASS'}else{"FAIL ($failures)"})"
Write-Output "artifacts: $root"
if ($failures -gt 0) { exit 1 }

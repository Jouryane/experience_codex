param(
    [string]$Server = "D:\experience_codex\experience-main\target\debug\experience-server.exe",
    [int]$Port = 18852
)
# S1-b/S1-c acceptance (no LLM): Tier1 file actions through the shared
# executor, policy denial for delete, and backup/restore of a run.
$ErrorActionPreference = "Stop"
$utf8NoBom = New-Object System.Text.UTF8Encoding($false)

function Post-Json([string]$Url, [hashtable]$Body) {
    $path = Join-Path $sceneHome ("req-" + [guid]::NewGuid().ToString('N') + ".json")
    [System.IO.File]::WriteAllText($path, ($Body | ConvertTo-Json -Depth 10), $utf8NoBom)
    $raw = & curl.exe -s -w "`n%{http_code}" -X POST -H "Content-Type: application/json" --data-binary "@$path" $Url
    $status = $raw[-1]
    $json = ($raw[0..($raw.Count - 2)] -join "`n")
    Remove-Item $path -Force -ErrorAction SilentlyContinue
    return @{ status = [int]$status; raw = $json; json = ($json | ConvertFrom-Json) }
}

function ExperienceJson([string]$Name, [string]$Pattern, [array]$Workflow, [array]$Postconditions) {
    return @{
        name = $Name
        trigger = @{ tool = 'exec_command'; command_pattern = $Pattern }
        preconditions = @(@{ key = 'cwd.exists'; expected = $true })
        workflow = $Workflow
        postconditions = $Postconditions
        verification = @()
        failure_policy = 'stop_and_report'
        undo = 'unsupported'
        status = 'active'
    }
}

$sceneHome = Join-Path $env:TEMP ("exp-s1exec-" + [guid]::NewGuid().ToString('N'))
$workspace = Join-Path $sceneHome 'workspace'
New-Item -ItemType Directory -Path $workspace -Force | Out-Null
[System.IO.File]::WriteAllText((Join-Path $sceneHome 'agents.json'), '{"schema_version":2,"agents":[]}', $utf8NoBom)
[System.IO.File]::WriteAllText((Join-Path $workspace 'existing.txt'), 'base', $utf8NoBom)
[System.IO.File]::WriteAllText((Join-Path $workspace 'keep.txt'), 'keep-me', $utf8NoBom)
# Stub executor: the readiness probe must pass, but the delegated turn itself
# only needs to terminate. L3 (the part under test) runs inline before it.
$stub = Join-Path $sceneHome 'stub.cmd'
[System.IO.File]::WriteAllText($stub, "@echo off`r`nexit /b 0`r`n", $utf8NoBom)

$store = @{
    schema_version = 1
    experiences = @(
        (ExperienceJson 's1_tier1_bundle' 's1 tier1 bundle' @(
            @{ action = 'write_file'; args = @{ path = 'out/new.txt'; content = 'NEW' } },
            @{ action = 'append_file'; args = @{ path = 'existing.txt'; content = '-appended' } },
            @{ action = 'mkdir'; args = @{ path = 'made/sub' } },
            @{ action = 'copy_file'; args = @{ source = 'existing.txt'; target = 'made/copied.txt' } },
            @{ action = 'move_file'; args = @{ source = 'made/copied.txt'; target = 'made/moved.txt' } },
            @{ action = 'read_file'; args = @{ path = 'out/new.txt' } }
        ) @(
            @{ key = 'file:made/moved.txt.exists'; expected = $true }
        )),
        (ExperienceJson 's1_deny_delete' 's1 deny delete' @(
            @{ action = 'delete_file'; args = @{ path = 'keep.txt' } }
        ) @(
            @{ key = 'file:keep.txt.exists'; expected = $false }
        ))
    )
    pinned = @()
    scopes = @{}
    display_names = @{}
    user_usage = @{}
    user_confidence = @{}
    references = @{}
    scope_policies = @{}
}
[System.IO.File]::WriteAllText((Join-Path $sceneHome 'store.json'), ($store | ConvertTo-Json -Depth 12), $utf8NoBom)

$proc = Start-Process -FilePath $Server -ArgumentList @('--home', $sceneHome, '--port', "$Port") -WindowStyle Hidden -PassThru
$base = "http://127.0.0.1:$Port"
$failed = @()
try {
    $ready = $false
    for ($i = 0; $i -lt 30; $i++) {
        try { $null = Invoke-RestMethod "$base/api/health" -TimeoutSec 2; $ready = $true; break }
        catch { Start-Sleep -Milliseconds 400 }
    }
    if (-not $ready) { throw "server not ready on port $Port" }

    $agent = Post-Json "$base/api/agents" @{
        id = 's1exec'
        label = 's1exec'
        kind = 'codex_cli'
        mode = 'managed'
        channel = 'session'
        executable = 'stub.cmd'
        directory = $sceneHome
        codex_home = $sceneHome
    }
    if ($agent.status -notin 200, 201) { $failed += 'agent_create'; Write-Output "agent_error=$($agent.json.error)" }

    $run = Post-Json "$base/api/sessions" @{
        agent_id = 's1exec'
        task = 's1 tier1 bundle'
        cwd = $workspace
    }
    # Wait for the delegated turn to terminate (it may fail; L3 already ran).
    if ($run.status -in 200, 201) {
        for ($i = 0; $i -lt 40; $i++) {
            $state = Invoke-RestMethod "$base/api/sessions/$($run.json.id)"
            if ($state.status -ne 'running') { break }
            Start-Sleep -Milliseconds 500
        }
    }
    $outFile = Join-Path $workspace 'out/new.txt'
    $okTier1 = (Test-Path $outFile) `
        -and ((Get-Content $outFile -Raw) -eq 'NEW') `
        -and ((Get-Content (Join-Path $workspace 'existing.txt') -Raw) -eq 'base-appended') `
        -and (Test-Path (Join-Path $workspace 'made/sub')) `
        -and (Test-Path (Join-Path $workspace 'made/moved.txt')) `
        -and -not (Test-Path (Join-Path $workspace 'made/copied.txt'))
    if (-not ($run.status -in 200, 201 -and $okTier1)) { $failed += 'tier1' }
    Write-Output "tier1_ok=$okTier1 session_status=$($run.status) appended=$((Get-Content (Join-Path $workspace 'existing.txt') -Raw))"
    if ($run.status -notin 200, 201) { Write-Output "run_error=$($run.json.error)" }

    $deny = Post-Json "$base/api/sessions" @{
        agent_id = 's1exec'
        task = 's1 deny delete'
        cwd = $workspace
    }
    $keepText = Get-Content (Join-Path $workspace 'keep.txt') -Raw
    $okDeny = (Test-Path (Join-Path $workspace 'keep.txt')) -and ($keepText -eq 'keep-me')
    if (-not $okDeny) { $failed += 'delete_guard' }
    Write-Output "delete_denied_ok=$okDeny keep_present=$(Test-Path (Join-Path $workspace 'keep.txt'))"

    $sessionId = $run.json.id
    $backups = Invoke-RestMethod "$base/api/backups?session=$sessionId"
    $snapshots = @($backups.backups)
    $okListed = $snapshots.Count -ge 1
    $snapshot = $snapshots[0].snapshot
    $restore = Post-Json "$base/api/backups/$sessionId/restore" @{ snapshot = $snapshot; actor = 'accept-script' }
    $afterRestore = Get-Content (Join-Path $workspace 'existing.txt') -Raw
    $okRestore = $restore.status -eq 200 `
        -and $afterRestore -eq 'base' `
        -and -not (Test-Path $outFile) `
        -and (Test-Path (Join-Path $workspace 'keep.txt'))
    if (-not ($okListed -and $okRestore)) { $failed += 'backup_restore' }
    Write-Output "backup_listed=$okListed snapshots=$($snapshots.Count) restore_status=$($restore.status) after_restore=$afterRestore fresh_removed=$(-not (Test-Path $outFile))"

    $restoreAudit = Invoke-RestMethod "$base/api/audit?record_type=backup_restored"
    $execAudit = Invoke-RestMethod "$base/api/audit?record_type=experience_execution"
    $okAudit = @($restoreAudit.records).Count -ge 1 -and @($execAudit.records).Count -ge 1
    if (-not $okAudit) { $failed += 'audit' }
    Write-Output "audit_ok=$okAudit restores=$(@($restoreAudit.records).Count) executions=$(@($execAudit.records).Count)"

    if ($failed.Count -eq 0) {
        Write-Output "S1-B/S1-C ACCEPTANCE: PASS"
    } else {
        Write-Output "S1-B/S1-C ACCEPTANCE: FAIL ($($failed -join ','))"
        exit 1
    }
} finally {
    Stop-Process -Id $proc.Id -Force -ErrorAction SilentlyContinue
    Remove-Item -Recurse -Force $sceneHome -ErrorAction SilentlyContinue
}

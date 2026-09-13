param(
    [string]$Server = "D:\experience_codex\experience-main\target\debug\experience-server.exe",
    [int]$Port = 18842
)
# S1-a capability policy smoke (no LLM): defaults, scope resolution, special
# read grant, rejection of widening/invalid policies, HTTP 400 visibility, and
# the policy_updated audit record.
$ErrorActionPreference = "Stop"
$utf8NoBom = New-Object System.Text.UTF8Encoding($false)

function Post-Json([string]$Url, [hashtable]$Body) {
    $path = Join-Path $phaseHome ("req-" + [guid]::NewGuid().ToString('N') + ".json")
    [System.IO.File]::WriteAllText($path, ($Body | ConvertTo-Json -Depth 8), $utf8NoBom)
    $raw = & curl.exe -s -w "`n%{http_code}" -X POST -H "Content-Type: application/json" --data-binary "@$path" $Url
    $status = $raw[-1]
    $json = ($raw[0..($raw.Count - 2)] -join "`n")
    Remove-Item $path -Force -ErrorAction SilentlyContinue
    return @{ status = [int]$status; raw = $json; json = ($json | ConvertFrom-Json) }
}

function Put-Json([string]$Url, [hashtable]$Body) {
    $path = Join-Path $phaseHome ("req-" + [guid]::NewGuid().ToString('N') + ".json")
    [System.IO.File]::WriteAllText($path, ($Body | ConvertTo-Json -Depth 8), $utf8NoBom)
    $raw = & curl.exe -s -w "`n%{http_code}" -X PUT -H "Content-Type: application/json" --data-binary "@$path" $Url
    $status = $raw[-1]
    $json = ($raw[0..($raw.Count - 2)] -join "`n")
    Remove-Item $path -Force -ErrorAction SilentlyContinue
    return @{ status = [int]$status; raw = $json; json = ($json | ConvertFrom-Json) }
}

function PolicyBody(
    [string]$Scope,
    [string]$FsWrite,
    [string]$FsDelete,
    [string]$ExecMode,
    [string[]]$ExecAllow,
    [int]$ReadMax = 20480,
    $SpecialGrant = $null
) {
    $fsRead = @{ enabled = $true; workspace_only = $true; max_bytes = $ReadMax }
    if ($null -ne $SpecialGrant) { $fsRead['special_grant_bytes'] = $SpecialGrant }
    return @{
        scope = $Scope
        actor = 'accept-script'
        policy = @{
            fs_read = $fsRead
            fs_write = $FsWrite
            fs_delete = $FsDelete
            exec = @{ mode = $ExecMode; allow = $ExecAllow; timeout_secs = 60; output_cap = 20000; allow_legacy_shell = $false }
            network = @{ mode = 'off'; allow = @() }
        }
    }
}

$phaseHome = Join-Path $env:TEMP ("exp-s1-" + [guid]::NewGuid().ToString('N'))
New-Item -ItemType Directory -Path $phaseHome -Force | Out-Null
[System.IO.File]::WriteAllText((Join-Path $phaseHome 'agents.json'), '{"schema_version":2,"agents":[]}', $utf8NoBom)
$proc = Start-Process -FilePath $Server -ArgumentList @('--home', $phaseHome, '--port', "$Port") -WindowStyle Hidden -PassThru
$failed = @()
try {
    $ready = $false
    for ($i = 0; $i -lt 30; $i++) {
        try { $null = Invoke-RestMethod "http://127.0.0.1:$Port/api/health" -TimeoutSec 2; $ready = $true; break }
        catch { Start-Sleep -Milliseconds 400 }
    }
    if (-not $ready) { throw 'server not ready' }

    # 1. Built-in defaults.
    $base = Invoke-RestMethod "http://127.0.0.1:$Port/api/policy"
    $okDefault = $base.scope_has_policy -eq $false `
        -and $base.effective.fs_write -eq 'workspace_only' `
        -and $base.effective.fs_delete -eq 'deny' `
        -and $base.effective.exec.mode -eq 'off' `
        -and $base.effective.network.mode -eq 'off' `
        -and $base.effective.fs_read.max_bytes -eq 20480 `
        -and $null -eq $base.effective.fs_read.special_grant_bytes
    if (-not $okDefault) { $failed += 'defaults' }
    Write-Output "defaults_ok=$okDefault fs_write=$($base.effective.fs_write) exec=$($base.effective.exec.mode) read_max=$($base.effective.fs_read.max_bytes)"

    # 2. Global policy: deny writes, raise the read cap with a special grant.
    $global = Put-Json "http://127.0.0.1:$Port/api/policy" (PolicyBody '__global__' 'deny' 'deny' 'off' @() 40960 1048576)
    $okGlobal = $global.status -eq 200 -and $global.json.ok -eq $true
    $after = Invoke-RestMethod "http://127.0.0.1:$Port/api/policy"
    $okGlobal = $okGlobal -and $after.effective.fs_write -eq 'deny' `
        -and $after.effective.fs_read.special_grant_bytes -eq 1048576
    if (-not $okGlobal) { $failed += 'global_set' }
    Write-Output "global_set_ok=$okGlobal status=$($global.status) effective_write=$($after.effective.fs_write) grant=$($after.effective.fs_read.special_grant_bytes)"

    # 3. Scope fallback + scope override.
    $scopeA = Invoke-RestMethod "http://127.0.0.1:$Port/api/policy?scope=scene-a"
    $okFallback = $scopeA.scope_has_policy -eq $false -and $scopeA.effective.fs_write -eq 'deny'
    $scoped = Put-Json "http://127.0.0.1:$Port/api/policy" (PolicyBody 'scene-a' 'workspace_only' 'deny' 'off' @())
    $scopeAfter = Invoke-RestMethod "http://127.0.0.1:$Port/api/policy?scope=scene-a"
    $okScope = $okFallback -and $scoped.status -eq 200 `
        -and $scopeAfter.scope_has_policy -eq $true `
        -and $scopeAfter.effective.fs_write -eq 'workspace_only' `
        -and $scopeAfter.effective.fs_read.special_grant_bytes -eq $null
    if (-not $okScope) { $failed += 'scope' }
    Write-Output "scope_ok=$okScope fallback_write=$($scopeA.effective.fs_write) scoped_write=$($scopeAfter.effective.fs_write)"

    # 4. Invalid policies are refused with a visible 400.
    $badFamily = Put-Json "http://127.0.0.1:$Port/api/policy" (PolicyBody 'scene-bad' 'workspace_only' 'whatever' 'off' @())
    $badExec = Put-Json "http://127.0.0.1:$Port/api/policy" (PolicyBody 'scene-bad' 'workspace_only' 'deny' 'allowlist' @())
    # A special grant below the base cap is a contradiction and must be refused.
    $badGrant = Put-Json "http://127.0.0.1:$Port/api/policy" (PolicyBody 'scene-bad' 'workspace_only' 'deny' 'off' @() 20480 1024)
    $execPolicy = Put-Json "http://127.0.0.1:$Port/api/policy" (PolicyBody 'scene-exec' 'workspace_only' 'deny' 'allowlist' @('git'))
    $okInvalid = $badFamily.status -eq 400 -and $badFamily.json.error -match 'fs_delete' `
        -and $badExec.status -eq 400 -and $badExec.json.error -match 'allowlist' `
        -and $badGrant.status -eq 400 -and $badGrant.json.error -match 'special_grant_bytes' `
        -and $execPolicy.status -eq 200
    if (-not $okInvalid) { $failed += 'invalid' }
    Write-Output "invalid_ok=$okInvalid bad_family=$($badFamily.status) bad_exec=$($badExec.status) bad_grant=$($badGrant.status) exec_ok=$($execPolicy.status)"

    # 6. Session capability requests must not exceed the scope policy.
    $agent = Post-Json "http://127.0.0.1:$Port/api/agents" @{
        id = 's1-agent'
        label = 's1'
        kind = 'codex_cli'
        mode = 'managed'
        channel = 'exec'
        executable = 'codex'
        directory = $phaseHome
    }
    $sessionBad = Post-Json "http://127.0.0.1:$Port/api/sessions" @{
        agent_id = 's1-agent'
        task = 'create probe file'
        cwd = $phaseHome
        scope = 'scene-a'
        capabilities = @('delete_file')
    }
    $sessionWiden = Post-Json "http://127.0.0.1:$Port/api/sessions" @{
        agent_id = 's1-agent'
        task = 'create probe file'
        cwd = $phaseHome
        scope = 'scene-exec'
        capabilities = @('exec_command')
    }
    # Both negative cases must fail on the policy check itself: delete_file is
    # denied by scene-a, and legacy exec_command is denied even under
    # scene-exec's allowlist mode (legacy shell needs an explicit opt-in).
    $okSession = ($agent.status -in @(200, 201)) `
        -and $sessionBad.status -eq 400 `
        -and $sessionBad.json.error -match 'exceeds the policy' `
        -and $sessionWiden.status -eq 400 `
        -and $sessionWiden.json.error -match 'exceeds the policy'
    $visibleError = $sessionBad.raw -match 'error'
    if (-not $visibleError) { $failed += 'session_visibility' }
    if (-not $okSession) { $failed += 'session_policy' }
    Write-Output "session_policy_ok=$okSession agent=$($agent.status) delete=$($sessionBad.status) exec_cmd=$($sessionWiden.status) msg=$($sessionWiden.json.error)"

    # 7. Audit: policy_updated records with actor + summary, no secrets.
    $audit = Invoke-RestMethod "http://127.0.0.1:$Port/api/audit?record_type=policy_updated"
    $records = @($audit.records)
    # Only accepted mutations are audited: global + scene-a + scene-exec.
    $okAudit = $records.Count -eq 3 `
        -and ($records | Where-Object { $_.candidate_name -eq 'scene-a' }).Count -ge 1 `
        -and ($records | Where-Object { $_.candidate_name -eq '__global__' }).Count -ge 1 `
        -and ($records | Where-Object { $_.reason -match 'actor=accept-script' }).Count -ge 1
    if (-not $okAudit) { $failed += 'audit' }
    Write-Output "audit_ok=$okAudit policy_records=$($records.Count)"

    if ($failed.Count -eq 0) {
        Write-Output "S1-A POLICY ACCEPTANCE: PASS"
    } else {
        Write-Output "S1-A POLICY ACCEPTANCE: FAIL ($($failed -join ','))"
        exit 1
    }
} finally {
    Stop-Process -Id $proc.Id -Force -ErrorAction SilentlyContinue
    Remove-Item -Recurse -Force $phaseHome -ErrorAction SilentlyContinue
}

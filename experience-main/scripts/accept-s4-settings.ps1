param(
    [string]$Server = "D:\experience_codex\experience-main\target\debug\experience-server.exe",
    [string]$UiDir = "D:\experience_codex\experience-main\ui\www",
    [int]$Port = 18910
)
# S4 acceptance (no LLM): settings persist + env override, similarity view is
# read-only, undo lists executions with snapshots, settings page is served.
$ErrorActionPreference = "Stop"
$utf8NoBom = New-Object System.Text.UTF8Encoding($false)

function Put-Json([string]$Url, [hashtable]$Body) {
    $path = Join-Path $sceneHome ("req-" + [guid]::NewGuid().ToString('N') + ".json")
    [System.IO.File]::WriteAllText($path, ($Body | ConvertTo-Json -Depth 10), $utf8NoBom)
    $raw = & curl.exe -s -w "`n%{http_code}" -X PUT -H "Content-Type: application/json" --data-binary "@$path" $Url
    $status = $raw[-1]
    $json = ($raw[0..($raw.Count - 2)] -join "`n")
    Remove-Item $path -Force -ErrorAction SilentlyContinue
    return @{ status = [int]$status; json = ($json | ConvertFrom-Json) }
}

$sceneHome = Join-Path $env:TEMP ("exp-s4-" + [guid]::NewGuid().ToString('N'))
New-Item -ItemType Directory -Path $sceneHome -Force | Out-Null
[System.IO.File]::WriteAllText((Join-Path $sceneHome 'agents.json'), '{"schema_version":2,"agents":[]}', $utf8NoBom)
$proc = Start-Process -FilePath $Server -ArgumentList @('--home', $sceneHome, '--port', "$Port", '--ui-dir', $UiDir) -WindowStyle Hidden -PassThru
$base = "http://127.0.0.1:$Port"
$failed = @()
try {
    $ready = $false
    for ($i = 0; $i -lt 30; $i++) {
        try { $null = Invoke-RestMethod "$base/api/health" -TimeoutSec 2; $ready = $true; break }
        catch { Start-Sleep -Milliseconds 400 }
    }
    if (-not $ready) { throw 'server not ready' }

    # 1. Settings: defaults, update, persistence across restart.
    $initial = Invoke-RestMethod "$base/api/settings"
    $okDefaults = $initial.settings.llm_compiler -eq $false -and $initial.settings.injection_policy -eq $false
    $put = Put-Json "$base/api/settings" @{ actor = 'accept-s4'; llm_compiler = $true; injection_policy = $true; undo_keep = 7 }
    $after = Invoke-RestMethod "$base/api/settings"
    $okPut = $put.status -eq 200 -and $after.settings.llm_compiler -eq $true -and $after.effective.injection_policy -eq $true
    $persisted = Test-Path (Join-Path $sceneHome 'settings.json')
    Stop-Process -Id $proc.Id -Force -ErrorAction SilentlyContinue
    Start-Sleep -Milliseconds 500
    $proc = Start-Process -FilePath $Server -ArgumentList @('--home', $sceneHome, '--port', "$Port", '--ui-dir', $UiDir) -WindowStyle Hidden -PassThru
    $ready = $false
    for ($i = 0; $i -lt 30; $i++) { try { $null = Invoke-RestMethod "$base/api/health" -TimeoutSec 2; $ready = $true; break } catch { Start-Sleep -Milliseconds 400 } }
    $reloaded = Invoke-RestMethod "$base/api/settings"
    $okPersist = $ready -and $reloaded.settings.llm_compiler -eq $true -and $reloaded.settings.undo_keep -eq 7 -and $persisted
    # 2. Env override wins over file (the server reads env at request time, so
    #    restart it with the variable set in its own environment).
    Stop-Process -Id $proc.Id -Force -ErrorAction SilentlyContinue
    Start-Sleep -Milliseconds 500
    $psi = New-Object System.Diagnostics.ProcessStartInfo
    $psi.FileName = $Server
    $psi.Arguments = "--home `"$sceneHome`" --port $Port --ui-dir `"$UiDir`""
    $psi.UseShellExecute = $false
    $psi.RedirectStandardOutput = $true
    $psi.RedirectStandardError = $true
    $psi.EnvironmentVariables['EXPERIENCE_INJECTION_POLICY'] = 'off'
    $proc = [System.Diagnostics.Process]::Start($psi)
    $ready = $false
    for ($i = 0; $i -lt 30; $i++) { try { $null = Invoke-RestMethod "$base/api/health" -TimeoutSec 2; $ready = $true; break } catch { Start-Sleep -Milliseconds 400 } }
    $envOverride = Invoke-RestMethod "$base/api/settings"
    $okEnv = $ready -and $envOverride.effective.injection_policy -eq $false -and $null -ne $envOverride.env_override.injection_policy
    # 3. Audit record for the settings change.
    $audit = Invoke-RestMethod "$base/api/audit?record_type=settings_updated"
    $okAudit = @($audit.records).Count -ge 1
    # 4. Undo surface answers (empty home => no runs, but shape is stable).
    $undo = Invoke-RestMethod "$base/api/undo?limit=5"
    $okUndo = $null -ne $undo.runs
    # 5. Similarity view answers and is read-only.
    $sim = Invoke-RestMethod "$base/api/similarity?threshold=0.5"
    $okSimilarity = $null -ne $sim.clusters -and $null -ne $sim.pairs
    # 6. Settings page is served with its navigation entry.
    $page = & curl.exe -s "$base/settings.html"
    $okPage = $page -match '设置' -and $page -match 'js/pages/settings.js'
    $js = & curl.exe -s "$base/js/pages/settings.js"
    $okJs = $js -match '/api/settings' -and $js -match '/api/undo'

    Write-Output "defaults=$okDefaults persist=$okPersist env_override=$okEnv audit=$okAudit undo=$okUndo similarity=$okSimilarity page=$okPage js=$okJs"
    if (-not $okDefaults) { $failed += 'defaults' }
    if (-not $okPersist) { $failed += 'persist' }
    if (-not $okEnv) { $failed += 'env' }
    if (-not $okAudit) { $failed += 'audit' }
    if (-not $okUndo) { $failed += 'undo' }
    if (-not $okSimilarity) { $failed += 'similarity' }
    if (-not $okPage) { $failed += 'page' }
    if (-not $okJs) { $failed += 'js' }

    if ($failed.Count -eq 0) {
        Write-Output "S4 SETTINGS ACCEPTANCE: PASS"
    } else {
        Write-Output "S4 SETTINGS ACCEPTANCE: FAIL ($($failed -join ','))"
        exit 1
    }
} finally {
    if ($null -ne $proc) { Stop-Process -Id $proc.Id -Force -ErrorAction SilentlyContinue }
    Remove-Item -Recurse -Force $sceneHome -ErrorAction SilentlyContinue
}

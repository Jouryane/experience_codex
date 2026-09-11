param(
    [string]$Server = "D:\experience_codex\experience-main\target\debug\experience-server.exe",
    [string]$CodexDirectory = "D:\experience_codex\codex-main\codex-rs\target\debug",
    [string]$CodexHome = "D:\experience_codex\codex-main\.codex-exp-home",
    [string]$Policy = "on"
)
# Scene group C real acceptance (3 codex sessions, INFERENCE policy ON):
# references are injected as "reference, questionable" context; the agent
# leads and writes the file. Assertions check the produced artifact contains
# the reference patterns plus the injection audit (injected + refs>=1).
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

$utf8NoBom = New-Object System.Text.UTF8Encoding($false)
$oldPolicy = $env:EXPERIENCE_INJECTION_POLICY
if ($Policy -eq 'on') { $env:EXPERIENCE_INJECTION_POLICY = 'on' } else { Remove-Item Env:EXPERIENCE_INJECTION_POLICY -ErrorAction SilentlyContinue }
$stamp = Get-Date -Format 'yyyyMMdd-HHmmss'
$root = "D:\experience_codex\experience-main\scripts\accept-artifacts\scenes-c-$stamp"
New-Item -ItemType Directory -Path $root -Force | Out-Null

function New-Ref([string]$id, [string]$title, [string[]]$steps, [string]$body) {
    return @{
        id = $id; title = $title; scope = $null; tags = @(); body = $body
        steps = $steps; tools_used = @(); plugins_used = @()
        source_agent = 'preset'; declared = $true; trust_level = 'declared'
        evidence_summary = $null; created_at = 1
    }
}

$scenes = @(
    @{
        id='C1'; file='theme.css'; contains=@('--bg','--bg-panel','--text','--accent')
        task='create theme.css in the current directory with :root tokens --bg --bg-panel --text --accent, then stop.'
        refs=@( (New-Ref 'ref_theme_tokens' 'Theme token convention' @('define :root tokens --bg/--bg-panel/--text/--accent','components reference tokens only','verify contrast') 'theme token convention') )
    },
    @{
        id='C2'; file='fetch.js'; contains=@('AppError','retry')
        task='create fetch.js in the current directory with an AppError wrapper and a retry once branch, then stop.'
        refs=@( (New-Ref 'ref_error_convention' 'Error convention' @('wrap failures in AppError','retry once on network errors','keep original error') 'error convention') )
    },
    @{
        id='C3'; file='commit.txt'; contains=@('feat(')
        task='create commit.txt in the current directory with a feat(theme) commit message and a Changed changelog entry, then stop.'
        refs=@( (New-Ref 'ref_commit_template' 'Commit template' @('subject feat(scope): summary','body explains why','changelog Added/Changed') 'commit template') )
    }
)

$failures = 0
$port = 18810
foreach ($scene in $scenes) {
    $port++
    $sceneHome = Join-Path $env:TEMP ("exp-scene-$($scene.id)-" + [guid]::NewGuid().ToString('N'))
    $ws = Join-Path $env:TEMP ("exp-scene-ws-$($scene.id)-" + [guid]::NewGuid().ToString('N'))
    New-Item -ItemType Directory -Path $sceneHome, $ws -Force | Out-Null
    $refMap = @{}
    foreach ($ref in $scene.refs) { $refMap[$ref.id] = $ref }
    $store = @{ schema_version=1; experiences=@(); pinned=@(); scopes=@{}; references=$refMap } | ConvertTo-Json -Depth 12
    [System.IO.File]::WriteAllText((Join-Path $sceneHome 'store.json'), $store, $utf8NoBom)
    $agents = @{ schema_version = 2; agents = @(@{ id='codex'; label='codex'; kind='codex_cli'; mode='managed'; directory=$CodexDirectory; channel='session'; codex_home=$CodexHome }) } | ConvertTo-Json -Depth 6
    [System.IO.File]::WriteAllText((Join-Path $sceneHome 'agents.json'), $agents, $utf8NoBom)

    $proc = Start-Process -FilePath $Server -ArgumentList @('--home', $sceneHome, '--port', "$port") -WindowStyle Hidden -PassThru
    $art = Join-Path $root $scene.id
    New-Item -ItemType Directory -Path $art -Force | Out-Null
    try {
        $ready = $false
        for ($i = 0; $i -lt 20; $i++) { try { $null = Invoke-RestMethod "http://127.0.0.1:$port/api/health" -TimeoutSec 2; $ready = $true; break } catch { Start-Sleep -Milliseconds 500 } }
        if (-not $ready) { throw 'server not ready' }
        $body = @{ agent_id='codex'; task=$scene.task; cwd=$ws } | ConvertTo-Json
        $created = Invoke-RestMethod "http://127.0.0.1:$port/api/sessions" -Method Post -Body $body -ContentType 'application/json'
        $sid = $created.id
        $s = $null
        for ($i = 0; $i -lt 240; $i++) { $s = Invoke-RestMethod "http://127.0.0.1:$port/api/sessions/$sid"; if ($s.status -ne 'running') { break }; Start-Sleep -Seconds 2 }
        $path = Join-Path $ws $scene.file
        $fileOk = Test-Path $path
        $content = if ($fileOk) { Get-Content $path -Raw -Encoding UTF8 } else { '' }
        foreach ($needle in $scene.contains) { if ($content -notmatch [regex]::Escape($needle)) { $fileOk = $false } }
        if ($scene.id -eq 'C3') { if ($content -notmatch 'Changed' -and $content -notmatch 'Added') { $fileOk = $false } }
        $ledger = @((Get-Content (Join-Path $sceneHome 'learning-l1.json') -Raw | ConvertFrom-Json).records)
        $injected = @($ledger | Where-Object { $_.record_type -eq 'injection' -and $_.outcome -eq 'injected' -and $_.reason -match 'refs=' })
        $injectionOk = if ($Policy -eq 'on') { $injected.Count -ge 1 } else { $injected.Count -eq 0 }
        $pass = $s.status -eq 'completed' -and $fileOk -and $injectionOk
        Write-Output "$($scene.id) status=$($s.status) file=$fileOk injected=$($injected.Count) => $(if($pass){'PASS'}else{'FAIL'})"
        if (-not $pass) { $failures++ }
        Copy-Item (Join-Path $sceneHome 'sessions.json') $art -ErrorAction SilentlyContinue
        Copy-Item (Join-Path $sceneHome 'learning-l1.json') $art -ErrorAction SilentlyContinue
    } catch {
        Write-Output "$($scene.id) ERROR $($_.Exception.Message)"
        $failures++
    } finally {
        Stop-Process -Id $proc.Id -Force -ErrorAction SilentlyContinue
        Remove-Item -Recurse -Force $sceneHome, $ws -ErrorAction SilentlyContinue
    }
}
if ($null -eq $oldPolicy) { Remove-Item Env:EXPERIENCE_INJECTION_POLICY -ErrorAction SilentlyContinue } else { $env:EXPERIENCE_INJECTION_POLICY = $oldPolicy }
Write-Output "SCENES C: $(if($failures -eq 0){'PASS'}else{"FAIL ($failures)"})"
Write-Output "artifacts: $root"
if ($failures -gt 0) { exit 1 }

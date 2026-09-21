param(
    [string]$Server = "D:\experience_codex\experience-main\target\debug\experience-server.exe",
    [string]$Presets = "D:\experience_codex\experience-main\experiences\scenes\presets.json",
    [string]$CodexDirectory = "D:\experience_codex\codex-main\codex-rs\target\debug",
    [string]$CodexHome = "D:\experience_codex\codex-main\.codex-exp-home",
    [string]$Only = ""
)
# Scene group A/B real acceptance (6 codex sessions):
#   A1-A3 fully local (write_file experience executes + verifies, agent closes)
#   B1-B3 semi (experience writes skeleton, agent fills the unknown part)
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

$preset = Get-Content $Presets -Raw -Encoding UTF8 | ConvertFrom-Json
$utf8NoBom = New-Object System.Text.UTF8Encoding($false)
$stamp = Get-Date -Format 'yyyyMMdd-HHmmss'
$root = "D:\experience_codex\experience-main\scripts\accept-artifacts\scenes-ab-$stamp"
New-Item -ItemType Directory -Path $root -Force | Out-Null

$scenes = @(
    @{ id='A1'; exp='create_manifest'; task='create project manifest in the current directory, then stop.'; files=@(@{p='manifest.json'; contains=@('preset-app')}) },
    @{ id='A2'; exp='create_editorconfig'; task='create editorconfig in the current directory, then stop.'; files=@(@{p='.editorconfig'; contains=@('root = true')}) },
    @{ id='A3'; exp='scaffold_skeleton'; task='scaffold project skeleton in the current directory, then stop.'; files=@(@{p='src/index.ts'; contains=@('hello')}, @{p='docs/README.md'; contains=@('# Docs')}) },
    @{ id='B1'; exp='readme_skeleton'; task='scaffold readme skeleton in the current directory, then edit README.md so it contains the line ACMEUI: AcmeUI, then stop.'; files=@(@{p='README.md'; contains=@('# Project','AcmeUI')}) },
    @{ id='B2'; exp='gitignore_base'; task='init gitignore base in the current directory, then add python rules, then stop.'; files=@(@{p='.gitignore'; contains=@('node_modules/','__pycache__')}) },
    @{ id='B3'; exp='smoke_test_scaffold'; task='create smoke test scaffold in the current directory, then add a case for function add(), then stop.'; files=@(@{p='tests/smoke.test.js'; contains=@('add')}) }
)

$failures = 0
$port = 18800
foreach ($scene in $scenes) {
    if ($Only -and $scene.id -ne $Only) { continue }
    $port++
    $sceneHome = Join-Path $env:TEMP ("exp-scene-$($scene.id)-" + [guid]::NewGuid().ToString('N'))
    $ws = Join-Path $env:TEMP ("exp-scene-ws-$($scene.id)-" + [guid]::NewGuid().ToString('N'))
    New-Item -ItemType Directory -Path $sceneHome, $ws -Force | Out-Null
    $experience = $preset.experiences | Where-Object { $_.name -eq $scene.exp }
    $store = @{ schema_version = 1; experiences = @($experience); pinned = @(); scopes = @{} } | ConvertTo-Json -Depth 12
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
        $fileOk = $true
        foreach ($check in $scene.files) {
            $path = Join-Path $ws $check.p
            if (-not (Test-Path $path)) { $fileOk = $false; continue }
            $content = Get-Content $path -Raw -Encoding UTF8
            foreach ($needle in $check.contains) { if ($content -notmatch [regex]::Escape($needle)) { $fileOk = $false } }
        }
        $ledger = @((Get-Content (Join-Path $sceneHome 'learning-l1.json') -Raw | ConvertFrom-Json).records)
        $exec = @($ledger | Where-Object { $_.record_type -eq 'experience_execution' -and $_.candidate_name -eq $scene.exp -and $_.outcome -eq 'success' })
        $delegated = @($ledger | Where-Object { $_.record_type -eq 'delegate' })
        $pass = $s.status -eq 'completed' -and $fileOk -and $exec.Count -eq 1 -and $delegated.Count -ge 1
        Write-Output "$($scene.id) status=$($s.status) files=$fileOk exec=$($exec.Count) delegate=$($delegated.Count) => $(if($pass){'PASS'}else{'FAIL'})"
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
Write-Output "SCENES A/B: $(if($failures -eq 0){'PASS'}else{"FAIL ($failures)"})"
Write-Output "artifacts: $root"
if ($failures -gt 0) { exit 1 }

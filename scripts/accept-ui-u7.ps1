param(
    [string]$Server = "D:\experience_codex\experience-main\target\debug\experience-server.exe",
    [string]$Presets = "D:\experience_codex\experience-main\experiences\scenes\presets.json",
    [string]$Chrome = "C:\Program Files\Google\Chrome\Application\chrome.exe",
    [int]$Port = 18830
)
# U7 automated acceptance (no LLM): serve the multi-page UI against a store
# with presets + one reference, then
#   1) HTTP 200 for every page and shared asset;
#   2) headless Chrome executes each page JS (virtual time budget) and the
#      dumped DOM must contain the page's key markers and dynamic data.
$ErrorActionPreference = "Stop"
$utf8NoBom = New-Object System.Text.UTF8Encoding($false)
$stamp = Get-Date -Format 'yyyyMMdd-HHmmss'
$art = "D:\experience_codex\experience-main\scripts\accept-artifacts\ui-u7-$stamp"
New-Item -ItemType Directory -Path $art -Force | Out-Null

$preset = Get-Content $Presets -Raw -Encoding UTF8 | ConvertFrom-Json
$reference = Get-Content "D:\experience_codex\experience-main\crates\experience-core\tests\fixtures\a5\trae-a4-reference.json" -Raw -Encoding UTF8 | ConvertFrom-Json
$refs = @{}; $refs[$reference.id] = $reference
$store = @{ schema_version = 1; experiences = @($preset.experiences); pinned = @(); scopes = @{}; references = $refs } | ConvertTo-Json -Depth 14
$sceneHome = Join-Path $env:TEMP ("exp-ui-u7-" + [guid]::NewGuid().ToString('N'))
New-Item -ItemType Directory -Path $sceneHome -Force | Out-Null
[System.IO.File]::WriteAllText((Join-Path $sceneHome 'store.json'), $store, $utf8NoBom)
$agents = @{ schema_version = 2; agents = @(@{ id='codex'; label='codex'; kind='codex_cli'; mode='managed'; directory='D:\experience_codex\codex-main\codex-rs\target\debug'; channel='session'; codex_home='D:\experience_codex\codex-main\.codex-exp-home' }) } | ConvertTo-Json -Depth 6
[System.IO.File]::WriteAllText((Join-Path $sceneHome 'agents.json'), $agents, $utf8NoBom)

$proc = Start-Process -FilePath $Server -ArgumentList @('--home', $sceneHome, '--port', "$Port") -WindowStyle Hidden -PassThru
try {
    $ready = $false
    for ($i = 0; $i -lt 20; $i++) { try { $null = Invoke-RestMethod "http://127.0.0.1:$Port/api/health" -TimeoutSec 2; $ready = $true; break } catch { Start-Sleep -Milliseconds 500 } }
    if (-not $ready) { throw 'server not ready' }

    $pages = @(
        @{ url='index.html'; markers=@('task-list','agent-select','scope-input') },
        @{ url='experiences.html'; markers=@('experience-tree','tree-search','status-filter','exp-similar','同族经验') },
        @{ url='absorb.html'; markers=@('upload-area','parse-btn','prompt-template','checklist') },
        @{ url='agents.html'; markers=@('agent-list','agent-form','refresh-agents') },
        @{ url='audit.html'; markers=@('audit-name','audit-timeline','refresh-audit') },
        @{ url='settings.html'; markers=@('policy-save','setting-compiler','undo-list','settings.js') }
    )
    $failures = 0
    foreach ($page in $pages) {
        $resp = Invoke-WebRequest "http://127.0.0.1:$Port/$($page.url)" -UseBasicParsing
        $httpOk = $resp.StatusCode -eq 200
        $dump = Join-Path $art ("$($page.url).dom.html")
        $errFile = Join-Path $art ("$($page.url).chrome.log")
        $profile = Join-Path $env:TEMP ("chrome-u7-" + [guid]::NewGuid().ToString('N'))
        $args = @('--headless','--disable-gpu','--no-sandbox',("--user-data-dir=" + $profile),'--virtual-time-budget=8000','--dump-dom',"http://127.0.0.1:$Port/$($page.url)")
        Start-Process -FilePath $Chrome -ArgumentList $args -Wait -RedirectStandardOutput $dump -RedirectStandardError $errFile -WindowStyle Hidden | Out-Null
        $domText = if (Test-Path $dump) { Get-Content $dump -Raw -Encoding UTF8 } else { '' }
        $markerOk = $true
        foreach ($marker in $page.markers) { if ($domText -notmatch [regex]::Escape($marker)) { $markerOk = $false } }
        Write-Output "$($page.url) http=$httpOk markers=$markerOk"
        if (-not ($httpOk -and $markerOk)) { $failures++ }
    }
    foreach ($asset in @('css/base.css','js/api.js','js/pages/absorb.js','js/pages/experiences.js','js/pages/settings.js','css/pages/settings.css')) {
        $r = Invoke-WebRequest "http://127.0.0.1:$Port/$asset" -UseBasicParsing
        Write-Output "asset $asset -> $($r.StatusCode)"
        if ($r.StatusCode -ne 200) { $failures++ }
    }
    $expDom = Get-Content (Join-Path $art 'experiences.html.dom.html') -Raw -Encoding UTF8
    $dynamicOk = $expDom -match 'create_manifest'
    Write-Output "experiences_dynamic_data=$dynamicOk"
    if (-not $dynamicOk) { $failures++ }
    $absorbDom = Get-Content (Join-Path $art 'absorb.html.dom.html') -Raw -Encoding UTF8
    $guideOk = $absorbDom -match 'run-notes'
    Write-Output "absorb_guide_loaded=$guideOk"
    if (-not $guideOk) { $failures++ }

    Write-Output "UI U7 AUTOMATED ACCEPTANCE: $(if($failures -eq 0){'PASS'}else{"FAIL ($failures)"})"
    Write-Output "artifacts: $art"
    if ($failures -gt 0) { exit 1 }
} finally {
    Stop-Process -Id $proc.Id -Force -ErrorAction SilentlyContinue
    Remove-Item -Recurse -Force $sceneHome -ErrorAction SilentlyContinue
}

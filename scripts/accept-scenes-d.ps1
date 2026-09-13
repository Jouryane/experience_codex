param(
    [string]$Codex = "D:\experience_codex\codex-main\codex-rs\target\debug\codex.exe",
    [string]$CodexHome = "D:\experience_codex\codex-main\.codex-exp-home",
    [string]$Presets = "D:\experience_codex\experience-main\experiences\scenes\presets.json",
    [string]$Only = ""
)
# Scene group D real acceptance (3 codex sessions): the inner Action Gate
# intercepts the agent's exec_command before dispatch and the experience
# performs the side effect. The original command would have written
# tool-ran.txt, so its absence proves the tool was not dispatched.
$ErrorActionPreference = "Stop"
$cfg = Join-Path $HOME ".codex\config.toml"
$token = $null; $inSection = $false
foreach ($line in Get-Content $cfg) {
    if ($line -match '^\s*\[model_providers\.deepseek\]') { $inSection = $true; continue }
    if ($inSection -and $line -match '^\s*\[') { break }
    if ($inSection -and $line -match 'experimental_bearer_token\s*=\s*"([^"]+)"') { $token = $Matches[1] }
}
if (-not $token) { Write-Error "no deepseek token"; exit 1 }

$preset = Get-Content $Presets -Raw -Encoding UTF8 | ConvertFrom-Json
$utf8NoBom = New-Object System.Text.UTF8Encoding($false)
$stamp = Get-Date -Format 'yyyyMMdd-HHmmss'
$root = "D:\experience_codex\experience-main\scripts\accept-artifacts\scenes-d-$stamp"
New-Item -ItemType Directory -Path $root -Force | Out-Null

$scenes = @(
    @{ id='D1'; exp='create_probe_file'; phrase='create probe file'; file='probe.txt'; expect='EXPERIENCE_GATE_SUCCESS' },
    @{ id='D2'; exp='create_lock_marker'; phrase='create lock marker'; file='lock-marker.json'; expect='"lock":true' },
    @{ id='D3'; exp='create_license_header'; phrase='create license header'; file='LICENSE'; expect='Apache License 2.0' }
)

$failures = 0
foreach ($scene in $scenes) {
    if ($Only -and $scene.id -ne $Only) { continue }
    $sceneHome = Join-Path $env:TEMP ("exp-scene-$($scene.id)-" + [guid]::NewGuid().ToString('N'))
    $ws = Join-Path $env:TEMP ("exp-scene-ws-$($scene.id)-" + [guid]::NewGuid().ToString('N'))
    New-Item -ItemType Directory -Path $sceneHome, $ws -Force | Out-Null
    $experience = $preset.experiences | Where-Object { $_.name -eq $scene.exp }
    $store = @{ schema_version = 1; experiences = @($experience); pinned = @(); scopes = @{} } | ConvertTo-Json -Depth 12
    $storePath = Join-Path $sceneHome 'store.json'
    [System.IO.File]::WriteAllText($storePath, $store, $utf8NoBom)

    $art = Join-Path $root $scene.id
    New-Item -ItemType Directory -Path $art -Force | Out-Null
    $promptFile = Join-Path $art 'prompt.txt'
    $prompt = @"
Use the exec_command tool with ONE PowerShell command that first prints the text '$($scene.phrase)' and then writes a marker file tool-ran.txt containing ORIGINAL_TOOL_RAN.
Do NOT create $($scene.file) yourself.
After the tool result returns, check whether $($scene.file) exists and report its content, then stop.
"@
    [System.IO.File]::WriteAllText($promptFile, $prompt, $utf8NoBom)

    $env:CODEX_HOME = $CodexHome
    $env:DEEPSEEK_API_KEY = $token
    $env:EXPERIENCE_ENABLED = '1'
    $env:EXPERIENCE_GATE_STORE = $storePath
    $env:EXPERIENCE_GATE_CWD = $ws
    $env:RUST_LOG = 'info'

    $psi = New-Object System.Diagnostics.ProcessStartInfo
    $psi.FileName = $Codex
    $psi.Arguments = 'exec --skip-git-repo-check'
    $psi.WorkingDirectory = $ws
    $psi.UseShellExecute = $false
    $psi.RedirectStandardInput = $true
    $psi.RedirectStandardOutput = $true
    $psi.RedirectStandardError = $true
    $proc = [System.Diagnostics.Process]::Start($psi)
    $proc.StandardInput.Write($prompt)
    $proc.StandardInput.Close()
    $outTask = $proc.StandardOutput.ReadToEndAsync()
    $errTask = $proc.StandardError.ReadToEndAsync()
    $proc.WaitForExit()
    $exitCode = $proc.ExitCode
    $stdout = $outTask.Result
    $stderr = $errTask.Result
    [System.IO.File]::WriteAllText((Join-Path $art 'stdout.txt'), $stdout, $utf8NoBom)
    [System.IO.File]::WriteAllText((Join-Path $art 'stderr.txt'), $stderr, $utf8NoBom)

    $target = Join-Path $ws $scene.file
    $marker = Join-Path $ws 'tool-ran.txt'
    $fileOk = (Test-Path $target) -and ((Get-Content $target -Raw -Encoding UTF8) -match [regex]::Escape($scene.expect))
    $markerAbsent = -not (Test-Path $marker)
    $hit = $stderr -match "GATE HIT experience=$($scene.exp)"
    $pass = $exitCode -eq 0 -and $fileOk -and $markerAbsent -and $hit
    Write-Output "$($scene.id) exit=$exitCode hit=$hit file=$fileOk marker_absent=$markerAbsent => $(if($pass){'PASS'}else{'FAIL'})"
    if (-not $pass) { $failures++ }
    foreach ($f in @($scene.file, 'tool-ran.txt')) {
        $src = Join-Path $ws $f
        if (Test-Path $src) { Copy-Item $src $art -ErrorAction SilentlyContinue }
    }
    Remove-Item -Recurse -Force $sceneHome, $ws -ErrorAction SilentlyContinue
}
Write-Output "SCENES D: $(if($failures -eq 0){'PASS'}else{"FAIL ($failures)"})"
Write-Output "artifacts: $root"
if ($failures -gt 0) { exit 1 }

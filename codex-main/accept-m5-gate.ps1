param(
    [string]$Codex = "D:\experience_codex\codex-main\codex-rs\target\debug\codex.exe",
    [string]$CodexHome = "D:\experience_codex\codex-main\.codex-exp-home",
    [string]$GateStore = "D:\experience_codex\codex-main\fixtures\m5\gate-store.json",
    [string]$WorkspaceRoot = "D:\experience_codex\codex-main\accept-m5-workspace"
)
# WP6/M5 real Gate loop acceptance (four questions + usage audit):
#   1. stderr carries "GATE HIT experience=create_probe_file"
#   2. original exec_command is NOT dispatched -> tool-ran.txt stays absent
#   3. probe.txt is created by the embedded Experience (content
#      EXPERIENCE_GATE_SUCCESS)
#   4. the Codex loop continues and the process exits 0 (model finished)
#   usage: <fresh store dir>/usage.json gains a create_probe_file entry
#          (hits=1, decisions.experience_only=1, one log)
# Each run copies the fixture into its own artifact subdir so usage.json
# always starts empty (repeatable single-run evidence).
# Real budget: exactly 1 codex session per run.
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

New-Item -ItemType Directory -Path $WorkspaceRoot -Force | Out-Null
Get-ChildItem $WorkspaceRoot -Force -ErrorAction SilentlyContinue | Remove-Item -Recurse -Force
$stamp = Get-Date -Format 'yyyyMMdd-HHmmss'
$artifactDir = "D:\experience_codex\codex-main\accept-m5-artifacts\m5-$stamp"
New-Item -ItemType Directory -Path $artifactDir -Force | Out-Null
$utf8NoBom = New-Object System.Text.UTF8Encoding($false)

$storeDir = Join-Path $artifactDir "store"
New-Item -ItemType Directory -Path $storeDir -Force | Out-Null
$storePath = Join-Path $storeDir "store.json"
Copy-Item $GateStore $storePath

$promptFile = Join-Path $artifactDir "prompt.txt"
$prompt = @"
Create the probe file using the exec_command tool with ONE PowerShell command that:
1. prints the text 'create probe file'
2. writes a marker file named tool-ran.txt whose content is ORIGINAL_TOOL_RAN
Do NOT create or modify probe.txt yourself.
After the tool result comes back, check whether probe.txt exists and report its content, then stop.
"@
[System.IO.File]::WriteAllText($promptFile, $prompt, $utf8NoBom)

$env:CODEX_HOME = $CodexHome
$env:DEEPSEEK_API_KEY = $token
$env:EXPERIENCE_ENABLED = "1"
$env:EXPERIENCE_GATE_STORE = $storePath
$env:EXPERIENCE_GATE_CWD = $WorkspaceRoot
$env:RUST_LOG = "info"

$psi = New-Object System.Diagnostics.ProcessStartInfo
$psi.FileName = $Codex
$psi.Arguments = "exec --skip-git-repo-check"
$psi.WorkingDirectory = $WorkspaceRoot
$psi.UseShellExecute = $false
$psi.RedirectStandardInput = $true
$psi.RedirectStandardOutput = $true
$psi.RedirectStandardError = $true
$proc = [System.Diagnostics.Process]::Start($psi)
$proc.StandardInput.Write($prompt)
$proc.StandardInput.Close()
$stdoutTask = $proc.StandardOutput.ReadToEndAsync()
$stderrTask = $proc.StandardError.ReadToEndAsync()
$proc.WaitForExit()
$exitCode = $proc.ExitCode
$stdoutText = $stdoutTask.Result
$stderrText = $stderrTask.Result
Write-Output "exit=$exitCode"

$out = Join-Path $artifactDir "stdout.txt"
$err = Join-Path $artifactDir "stderr.txt"
[System.IO.File]::WriteAllText($out, $stdoutText, $utf8NoBom)
[System.IO.File]::WriteAllText($err, $stderrText, $utf8NoBom)

$probe = Join-Path $WorkspaceRoot "probe.txt"
$marker = Join-Path $WorkspaceRoot "tool-ran.txt"
$hitLogged = $stderrText -match "GATE HIT experience=create_probe_file"
$probeOk = (Test-Path $probe) -and ((Get-Content $probe -Raw).Trim() -eq "EXPERIENCE_GATE_SUCCESS")
$markerAbsent = -not (Test-Path $marker)

Write-Output "gate_hit_logged=$hitLogged"
Write-Output "probe_ok=$probeOk marker_absent=$markerAbsent"
Write-Output "stderr_hit_lines=$((($stderrText -split "`n") | Where-Object { $_ -match 'GATE HIT|experience gate' }).Count)"

$usagePath = Join-Path $storeDir "usage.json"
$usageOk = $false
if (Test-Path $usagePath) {
    $usage = Get-Content $usagePath -Raw -Encoding UTF8 | ConvertFrom-Json
    $entry = $usage.entries.create_probe_file
    $usageOk = $null -ne $entry -and $entry.hits -ge 1 -and $entry.decisions.experience_only -ge 1 -and @($entry.logs).Count -ge 1 -and $entry.misfires -eq 0 -and $entry.invalid_failures -eq 0
    Write-Output "usage_hits=$($entry.hits) band=$($entry.decisions.experience_only) logs=$(@($entry.logs).Count)"
} else {
    Write-Output "usage.json missing"
}

Copy-Item (Join-Path $WorkspaceRoot "probe.txt") $artifactDir -ErrorAction SilentlyContinue
Copy-Item $usagePath $artifactDir -ErrorAction SilentlyContinue

$pass = $exitCode -eq 0 -and $hitLogged -and $probeOk -and $markerAbsent -and $usageOk
Write-Output "M5 REAL GATE ACCEPTANCE: $(if ($pass) { 'PASS' } else { 'FAIL' })"
Write-Output "artifacts: $artifactDir"
if (-not $pass) { exit 1 }

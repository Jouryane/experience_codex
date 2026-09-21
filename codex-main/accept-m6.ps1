param(
    [string]$Codex = "D:\experience_codex\codex-main\codex-rs\target\debug\codex.exe",
    [string]$RealHome = "D:\experience_codex\codex-main\.codex-exp-home",
    [string]$WorkspaceRoot = "D:\experience_codex\codex-main\accept-m6-workspace",
    [string]$ArtifactRoot = "D:\experience_codex\codex-main\accept-m6-artifacts",
    [int]$FakeProviderPort = 18765
)
# M6 acceptance:
#   1. full task -> canonical task gate, zero model requests, verified files,
#      usage band experience_only, backup manifest exists
#   2. mid-turn action -> existing M5 dispatch Gate (native tool not dispatched)
#   3. partial task -> verified prefix, remainder delegated to LLM, correct
#   4. unknown task -> baseline LLM path, no Experience usage
$ErrorActionPreference = "Stop"

function Read-DeepSeekToken {
    param([string]$ConfigPath)
    $token = $null
    $inSection = $false
    foreach ($line in Get-Content $ConfigPath) {
        if ($line -match '^\s*\[model_providers\.deepseek\]') { $inSection = $true; continue }
        if ($inSection -and $line -match '^\s*\[') { break }
        if ($inSection -and $line -match 'experimental_bearer_token\s*=\s*"([^"]+)"') {
            $token = $Matches[1]
        }
    }
    if (-not $token) {
        $userConfig = Join-Path $HOME ".codex\config.toml"
        if (Test-Path $userConfig) {
            $inSection = $false
            foreach ($line in Get-Content $userConfig) {
                if ($line -match '^\s*\[model_providers\.deepseek\]') { $inSection = $true; continue }
                if ($inSection -and $line -match '^\s*\[') { break }
                if ($inSection -and $line -match 'experimental_bearer_token\s*=\s*"([^"]+)"') {
                    $token = $Matches[1]
                }
            }
        }
    }
    if (-not $token) { throw "no deepseek token found" }
    return $token
}

function New-Workspace {
    param([string]$Name)
    $path = Join-Path $WorkspaceRoot $Name
    if (Test-Path $path) { Remove-Item -LiteralPath $path -Recurse -Force }
    New-Item -ItemType Directory -Path $path -Force | Out-Null
    return $path
}

function Invoke-CodexCase {
    param(
        [string]$CodexHome,
        [string]$Workspace,
        [string]$Prompt,
        [hashtable]$ExtraEnv,
        [string]$ArtifactName
    )
    $psi = New-Object System.Diagnostics.ProcessStartInfo
    $psi.FileName = $Codex
    $psi.Arguments = "exec --skip-git-repo-check"
    $psi.WorkingDirectory = $Workspace
    $psi.UseShellExecute = $false
    $psi.RedirectStandardInput = $true
    $psi.RedirectStandardOutput = $true
    $psi.RedirectStandardError = $true
    $psi.Environment["CODEX_HOME"] = $CodexHome
    $psi.Environment["RUST_LOG"] = "info"
    foreach ($key in $ExtraEnv.Keys) {
        if ($null -eq $ExtraEnv[$key]) {
            [void]$psi.Environment.Remove($key)
        } else {
            $psi.Environment[$key] = [string]$ExtraEnv[$key]
        }
    }
    $proc = [System.Diagnostics.Process]::Start($psi)
    $proc.StandardInput.Write($Prompt)
    $proc.StandardInput.Close()
    $stdoutTask = $proc.StandardOutput.ReadToEndAsync()
    $stderrTask = $proc.StandardError.ReadToEndAsync()
    $proc.WaitForExit()
    $stdoutText = $stdoutTask.Result
    $stderrText = $stderrTask.Result
    $utf8NoBom = New-Object System.Text.UTF8Encoding($false)
    [System.IO.File]::WriteAllText((Join-Path $ArtifactRoot "$ArtifactName.stdout.txt"), $stdoutText, $utf8NoBom)
    [System.IO.File]::WriteAllText((Join-Path $ArtifactRoot "$ArtifactName.stderr.txt"), $stderrText, $utf8NoBom)
    Write-Output "case=$ArtifactName exit=$($proc.ExitCode)"
    return [pscustomobject]@{
        ExitCode = $proc.ExitCode
        Stdout   = $stdoutText
        Stderr   = $stderrText
    }
}

function Assert-True {
    param([bool]$Condition, [string]$Message)
    if (-not $Condition) { throw "ASSERT FAILED: $Message" }
}

function File-Equals {
    param([string]$Path, [string]$Expected)
    return (Test-Path $Path) -and ((Get-Content -Raw -LiteralPath $Path) -eq $Expected)
}

New-Item -ItemType Directory -Path $WorkspaceRoot -Force | Out-Null
New-Item -ItemType Directory -Path $ArtifactRoot -Force | Out-Null
$stamp = Get-Date -Format 'yyyyMMdd-HHmmss'
$ArtifactRoot = Join-Path $ArtifactRoot "m6-$stamp"
New-Item -ItemType Directory -Path $ArtifactRoot -Force | Out-Null

$token = Read-DeepSeekToken (Join-Path $RealHome "config.toml")
$fakeLog = Join-Path $ArtifactRoot "fake-provider.jsonl"
$fakeHome = Join-Path $ArtifactRoot "fake-home"
New-Item -ItemType Directory -Path $fakeHome -Force | Out-Null

$results = [ordered]@{}
$fakeProcess = $null
try {
    # ------------------------------------------------ 1. full task, zero LLM
    $fullWs = New-Workspace "full"
    $fullBackup = Join-Path $ArtifactRoot "full-backup"
    New-Item -ItemType Directory -Path $fullBackup -Force | Out-Null
    $fullStoreDir = Join-Path $ArtifactRoot "full-store"
    New-Item -ItemType Directory -Path $fullStoreDir -Force | Out-Null
    $fullStore = Join-Path $fullStoreDir "store.json"
    Copy-Item "fixtures\m6\task-full-store.json" $fullStore
    [System.IO.File]::WriteAllText((Join-Path $fullWs "m6-existing.txt"), "BEFORE`n", (New-Object System.Text.UTF8Encoding($false)))

    $fakeScript = (Resolve-Path "fixtures\m6\fake_provider.py").Path
    $fakeProcess = Start-Process -FilePath "python" `
        -ArgumentList @($fakeScript, [string]$FakeProviderPort, $fakeLog) `
        -PassThru -WindowStyle Hidden
    $healthOk = $false
    for ($i = 0; $i -lt 40; $i++) {
        try {
            $reply = Invoke-WebRequest -UseBasicParsing -Uri "http://127.0.0.1:$FakeProviderPort/health" -TimeoutSec 1
            if ($reply.StatusCode -eq 200) { $healthOk = $true; break }
        } catch {}
        Start-Sleep -Milliseconds 150
    }
    Assert-True $healthOk "fake provider did not start"

    $fakeConfig = @"
model = "deepseek-v4-flash"
model_provider = "m6fake"
model_catalog_json = "$(($RealHome -replace '\\','/'))/models.json"
approval_policy = "never"
sandbox_mode = "danger-full-access"

[model_providers.m6fake]
name = "m6fake"
base_url = "http://127.0.0.1:$FakeProviderPort/v1"
wire_api = "responses"
env_key = "M6_FAKE_KEY"
requires_openai_auth = false

[projects.'$($fullWs.ToLowerInvariant().Replace('\','\\'))']
trust_level = "trusted"
"@
    [System.IO.File]::WriteAllText((Join-Path $fakeHome "config.toml"), $fakeConfig, (New-Object System.Text.UTF8Encoding($false)))

    $full = Invoke-CodexCase -CodexHome $fakeHome -Workspace $fullWs `
        -Prompt "M6 task full: create the required M6 artifacts and stop." `
        -ArtifactName "full" `
        -ExtraEnv @{
            M6_FAKE_KEY = "test"
            EXPERIENCE_ENABLED = "1"
            EXPERIENCE_TASK_GATE = "1"
            EXPERIENCE_GATE_STORE = $fullStore
            EXPERIENCE_GATE_CWD = $fullWs
            EXPERIENCE_GATE_BACKUP_ROOT = $fullBackup
        }
    Assert-True ($full.ExitCode -eq 0) "full task exit code"
    Assert-True (File-Equals (Join-Path $fullWs "m6-full.txt") "M6_FULL_OK`n") "full artifact"
    Assert-True (File-Equals (Join-Path $fullWs "m6-existing.txt") "M6_FULL_OK`n") "full overwrite"
    $fakeRequests = if (Test-Path $fakeLog) { @(Get-Content $fakeLog).Count } else { 0 }
    Assert-True ($fakeRequests -eq 0) "full task must not sample a model (requests=$fakeRequests)"
    $fullUsage = Get-Content -Raw (Join-Path $fullStoreDir "usage.json") | ConvertFrom-Json
    Assert-True ($fullUsage.entries.m6_task_full.hits -ge 1) "full usage hits"
    Assert-True ($fullUsage.entries.m6_task_full.decisions.experience_only -ge 1) "full usage band"
    $backupFiles = @(Get-ChildItem -LiteralPath $fullBackup -Recurse -File -ErrorAction SilentlyContinue)
    Assert-True ($backupFiles.Count -ge 1) "full backup manifest/copy missing"
    $results.full = "PASS"

    # ------------------------------------------------ 2. mid-turn action gate
    Remove-Item Env:EXPERIENCE_TASK_GATE -ErrorAction SilentlyContinue
    Remove-Item Env:EXPERIENCE_GATE_BACKUP_ROOT -ErrorAction SilentlyContinue
    $m5Output = & ".\accept-m5-gate.ps1" -Codex $Codex -CodexHome $RealHome 2>&1
    $m5Text = ($m5Output | Out-String)
    $results.inner = if ($m5Text -match "M5 REAL GATE ACCEPTANCE: PASS") { "PASS" } else { "FAIL" }
    Assert-True ($results.inner -eq "PASS") "M5 inner gate acceptance"

    # ------------------------------------------------ 3. partial task
    $partialWs = New-Workspace "partial"
    $partialStoreDir = Join-Path $ArtifactRoot "partial-store"
    New-Item -ItemType Directory -Path $partialStoreDir -Force | Out-Null
    $partialStore = Join-Path $partialStoreDir "store.json"
    Copy-Item "fixtures\m6\task-partial-store.json" $partialStore
    $partial = Invoke-CodexCase -CodexHome $RealHome -Workspace $partialWs `
        -Prompt "M6 task partial: create the required prefix and then create m6-remaining.txt with content M6_REMAIN_OK.`n" `
        -ArtifactName "partial" `
        -ExtraEnv @{
            DEEPSEEK_API_KEY = $token
            EXPERIENCE_ENABLED = "1"
            EXPERIENCE_TASK_GATE = "1"
            EXPERIENCE_GATE_STORE = $partialStore
            EXPERIENCE_GATE_CWD = $partialWs
        }
    Assert-True ($partial.ExitCode -eq 0) "partial task exit code"
    Assert-True (File-Equals (Join-Path $partialWs "m6-prefix.txt") "M6_PREFIX_OK`n") "partial prefix"
    Assert-True ((Get-Content -Raw (Join-Path $partialWs "m6-remaining.txt")).Trim() -eq "M6_REMAIN_OK") "partial remainder"
    $partialUsage = Get-Content -Raw (Join-Path $partialStoreDir "usage.json") | ConvertFrom-Json
    Assert-True ($partialUsage.entries.m6_task_prefix.decisions.experience_first -ge 1) "partial usage band"
    $results.partial = "PASS"

    # ------------------------------------------------ 4. unknown task baseline
    $unknownWs = New-Workspace "unknown"
    $unknownStoreDir = Join-Path $ArtifactRoot "unknown-store"
    New-Item -ItemType Directory -Path $unknownStoreDir -Force | Out-Null
    $unknownStore = Join-Path $unknownStoreDir "store.json"
    Copy-Item "fixtures\m6\empty-store.json" $unknownStore
    $unknown = Invoke-CodexCase -CodexHome $RealHome -Workspace $unknownWs `
        -Prompt "Create m6-unknown.txt containing exactly M6_UNKNOWN_OK and nothing else.`n" `
        -ArtifactName "unknown" `
        -ExtraEnv @{
            DEEPSEEK_API_KEY = $token
            EXPERIENCE_ENABLED = "1"
            EXPERIENCE_TASK_GATE = "1"
            EXPERIENCE_GATE_STORE = $unknownStore
            EXPERIENCE_GATE_CWD = $unknownWs
        }
    Assert-True ($unknown.ExitCode -eq 0) "unknown task exit code"
    Assert-True ((Get-Content -Raw (Join-Path $unknownWs "m6-unknown.txt")).Trim() -eq "M6_UNKNOWN_OK") "unknown baseline artifact"
    $unknownUsagePath = Join-Path $unknownStoreDir "usage.json"
    $unknownEntries = if (Test-Path $unknownUsagePath) {
        (Get-Content -Raw $unknownUsagePath | ConvertFrom-Json).entries.PSObject.Properties.Count
    } else { 0 }
    Assert-True ($unknownEntries -eq 0) "unknown task must not record Experience usage"
    $results.unknown = "PASS"
}
finally {
    if ($null -ne $fakeProcess -and -not $fakeProcess.HasExited) {
        Stop-Process -Id $fakeProcess.Id -Force -ErrorAction SilentlyContinue
    }
}

$results.GetEnumerator() | ForEach-Object { Write-Output "$($_.Key)=$($_.Value)" }
$pass = ($results.Values -notcontains "FAIL") -and ($results.Count -eq 4)
Write-Output "M6 CANONICAL ACCEPTANCE: $(if ($pass) { 'PASS' } else { 'FAIL' })"
Write-Output "artifacts: $ArtifactRoot"
if (-not $pass) { exit 1 }

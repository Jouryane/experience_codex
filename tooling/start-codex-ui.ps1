<#
Start the REBUILT Codex with the original desktop UI.

How it works (all evidence from the app's own bundle, see
try-official-ui-with-fork.ps1): the desktop app resolves its backend as

    hostConfig.codex_cli_command  (source: "override")
    process.env.CODEX_CLI_PATH    (source: "override")
    its own bundled codex         (source: "primary-runtime")

Setting CODEX_CLI_PATH therefore makes the official UI drive OUR fork, and
CODEX_HOME makes it read/write OUR home. A dedicated --user-data-dir gives the
second instance its own Electron profile, so the installed app (its window,
its single-instance lock, its state) is never involved.

Result: two independent applications sharing nothing.
  * installed Codex : its own package, its own home (%USERPROFILE%\.codex)
  * this one        : the fork binary, %USERPROFILE%\.codex-experience

Usage:
  powershell -NoProfile -ExecutionPolicy Bypass -File start-codex-ui.ps1
  ... -ForkHome <dir>   -Profile <dir>   -DryRun
#>
param(
    [string]$Workspace = "D:\experience_codex",
    [string]$ForkHome = (Join-Path $env:USERPROFILE ".codex-experience"),
    [string]$Profile,
    [switch]$DryRun
)
$ErrorActionPreference = "Stop"

if (-not $Profile) { $Profile = Join-Path $ForkHome "ui-profile" }

$forkExe = Join-Path $Workspace "codex-main\codex-rs\target\debug\codex.exe"
if (-not (Test-Path $forkExe)) {
    throw "fork build not found: $forkExe`nbuild it: cd $Workspace\codex-main\codex-rs; cargo build -p codex-cli"
}
if (-not (Test-Path (Join-Path $ForkHome "config.toml"))) {
    throw "fork home not configured: $ForkHome`nrun once: powershell -NoProfile -ExecutionPolicy Bypass -File `"$Workspace\setup-fork-home.ps1`""
}

# The installed package directory is versioned, so it is discovered rather than
# hard-coded: an app update must not break this shortcut. Enumerating
# C:\Program Files\WindowsApps needs elevation, so the supported API comes
# first, with the running app's own path as a fallback.
$appExe = $null
$installLocation = (Get-AppxPackage -Name OpenAI.Codex -ErrorAction SilentlyContinue |
    Select-Object -First 1 -ExpandProperty InstallLocation)
if ($installLocation) {
    $candidate = Join-Path $installLocation "app\ChatGPT.exe"
    if (Test-Path $candidate) { $appExe = $candidate }
}
if (-not $appExe) {
    # ...\WindowsApps\OpenAI.Codex_<version>\app\ChatGPT.exe is what is running.
    $running = Get-Process -Name ChatGPT -ErrorAction SilentlyContinue |
        Where-Object { $_.Path } |
        Select-Object -First 1 -ExpandProperty Path
    if ($running) { $appExe = $running }
}
if (-not $appExe -or -not (Test-Path $appExe)) {
    throw "the installed Codex desktop UI was not found (Get-AppxPackage OpenAI.Codex returned '$installLocation')"
}

New-Item -ItemType Directory -Path $Profile -Force | Out-Null

$env:CODEX_CLI_PATH = $forkExe
$env:CODEX_HOME = $ForkHome
# Never hand the session to a local app-server daemon: the point is to run the
# fork binary in the foreground of this instance.
$env:CODEX_APP_SERVER_FORCE_CLI = "1"

Write-Host "ui            : $appExe"
Write-Host "backend (fork): $forkExe"
Write-Host "fork home     : $ForkHome"
Write-Host "ui profile    : $Profile"
Write-Host "installed app : untouched ($env:USERPROFILE\.codex)"

if ($DryRun) {
    Write-Host "(dry run: not starting)"
    exit 0
}

Start-Process -FilePath $appExe -ArgumentList @(
    "--user-data-dir=`"$Profile`""
) | Out-Null
Write-Host "started."

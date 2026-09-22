<#
(Re)create the three Desktop shortcuts.

  * 体验版 Codex (UI)   -> start-codex-ui.cmd
                           the original desktop UI, driving the fork backend
                           with the fork's own home
  * 体验版 Codex (终端) -> start-experience-codex.cmd
                           the fork's own terminal UI (CLI work, codex exec)
  * Experience 查看      -> codex.exe experience html --open --store <fork store>
                           the read-only HTML report

All use assets\experience.ico so they do not look like shell scripts.
Run: powershell -NoProfile -ExecutionPolicy Bypass -File assets\make-shortcuts.ps1
#>
param(
    [string]$Workspace = "D:\experience_codex",
    [string]$Desktop = [Environment]::GetFolderPath('Desktop')
)
$ErrorActionPreference = "Stop"

$icon = Join-Path $Workspace "assets\experience.ico"
$codex = Join-Path $Workspace "codex-main\codex-rs\target\debug\codex.exe"
$wrapper = Join-Path $Workspace "start-experience-codex.cmd"
$uiWrapper = Join-Path $Workspace "start-codex-ui.cmd"
$forkHome = Join-Path $env:USERPROFILE ".codex-experience"
foreach ($required in @($icon, $codex, $wrapper, $uiWrapper)) {
    if (-not (Test-Path $required)) { throw "missing: $required" }
}

$shell = New-Object -ComObject WScript.Shell

# The original single name meant "the terminal one"; it is replaced by two
# explicit names so it is always obvious which application starts.
$legacyPath = Join-Path $Desktop "体验版 Codex.lnk"
if (Test-Path $legacyPath) { Remove-Item -LiteralPath $legacyPath -Force }

$uiPath = Join-Path $Desktop "体验版 Codex (UI).lnk"
$ui = $shell.CreateShortcut($uiPath)
$ui.TargetPath = $uiWrapper
$ui.Arguments = ""
$ui.WorkingDirectory = $Workspace
$ui.Description = "改版 Codex：原版桌面 UI + 我们的后端与独立 home"
$ui.IconLocation = "$icon,0"
# 7 = minimised: the wrapper's console should not pop over the UI window.
$ui.WindowStyle = 7
$ui.Save()

$launcherPath = Join-Path $Desktop "体验版 Codex (终端).lnk"
$launcher = $shell.CreateShortcut($launcherPath)
$launcher.TargetPath = $wrapper
$launcher.Arguments = ""
$launcher.WorkingDirectory = $Workspace
$launcher.Description = "改版 Codex 自带的终端界面（命令行 / codex exec）"
$launcher.IconLocation = "$icon,0"
$launcher.Save()

$viewerPath = Join-Path $Desktop "Experience 查看.lnk"
$viewer = $shell.CreateShortcut($viewerPath)
$viewer.TargetPath = $codex
# Explicit store: a shortcut cannot set CODEX_HOME, and the fork must never
# read the installed app's store.
$viewer.Arguments = "experience html --open --store `"$forkHome\experience\store.json`""
$viewer.WorkingDirectory = $Workspace
$viewer.Description = "查看 Experience 记录：置信度 / 执行记录 / 详情（只读 HTML）"
$viewer.IconLocation = "$icon,0"
$viewer.Save()

foreach ($path in @($uiPath, $launcherPath, $viewerPath)) {
    $link = $shell.CreateShortcut($path)
    Write-Host ("{0}" -f (Split-Path $path -Leaf))
    Write-Host ("  target : {0}" -f $link.TargetPath)
    Write-Host ("  args   : {0}" -f $link.Arguments)
    Write-Host ("  icon   : {0}" -f $link.IconLocation)
    Write-Host ("  start  : {0}" -f $link.WorkingDirectory)
    Write-Host ("  exists : {0}" -f (Test-Path $link.TargetPath))
}

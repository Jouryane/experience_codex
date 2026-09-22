<#
Remove the two files an earlier version of this project created inside the
INSTALLED Codex app's home (%USERPROFILE%\.codex\experience):

  store.json            an empty canonical envelope written when the launcher
                        still pointed at the real home
  experience-view.html  a report generated while testing the viewer

Both are obsolete now that the fork has its own home, and leaving them inside
the desktop app's home is exactly the coupling this project is trying to cut.
The `experience` directory itself predates this project and is kept.

Idempotent and read-only apart from those two paths.
Usage: powershell -NoProfile -ExecutionPolicy Bypass -File clean-real-home-leftovers.ps1
#>
param(
    [string]$RealHome = (Join-Path $env:USERPROFILE ".codex"),
    [string]$Workspace = "D:\experience_codex"
)
$ErrorActionPreference = "Stop"

$dir = Join-Path $RealHome "experience"
foreach ($name in @("store.json", "experience-view.html")) {
    $path = Join-Path $dir $name
    if (Test-Path $path) {
        Remove-Item -LiteralPath $path -Force
        Write-Host "removed $path"
    }
    else {
        Write-Host "already absent: $path"
    }
}
$left = @(Get-ChildItem $dir -Force -ErrorAction SilentlyContinue)
Write-Host "remaining in ${dir}: $($left.Count) item(s)"
$left | ForEach-Object { Write-Host "  $($_.Name)" }

# ---- workspace scratch left over from the UI experiments ------------------
# The first UI probe used .ui-fork-profile; the shipping launcher keeps its
# Electron profile inside the fork home instead.
$obsolete = Join-Path $Workspace ".ui-fork-profile"
if (Test-Path $obsolete) {
    Remove-Item -LiteralPath $obsolete -Recurse -Force
    Write-Host "removed obsolete scratch profile $obsolete"
}
else {
    Write-Host "already absent: $obsolete"
}

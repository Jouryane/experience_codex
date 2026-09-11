param(
    [string]$Dist = "D:\experience_codex\experience-main\dist\Experience",
    [switch]$Offline = $true,
    [switch]$Release
)
$ErrorActionPreference = "Stop"

$cargoBin = Join-Path $HOME ".cargo\bin"
if (Test-Path $cargoBin) { $env:PATH = "$cargoBin;$env:PATH" }

$root = Split-Path -Parent $PSScriptRoot
$profile = if ($Release) { "release" } else { "debug" }
$targetDir = Join-Path $root "target\$profile"

Write-Host "=== Build experience-server / experience-launcher ($profile) ==="
$buildArgs = @("build", "-p", "experience-server", "-p", "experience-launcher")
if ($Offline) { $buildArgs += "--offline" }
if ($Release) { $buildArgs += "--release" }
Push-Location $root
& cargo @buildArgs
Pop-Location
if ($LASTEXITCODE -ne 0) { throw "build failed" }

Write-Host "=== Assemble $Dist ==="
if (Test-Path $Dist) {
    try {
        Remove-Item -Recurse -Force $Dist
    } catch {
        # A process may still hold the folder as its working directory;
        # empty it in place instead of removing the directory itself.
        Write-Warning "dist folder in use; emptying contents instead"
        Get-ChildItem $Dist -Force | Remove-Item -Recurse -Force
    }
}
New-Item -ItemType Directory -Path $Dist -Force | Out-Null
New-Item -ItemType Directory -Path (Join-Path $Dist "ui") -Force | Out-Null

Copy-Item (Join-Path $targetDir "experience-server.exe") $Dist
Copy-Item (Join-Path $targetDir "experience-launcher.exe") (Join-Path $Dist "Experience.exe")
Copy-Item (Join-Path $root "ui\www\*") (Join-Path $Dist "ui") -Recurse

$readmeLines = @(
    "Experience (portable)",
    "=====================",
    "",
    "Run: double-click Experience.exe (opens the default browser).",
    "",
    "Layout (launcher only starts; logic/pages live on disk):",
    "- Experience.exe         thin launcher (packaged once)",
    "- experience-server.exe  local API process (rebuild/replace alone)",
    "- ui/                    frontend (edit and refresh browser)",
    "",
    "Data: <parent-of-exe>/.experience-home (override EXPERIENCE_HOME):",
    "  store.json (experiences) / agents.json (executors) / sessions.json",
    "",
    "Configure agents on the Agents page; run tasks on the Tasks page.",
    "CODEX_HOME / secrets are provided by the agent config; Experience",
    "never reads secrets itself."
)
$readme = $readmeLines -join "`r`n"
Set-Content -Path (Join-Path $Dist "README.txt") -Value $readme -Encoding ASCII

Write-Host "=== Done: $Dist ==="
Get-ChildItem $Dist | Select-Object Name, Length

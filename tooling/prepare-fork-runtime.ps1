<#
Make the fork's directory a complete "codex runtime".

The desktop app resolves its helper programs next to the CLI it runs:

    kJ = ["codex-code-mode-host.exe",
          "codex-windows-sandbox-setup.exe",
          "codex-command-runner.exe"]

so pointing CODEX_CLI_PATH at the fork without those siblings makes the app's
"Finishing Windows setup" step fail with
"windows cannot find 'codex-windows-sandbox-setup.exe'".

What this script does:
  * builds codex-windows-sandbox-setup.exe and codex-command-runner.exe from
    OUR fork, so the sandbox tooling matches the backend version;
  * copies codex-code-mode-host.exe from the installed app. That one cannot be
    built here: it links v8 and its build script needs symlink privileges
    (Windows Developer Mode or elevation). The app's own copy is used instead —
    it is the component the app was shipped with, and this is documented rather
    than hidden.

Re-run it after `cargo clean` or an app update.
Usage: powershell -NoProfile -ExecutionPolicy Bypass -File prepare-fork-runtime.ps1
#>
param(
    [string]$Workspace = "D:\experience_codex",
    [string]$ForkHome = (Join-Path $env:USERPROFILE ".codex-experience")
)
$ErrorActionPreference = "Stop"

$crateDir = Join-Path $Workspace "codex-main\codex-rs"
$outDir = Join-Path $crateDir "target\debug"
$forkExe = Join-Path $outDir "codex.exe"
if (-not (Test-Path $forkExe)) {
    throw "fork not built: $forkExe`nbuild it: cd $crateDir; cargo build -p codex-cli"
}

# The build environment is not on a normal user PATH (cargo/rustup live under
# the user profile, the linker comes from w64devkit), so it is set up here.
$toolchain = "stable-x86_64-pc-windows-gnu"
$env:RUSTUP_TOOLCHAIN = $toolchain
$paths = @(
    (Join-Path $env:USERPROFILE ".local\w64devkit\w64devkit\bin"),
    (Join-Path $env:USERPROFILE ".cargo\bin"),
    (Join-Path $env:USERPROFILE ".rustup\toolchains\$toolchain\bin"),
    (Join-Path $env:USERPROFILE ".rustup\toolchains\$toolchain\lib\rustlib\x86_64-pc-windows-gnu\bin\self-contained")
)
foreach ($path in $paths) {
    if ((Test-Path $path) -and ($env:PATH -notlike "*$path*")) {
        $env:PATH = "$path;$env:PATH"
    }
}
$env:CC = "gcc"
if (-not (Get-Command cargo -ErrorAction SilentlyContinue)) {
    throw "cargo not found; install the Rust toolchain or add it to PATH"
}

# 1. sandbox companions, built from the fork -------------------------------
Write-Host "building codex-windows-sandbox (setup + command-runner)..."
Push-Location $crateDir
try {
    & cargo build -p codex-windows-sandbox --jobs 4 --offline
    if ($LASTEXITCODE -ne 0) { throw "cargo build failed with $LASTEXITCODE" }
}
finally {
    Pop-Location
}

# 2. the code-mode host, taken from the installed app ----------------------
$codeModeHost = Join-Path $outDir "codex-code-mode-host.exe"
if (-not (Test-Path $codeModeHost)) {
    $candidates = @()
    $binRoot = Join-Path $env:LOCALAPPDATA "OpenAI\Codex\bin"
    if (Test-Path $binRoot) {
        $candidates += Get-ChildItem $binRoot -Directory -ErrorAction SilentlyContinue |
            ForEach-Object { Join-Path $_.FullName "codex-code-mode-host.exe" }
    }
    $installLocation = (Get-AppxPackage -Name OpenAI.Codex -ErrorAction SilentlyContinue |
        Select-Object -First 1 -ExpandProperty InstallLocation)
    if ($installLocation) {
        $candidates += Join-Path $installLocation "app\resources\codex-code-mode-host.exe"
    }
    $source = $candidates | Where-Object { Test-Path $_ } | Select-Object -First 1
    if (-not $source) { throw "codex-code-mode-host.exe not found; install the Codex desktop app or add the file manually" }
    Copy-Item -LiteralPath $source -Destination $codeModeHost -Force
    Write-Host "copied codex-code-mode-host.exe from the installed app:"
    Write-Host "  $source   (cannot be built here: v8 needs symlink privileges)"
}

# 3. report ----------------------------------------------------------------
Write-Host ""
Write-Host "runtime directory: $outDir"
$expected = @(
    "codex.exe",
    "codex-windows-sandbox-setup.exe",
    "codex-command-runner.exe",
    "codex-code-mode-host.exe"
)
$missing = @()
foreach ($name in $expected) {
    $path = Join-Path $outDir $name
    if (Test-Path $path) {
        Write-Host ("  ok      {0,-36} {1,15:N0} bytes" -f $name, (Get-Item $path).Length)
    }
    else {
        Write-Host ("  MISSING {0}" -f $name)
        $missing += $name
    }
}
Write-Host ""
if ($missing.Count -eq 0) {
    Write-Host "fork runtime complete - the desktop UI's setup step can find its helpers."
}
else {
    Write-Host "still missing: $($missing -join ', ')"
    exit 1
}

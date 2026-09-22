# cargo-check-gnu.ps1
# Run cargo check -p codex-core with GNU toolchain + w64devkit gcc
# Usage: powershell -ExecutionPolicy Bypass -File cargo-check-gnu.ps1

$ErrorActionPreference = "Continue"

# Toolchain override (bypasses rust-toolchain.toml which pins 1.95.0 MSVC)
$env:RUSTUP_TOOLCHAIN = "stable-x86_64-pc-windows-gnu"

# w64devkit gcc 16.2.0 (real C compiler)
$profile = $env:USERPROFILE
$w64Bin = Join-Path $profile ".local\w64devkit\w64devkit\bin"

# Rust GNU toolchain
$rustBin = Join-Path $profile ".rustup\toolchains\stable-x86_64-pc-windows-gnu\bin"
$selfContained = Join-Path $profile ".rustup\toolchains\stable-x86_64-pc-windows-gnu\lib\rustlib\x86_64-pc-windows-gnu\bin\self-contained"
$cargoBin = Join-Path $profile ".cargo\bin"

# Set PATH: w64devkit first so gcc/ar/as are found, then cargo, then rustc, then self-contained (linker)
$env:PATH = "$w64Bin;$cargoBin;$rustBin;$selfContained;$env:PATH"

# Use w64devkit gcc as C compiler
$env:CC = "gcc"

# Use git for fetching deps (avoids libgit2 issues)
$env:CARGO_NET_GIT_FETCH_WITH_CLI = "true"
$env:CARGO_REGISTRIES_CRATES_IO_PROTOCOL = "sparse"

Write-Host "=== Environment ===" -ForegroundColor Cyan
Write-Host "Toolchain: $env:RUSTUP_TOOLCHAIN"
Write-Host "CC: $env:CC"
& gcc --version 2>&1 | Select-Object -First 1
& rustc --version 2>&1
Write-Host ""

Write-Host "=== Running cargo check -p codex-core ===" -ForegroundColor Cyan
cargo check -p codex-core --jobs 4 2>&1

$exitCode = $LASTEXITCODE
Write-Host ""
if ($exitCode -eq 0) {
    Write-Host "SUCCESS: cargo check passed (exit $exitCode)" -ForegroundColor Green
} else {
    Write-Host "FAILED: cargo check failed (exit $exitCode)" -ForegroundColor Red
}
exit $exitCode

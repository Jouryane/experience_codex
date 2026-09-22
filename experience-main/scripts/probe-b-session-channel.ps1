param(
    # Resolved from PATH unless given; a machine-specific path is never baked in.
    [string]$Codex = "",
    [switch]$StartDaemon
)
# Path B probe: discover whether Experience can attach to a REAL codex
# session context (shared local app-server daemon / queue / remote-control).
# Run this from your own terminal. It is READ-ONLY: it lists surfaces and
# sessions; it does NOT queue messages into any session.
$ErrorActionPreference = "Continue"

if (-not $Codex) { $Codex = (Get-Command codex.exe -ErrorAction SilentlyContinue).Source }
if (-not (Test-Path $Codex)) { Write-Host "codex not found: $Codex"; exit 1 }

$logDir = Join-Path $env:TEMP "exp-probe-b"
New-Item -ItemType Directory -Path $logDir -Force | Out-Null

Write-Host "=== Probe B: session-channel surface discovery ==="
Write-Host "codex: $Codex"
& $Codex --version
Write-Host ""

Write-Host "--- 1) app-server daemon help ---"
& $Codex app-server daemon --help 2>&1 | Tee-Object -FilePath (Join-Path $logDir "daemon-help.txt")
Write-Host ""

Write-Host "--- 2) app-server daemon version (is a daemon running?) ---"
& $Codex app-server daemon version 2>&1 | Tee-Object -FilePath (Join-Path $logDir "daemon-version.txt")
Write-Host ""

if ($StartDaemon) {
    Write-Host "--- 2b) app-server daemon start (user opted in) ---"
    & $Codex app-server daemon start 2>&1 | Tee-Object -FilePath (Join-Path $logDir "daemon-start.txt")
    Write-Host ""
    Write-Host "--- 2c) daemon version after start ---"
    & $Codex app-server daemon version 2>&1 | Tee-Object -FilePath (Join-Path $logDir "daemon-version2.txt")
    Write-Host ""
}

Write-Host "--- 3) agents --help (sessions require --remote on this platform) ---"
& $Codex agents --help 2>&1 | Select-Object -First 60 | Tee-Object -FilePath (Join-Path $logDir "agents-help.txt")
Write-Host ""

Write-Host "--- 3b) agents (best effort with unix remote; may need the real endpoint) ---"
& $Codex agents --remote unix:// 2>&1 | Select-Object -First 60 | Tee-Object -FilePath (Join-Path $logDir "agents.txt")
Write-Host ""

Write-Host "--- 4) queue --help (interface only; NOT sending) ---"
& $Codex queue --help 2>&1 | Select-Object -First 45 | Tee-Object -FilePath (Join-Path $logDir "queue-help.txt")
Write-Host ""

Write-Host "--- 5) app-server proxy --help (stdio bridge to the daemon) ---"
& $Codex app-server proxy --help 2>&1 | Select-Object -First 45 | Tee-Object -FilePath (Join-Path $logDir "proxy-help.txt")
Write-Host ""

Write-Host "--- 6) remote-control --help ---"
& $Codex remote-control --help 2>&1 | Select-Object -First 45 | Tee-Object -FilePath (Join-Path $logDir "remote-help.txt")
Write-Host ""

Write-Host "logs saved under: $logDir"
Write-Host "PROBE-B: review outputs above (daemon version, agents --help/--remote, proxy stdio bridge, queue & remote-control)."
Write-Host "If daemon version shows no running daemon, rerun with -StartDaemon."

param(
    [string]$Codex = "C:\Users\15284\AppData\Local\OpenAI\Codex\bin\8e5b6932251c2c1c\codex.exe"
)
# Probe C: wire-level stdio JSON-RPC handshake against the OFFICIAL codex
# app-server, under the REAL user profile. Run in your own terminal.
# Sends initialize -> initialized -> server/diagnostics and prints replies.
$ErrorActionPreference = "Stop"

if (-not (Test-Path $Codex)) { Write-Host "codex not found: $Codex"; exit 1 }

$logDir = Join-Path $env:TEMP "exp-probe-c"
New-Item -ItemType Directory -Path $logDir -Force | Out-Null
$in = Join-Path $logDir "in.txt"
$out = Join-Path $logDir "out.txt"
$err = Join-Path $logDir "err.txt"

$lines = @(
    '{"method":"initialize","id":1,"params":{"clientInfo":{"name":"experience-probe-c","version":"0.0.0"},"capabilities":{}}}',
    '{"method":"initialized"}',
    '{"method":"server/diagnostics","id":2,"params":{}}'
)
$text = ($lines -join "`r`n") + "`r`n"
[System.IO.File]::WriteAllText($in, $text, (New-Object System.Text.UTF8Encoding($false)))
Remove-Item $out, $err -ErrorAction SilentlyContinue

Write-Host "=== Probe C: official codex app-server --stdio JSON-RPC handshake ==="
$p = Start-Process -FilePath $Codex `
    -ArgumentList @("app-server", "--stdio") `
    -RedirectStandardInput $in -RedirectStandardOutput $out -RedirectStandardError $err `
    -NoNewWindow -PassThru

$deadline = (Get-Date).AddSeconds(30)
while (-not $p.HasExited -and (Get-Date) -lt $deadline) { Start-Sleep -Milliseconds 300 }
if (-not $p.HasExited) { Stop-Process -Id $p.Id -Force }

Write-Host "--- stdout ---"
Get-Content $out -Encoding UTF8 -ErrorAction SilentlyContinue
Write-Host "--- stderr tail ---"
Get-Content $err -Tail 20 -Encoding UTF8 -ErrorAction SilentlyContinue
Write-Host "logs: $logDir"

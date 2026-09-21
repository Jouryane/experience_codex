param(
    [string]$Codex = "C:\Users\someuser\AppData\Local\OpenAI\Codex\bin\8e5b6932251c2c1c\codex.exe",
    [string]$Workspace = "D:\experience_codex\experience-main"
)
# Probe D: create a real codex thread over stdio app-server (JSON-RPC).
# Sends initialize -> initialized -> thread/start {cwd}. It does NOT queue
# a message or start a model turn, so no network/model cost. Prints the
# response (thread id etc.).
$ErrorActionPreference = "Stop"

if (-not (Test-Path $Codex)) { Write-Host "codex not found: $Codex"; exit 1 }

$logDir = Join-Path $env:TEMP "exp-probe-d"
New-Item -ItemType Directory -Path $logDir -Force | Out-Null
$in = Join-Path $logDir "in.txt"
$out = Join-Path $logDir "out.txt"
$err = Join-Path $logDir "err.txt"

$messages = @(
    (@{
        method = "initialize"
        id     = 1
        params = @{
            clientInfo   = @{ name = "experience-probe-d"; version = "0.0.0" }
            capabilities = @{ experimentalApi = $true }
        }
    } | ConvertTo-Json -Compress -Depth 8),
    (@{ method = "initialized" } | ConvertTo-Json -Compress),
    (@{
        method = "thread/start"
        id     = 2
        params = @{
            cwd          = $Workspace
            threadSource = "experience-session-channel"
        }
    } | ConvertTo-Json -Compress -Depth 6)
)
$text = ($messages -join "`r`n") + "`r`n"
[System.IO.File]::WriteAllText($in, $text, (New-Object System.Text.UTF8Encoding($false)))
Remove-Item $out, $err -ErrorAction SilentlyContinue

Write-Host "=== Probe D: thread/start over stdio app-server ==="
Write-Host "workspace: $Workspace"
Write-Host "sequence: initialize(experimentalApi) -> initialized -> thread/start(id2)"
$p = Start-Process -FilePath $Codex `
    -ArgumentList @("app-server", "--stdio") `
    -RedirectStandardInput $in -RedirectStandardOutput $out -RedirectStandardError $err `
    -NoNewWindow -PassThru

$deadline = (Get-Date).AddSeconds(60)
while (-not $p.HasExited -and (Get-Date) -lt $deadline) { Start-Sleep -Milliseconds 300 }
$wasRunning = -not $p.HasExited
if ($wasRunning) { Stop-Process -Id $p.Id -Force }
Write-Host "processExited=$(-not $wasRunning) exitCode=$(if ($p.HasExited) { $p.ExitCode } else { 'killed' })"

Write-Host "--- stdout (head/tail) ---"
$content = Get-Content $out -Raw -Encoding UTF8 -ErrorAction SilentlyContinue
if ($content) {
    Write-Host ("stdoutBytes=" + $content.Length)
    Write-Host $content.Substring(0, [Math]::Min(1200, $content.Length))
    if ($content.Length -gt 2000) {
        Write-Host "...[truncated]..."
        Write-Host $content.Substring($content.Length - 800)
    }
} else {
    Write-Host "(stdout empty)"
}
Write-Host "--- stderr tail ---"
Get-Content $err -Tail 20 -Encoding UTF8 -ErrorAction SilentlyContinue
Write-Host "full log: $out"

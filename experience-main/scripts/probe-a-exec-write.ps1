param(
    [string]$Workspace = "D:\experience_codex\experience-main",
    # Resolved from PATH unless given; a machine-specific path is never baked in.
    [string]$Codex = ""
)
# Path A probe: prove headless codex exec can WRITE with a minimal policy
# (workspace-write sandbox + automatic approvals), under the REAL user
# profile. Run this from your own terminal, NOT from Experience.
$ErrorActionPreference = "Continue"

if (-not $Codex) { $Codex = (Get-Command codex.exe -ErrorAction SilentlyContinue).Source }
if (-not (Test-Path $Codex)) { Write-Host "codex not found: $Codex"; exit 1 }
if (-not (Test-Path $Workspace)) { Write-Host "workspace not found: $Workspace"; exit 1 }

$probeFile = Join-Path $Workspace "experience-probe-a.txt"
Remove-Item $probeFile -ErrorAction SilentlyContinue

$logDir = Join-Path $env:TEMP "exp-probe-a"
New-Item -ItemType Directory -Path $logDir -Force | Out-Null
$out = Join-Path $logDir "out.txt"
$err = Join-Path $logDir "err.txt"
Remove-Item $out, $err -ErrorAction SilentlyContinue

$prompt = "In the workspace $Workspace, create a file named experience-probe-a.txt whose content is exactly PROBE_OK, then read it back and report its content. Do not modify anything else."

Write-Host "=== Probe A: codex exec (workspace-write + approve-for-me) ==="
Write-Host "workspace: $Workspace"
Write-Host "policy: --approve-for-me (implies workspace-write sandbox; -s must NOT be passed)"
Write-Host ""

& $Codex exec `
    --approve-for-me `
    -C $Workspace `
    --skip-git-repo-check `
    $prompt 2>&1 | Out-File -FilePath $out -Encoding utf8
$exit = $LASTEXITCODE

Write-Host "exit=$exit"
Write-Host "--- output (tail) ---"
Get-Content $out -Tail 40 -ErrorAction SilentlyContinue

if (Test-Path $probeFile) {
    $content = Get-Content $probeFile -Raw -ErrorAction SilentlyContinue
    Write-Host ""
    Write-Host "file exists: $probeFile"
    Write-Host "content: $content"
    if ($content -match "PROBE_OK") {
        Write-Host "PROBE-A: PASS (headless write works with workspace-write)"
        exit 0
    }
}
Write-Host "PROBE-A: FAIL (see output above)"
exit 1

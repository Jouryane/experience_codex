param(
    [string]$Server = "D:\experience_codex\experience-main\target\debug\experience-server.exe",
    [int]$Port = 18840
)
# P0 security smoke (no LLM): redaction across parse -> run-notes -> store/
# ledger. Path-guard behavior is covered by unit tests (server + runner).
$ErrorActionPreference = "Stop"
$utf8NoBom = New-Object System.Text.UTF8Encoding($false)

$markdown = @'
# P0 leak test

## 目标与约束
API_KEY=sk-live-abc123XYZ789
Authorization: Bearer eyJhbGciOiJIUzI1NiJ9.payload.signature
DATABASE_URL=postgres://appuser:sup3rs3cret@db.internal:5432/app

## 步骤与子步骤
- export TOKEN=ghp_0123456789abcdef0123456789abcdef
- curl https://appuser:sup3rs3cret@example.com/api/v1

## 最终产物与验证
- git diff --stat: 1 file changed
'@

$secrets = @(
    "sk-live-abc123XYZ789",
    "eyJhbGciOiJIUzI1NiJ9.payload.signature",
    "sup3rs3cret",
    "ghp_0123456789abcdef0123456789abcdef"
)

$phaseHome = Join-Path $env:TEMP ("exp-p0-" + [guid]::NewGuid().ToString('N'))
New-Item -ItemType Directory -Path $phaseHome -Force | Out-Null
[System.IO.File]::WriteAllText((Join-Path $phaseHome 'agents.json'), '{"schema_version":2,"agents":[]}', $utf8NoBom)
$proc = Start-Process -FilePath $Server -ArgumentList @('--home', $phaseHome, '--port', "$Port") -WindowStyle Hidden -PassThru
try {
    $ready = $false
    for ($i = 0; $i -lt 20; $i++) { try { $null = Invoke-RestMethod "http://127.0.0.1:$Port/api/health" -TimeoutSec 2; $ready = $true; break } catch { Start-Sleep -Milliseconds 500 } }
    if (-not $ready) { throw 'server not ready' }

    $parseBody = @{ markdown = $markdown; agent = 'p0'; scope = 'scene-p0' } | ConvertTo-Json -Depth 6
    $p = Join-Path $phaseHome 'parse.json'
    [System.IO.File]::WriteAllText($p, $parseBody, $utf8NoBom)
    $parseRaw = & curl.exe -s -X POST -H "Content-Type: application/json" --data-binary "@$p" "http://127.0.0.1:$Port/api/ingestion/parse"
    $parsed = $parseRaw | ConvertFrom-Json
    $parseOk = $parsed.redacted -eq $true -and $parsed.redactions -gt 0 -and $parseRaw -match '<masked:'
    foreach ($secret in $secrets) { if ($parseRaw.Contains($secret)) { $parseOk = $false } }
    Write-Output "parse_redacted=$parseOk redactions=$($parsed.redactions)"

    $runBody = @{ markdown = $markdown; agent = 'p0'; scope = 'scene-p0'; actor = 'user'; reason = 'p0 smoke' } | ConvertTo-Json -Depth 6
    $r = Join-Path $phaseHome 'run.json'
    [System.IO.File]::WriteAllText($r, $runBody, $utf8NoBom)
    $runRaw = & curl.exe -s -X POST -H "Content-Type: application/json" --data-binary "@$r" "http://127.0.0.1:$Port/api/ingestion/run-notes"
    $created = $runRaw | ConvertFrom-Json
    $runOk = $created.ok -eq $true -and $created.redacted -eq $true

    $storeText = Get-Content (Join-Path $phaseHome 'store.json') -Raw -Encoding UTF8
    $ledgerText = Get-Content (Join-Path $phaseHome 'learning-l1.json') -Raw -Encoding UTF8
    $persistedOk = $true
    foreach ($secret in $secrets) {
        if ($storeText.Contains($secret) -or $ledgerText.Contains($secret) -or $runRaw.Contains($secret)) { $persistedOk = $false }
    }
    Write-Output "run_notes_redacted=$runOk persisted_clean=$persistedOk"

    if ($parseOk -and $runOk -and $persistedOk) {
        Write-Output "P0 SECURITY ACCEPTANCE: PASS"
    } else {
        Write-Output "P0 SECURITY ACCEPTANCE: FAIL"
        exit 1
    }
} finally {
    Stop-Process -Id $proc.Id -Force -ErrorAction SilentlyContinue
    Remove-Item -Recurse -Force $phaseHome -ErrorAction SilentlyContinue
}

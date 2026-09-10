param(
    [string]$Server = "D:\experience_codex\experience-main\dist\Experience\experience-server.exe",
    [int]$Port = 18794
)
# U1 HTTP smoke (no LLM): GET /api/audit returns newest-first ledger records
# with record_type/name filters and limit.
$ErrorActionPreference = "Stop"

$phaseHome = Join-Path $env:TEMP ("exp-u1-audit-" + [guid]::NewGuid().ToString('N'))
New-Item -ItemType Directory -Path $phaseHome -Force | Out-Null
$utf8NoBom = New-Object System.Text.UTF8Encoding($false)
$ledger = @'
{
  "schema_version": 1,
  "records": [
    { "record_type": "written", "candidate_name": "cand_a", "outcome": "written", "recorded_at": 1 },
    { "record_type": "delegate", "candidate_name": "create_probe_file", "outcome": "delegated", "recorded_at": 2 },
    { "record_type": "delegation_completed", "candidate_name": "create_probe_file", "outcome": "completed", "recorded_at": 3 },
    { "record_type": "adopted", "candidate_name": "create_probe_file", "outcome": "ok", "recorded_at": 4 }
  ]
}
'@
[System.IO.File]::WriteAllText((Join-Path $phaseHome "learning-l1.json"), $ledger, $utf8NoBom)
[System.IO.File]::WriteAllText((Join-Path $phaseHome "agents.json"), '{"schema_version":2,"agents":[]}', $utf8NoBom)

$proc = Start-Process -FilePath $Server -ArgumentList @("--home", $phaseHome, "--port", "$Port") -WindowStyle Hidden -PassThru
try {
    $ready = $false
    for ($i = 0; $i -lt 20; $i++) {
        try { $null = Invoke-RestMethod -Uri "http://127.0.0.1:$Port/api/health" -TimeoutSec 2; $ready = $true; break } catch { Start-Sleep -Milliseconds 500 }
    }
    if (-not $ready) { throw "server not ready" }

    $all = Invoke-RestMethod -Uri "http://127.0.0.1:$Port/api/audit"
    $ok1 = $all.records.Count -eq 4 -and $all.records[0].record_type -eq "adopted" -and $all.records[3].record_type -eq "written"
    Write-Output "newest_first=$ok1 first=$($all.records[0].record_type)"

    $filtered = Invoke-RestMethod -Uri "http://127.0.0.1:$Port/api/audit?record_type=delegate"
    $ok2 = $filtered.records.Count -eq 1 -and $filtered.records[0].record_type -eq "delegate"
    Write-Output "record_type_filter=$ok2"

    $named = Invoke-RestMethod -Uri "http://127.0.0.1:$Port/api/audit?name=create_probe_file&limit=2"
    $ok3 = $named.records.Count -eq 2
    Write-Output "name_limit=$ok3"

    if ($ok1 -and $ok2 -and $ok3) {
        Write-Output "U1 AUDIT ACCEPTANCE: PASS"
    } else {
        Write-Output "U1 AUDIT ACCEPTANCE: FAIL"
        exit 1
    }
} finally {
    Stop-Process -Id $proc.Id -Force -ErrorAction SilentlyContinue
    Remove-Item -Recurse -Force $phaseHome -ErrorAction SilentlyContinue
}

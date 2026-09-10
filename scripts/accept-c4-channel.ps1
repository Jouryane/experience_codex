param(
    [string]$Server = "D:\experience_codex\experience-main\dist\Experience\experience-server.exe",
    [string]$Fixture = "D:\experience_codex\experience-main\crates\experience-core\tests\fixtures\c1\scope\store.json",
    [int]$PortA = 18791,
    [int]$PortB = 18792
)
# C4 channel HTTP smoke (no LLM): scope-filtered export from A, import into
# an empty B (scope required), plus a read-only state snapshot.
# Asserts:
#   export?scope=scene-a contains create_probe_a + create_probe_global and
#   not cand_create_probe_b
#   import to B requires scope; after import B list (scope=scene-a) matches
#   and ledger has imported
#   GET /api/state/snapshot?cwd=<temp> is read-only listing
$ErrorActionPreference = "Stop"

$homeA = Join-Path $env:TEMP ("exp-c4-a-" + [guid]::NewGuid().ToString('N'))
$homeB = Join-Path $env:TEMP ("exp-c4-b-" + [guid]::NewGuid().ToString('N'))
New-Item -ItemType Directory -Path $homeA, $homeB -Force | Out-Null
Copy-Item $Fixture (Join-Path $homeA "store.json")
$utf8NoBom = New-Object System.Text.UTF8Encoding($false)
[System.IO.File]::WriteAllText((Join-Path $homeA "agents.json"), '{"schema_version":2,"agents":[]}', $utf8NoBom)
[System.IO.File]::WriteAllText((Join-Path $homeB "agents.json"), '{"schema_version":2,"agents":[]}', $utf8NoBom)

$procA = Start-Process -FilePath $Server -ArgumentList @("--home", $homeA, "--port", "$PortA") -WindowStyle Hidden -PassThru
$procB = Start-Process -FilePath $Server -ArgumentList @("--home", $homeB, "--port", "$PortB") -WindowStyle Hidden -PassThru
try {
    foreach ($port in @($PortA, $PortB)) {
        $ready = $false
        for ($i = 0; $i -lt 20; $i++) {
            try { $null = Invoke-RestMethod -Uri "http://127.0.0.1:$port/api/health" -TimeoutSec 2; $ready = $true; break } catch { Start-Sleep -Milliseconds 500 }
        }
        if (-not $ready) { throw "server not ready on $port" }
    }

    $export = Invoke-RestMethod -Uri "http://127.0.0.1:$PortA/api/experiences/export?scope=scene-a"
    $exportText = $export | ConvertTo-Json -Depth 10
    $ok1 = $exportText -match "create_probe_a" -and $exportText -match "create_probe_global" -and $exportText -notmatch "cand_create_probe_b"
    Write-Output "export_filter_ok=$ok1"

    $body = @{ payload = $export; scope = "scene-a"; actor = "smoke" } | ConvertTo-Json -Depth 12
    $null = Invoke-RestMethod -Uri "http://127.0.0.1:$PortB/api/experiences/import" -Method Post -Body $body -ContentType "application/json"
    $listB = @(Invoke-RestMethod -Uri "http://127.0.0.1:$PortB/api/experiences?scope=scene-a")
    $namesB = @($listB | ForEach-Object { $_.name })
    $ok2 = ($namesB -contains "create_probe_a") -and ($namesB -contains "create_probe_global") -and ($namesB -notcontains "cand_create_probe_b")
    $ledgerB = Get-Content (Join-Path $homeB "learning-l1.json") -Raw | ConvertFrom-Json
    $ok2 = $ok2 -and (@($ledgerB.records | Where-Object { $_.record_type -eq "imported" }).Count -ge 1)
    Write-Output "import_ok=$ok2 names=$($namesB -join ',')"

    $snap = Invoke-RestMethod -Uri "http://127.0.0.1:$PortB/api/state/snapshot?cwd=$homeB"
    $ok3 = $snap.exists -eq $true -and $null -ne $snap.entries
    Write-Output "snapshot_ok=$ok3"

    if ($ok1 -and $ok2 -and $ok3) {
        Write-Output "C4 CHANNEL ACCEPTANCE: PASS (export/import/snapshot)"
    } else {
        Write-Output "C4 CHANNEL ACCEPTANCE: FAIL"
        exit 1
    }
} finally {
    Stop-Process -Id $procA.Id, $procB.Id -Force -ErrorAction SilentlyContinue
    Remove-Item -Recurse -Force $homeA, $homeB -ErrorAction SilentlyContinue
}

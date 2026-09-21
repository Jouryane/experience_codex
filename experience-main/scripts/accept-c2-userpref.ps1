param(
    [string]$Server = "D:\experience_codex\experience-main\dist\Experience\experience-server.exe",
    [string]$Fixture = "D:\experience_codex\experience-main\crates\experience-core\tests\fixtures\c2\preference\store.json",
    [int]$Port = 18790
)
# C2.1+C2.2 HTTP smoke (no LLM): display_name alias + user preference
# (usage deny/allow + user confidence). Alias/confidence never touch the
# Experience body or evidence counters; deny is the only wakeup filter hint.
$ErrorActionPreference = "Stop"

$tempHome = Join-Path $env:TEMP ("exp-c2-pref-" + [guid]::NewGuid().ToString('N'))
New-Item -ItemType Directory -Path $tempHome -Force | Out-Null
Copy-Item $Fixture (Join-Path $tempHome "store.json")
$utf8NoBom = New-Object System.Text.UTF8Encoding($false)
[System.IO.File]::WriteAllText((Join-Path $tempHome "agents.json"), '{"schema_version":2,"agents":[]}', $utf8NoBom)

$proc = Start-Process -FilePath $Server -ArgumentList @("--home", $tempHome, "--port", "$Port") -WindowStyle Hidden -PassThru
try {
    $ready = $false
    for ($i = 0; $i -lt 20; $i++) {
        try { $null = Invoke-RestMethod -Uri "http://127.0.0.1:$Port/api/health" -TimeoutSec 2; $ready = $true; break } catch { Start-Sleep -Milliseconds 500 }
    }
    if (-not $ready) { throw "server not ready" }

    $null = Invoke-RestMethod -Uri "http://127.0.0.1:$Port/api/experiences/create_probe_a/display_name" -Method Post -Body '{"display_name":"A-scene-probe (edited)","actor":"alice"}' -ContentType "application/json"
    $null = Invoke-RestMethod -Uri "http://127.0.0.1:$Port/api/experiences/create_probe_a/user-preference" -Method Post -Body '{"usage":"allow","confidence":0.85,"reason":"user-trusted","actor":"alice"}' -ContentType "application/json"
    $detail = Invoke-RestMethod -Uri "http://127.0.0.1:$Port/api/experiences/create_probe_a"
    $ok1 = $detail.display_name -eq "A-scene-probe (edited)" -and $detail.user_usage -eq "allow" -and $detail.user_confidence -eq 0.85
    Write-Output "display_name=$($detail.display_name) usage=$($detail.user_usage) confidence=$($detail.user_confidence)"

    $ledger = Get-Content (Join-Path $tempHome "learning-l1.json") -Raw | ConvertFrom-Json
    $types = @($ledger.records | ForEach-Object { $_.record_type })
    $ok2 = ($types -contains "renamed") -and ($types -contains "preference_updated")
    Write-Output "ledger_types=$($types -join ',')"

    $badRejected = $false
    try {
        $null = Invoke-RestMethod -Uri "http://127.0.0.1:$Port/api/experiences/create_probe_a/user-preference" -Method Post -Body '{"confidence":1.5}' -ContentType "application/json"
    } catch { $badRejected = $true }
    Write-Output "out_of_range_rejected=$badRejected"

    if ($ok1 -and $ok2 -and $badRejected) {
        Write-Output "C2 USER PREFERENCE ACCEPTANCE: PASS (alias + usage/confidence)"
    } else {
        Write-Output "C2 USER PREFERENCE ACCEPTANCE: FAIL"
        exit 1
    }
} finally {
    Stop-Process -Id $proc.Id -Force -ErrorAction SilentlyContinue
    Remove-Item -Recurse -Force $tempHome -ErrorAction SilentlyContinue
}

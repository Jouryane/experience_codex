param(
    [string]$Server = "D:\experience_codex\experience-main\dist\Experience\experience-server.exe",
    [string]$Notes = "D:\experience_codex\experience-main\crates\experience-core\tests\fixtures\a\run-notes\run-notes.md",
    [int]$Port = 18797
)
# Stage A run-notes auto-parse smoke (no LLM):
#   POST /api/ingestion/parse      -> structured draft for UI auto-fill
#   POST /api/ingestion/run-notes  -> parse + persist reference + ledger
$ErrorActionPreference = "Stop"

$phaseHome = Join-Path $env:TEMP ("exp-a-runnotes-" + [guid]::NewGuid().ToString('N'))
New-Item -ItemType Directory -Path $phaseHome -Force | Out-Null
$utf8NoBom = New-Object System.Text.UTF8Encoding($false)
[System.IO.File]::WriteAllText((Join-Path $phaseHome "agents.json"), '{"schema_version":2,"agents":[]}', $utf8NoBom)

$proc = Start-Process -FilePath $Server -ArgumentList @("--home", $phaseHome, "--port", "$Port") -WindowStyle Hidden -PassThru
try {
    $ready = $false
    for ($i = 0; $i -lt 20; $i++) {
        try { $null = Invoke-RestMethod -Uri "http://127.0.0.1:$Port/api/health" -TimeoutSec 2; $ready = $true; break } catch { Start-Sleep -Milliseconds 500 }
    }
    if (-not $ready) { throw "server not ready" }

    $markdown = [string](Get-Content $Notes -Raw -Encoding UTF8)
    $parseBody = @{ markdown = $markdown; agent = "trae"; scope = "scene-frontend" } | ConvertTo-Json -Depth 6
    $parseJsonPath = Join-Path $phaseHome "parse.json"
    [System.IO.File]::WriteAllText($parseJsonPath, $parseBody, $utf8NoBom)
    $parseRaw = & curl.exe -s -X POST -H "Content-Type: application/json" --data-binary "@$parseJsonPath" "http://127.0.0.1:$Port/api/ingestion/parse"
    $parsed = $parseRaw | ConvertFrom-Json
    $ok1 = $parsed.task -match "API" -and @($parsed.materials.steps).Count -ge 6 -and ($parsed.materials.plugins_used -join ",") -match "ui-kit" -and ($parsed.materials.artifacts -join ",") -match "theme.css"
    Write-Output "parse_ok=$ok1 task=$($parsed.task) steps=$(@($parsed.materials.steps).Count)"

    $runBody = @{ markdown = $markdown; agent = "trae"; scope = "scene-frontend"; actor = "user"; reason = "run-notes absorb" } | ConvertTo-Json -Depth 6
    $runJsonPath = Join-Path $phaseHome "run-notes.json"
    [System.IO.File]::WriteAllText($runJsonPath, $runBody, $utf8NoBom)
    $created = (& curl.exe -s -X POST -H "Content-Type: application/json" --data-binary "@$runJsonPath" "http://127.0.0.1:$Port/api/ingestion/run-notes") | ConvertFrom-Json
    $ok2 = $created.ok -eq $true -and $created.reference_id -match "ref_" -and $created.trust_level -eq "declared"
    Write-Output "run_notes_ok=$ok2 id=$($created.reference_id) trust=$($created.trust_level)"

    $list = Invoke-RestMethod -Uri "http://127.0.0.1:$Port/api/references?scope=scene-frontend"
    $ok3 = @($list.references).Count -eq 1 -and @($list.references[0].steps).Count -ge 6
    Write-Output "reference_ok=$ok3"

    $ledger = Get-Content (Join-Path $phaseHome "learning-l1.json") -Raw | ConvertFrom-Json
    $ok4 = @($ledger.records | Where-Object { $_.record_type -eq "ingested" -and $_.reason -match "run-notes" }).Count -ge 1
    Write-Output "ledger_ok=$ok4"

    if ($ok1 -and $ok2 -and $ok3 -and $ok4) {
        Write-Output "A RUN-NOTES ACCEPTANCE: PASS"
    } else {
        Write-Output "A RUN-NOTES ACCEPTANCE: FAIL"
        exit 1
    }
} finally {
    Stop-Process -Id $proc.Id -Force -ErrorAction SilentlyContinue
    Remove-Item -Recurse -Force $phaseHome -ErrorAction SilentlyContinue
}

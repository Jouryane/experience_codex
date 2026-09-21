param(
    [string]$Server = "D:\experience_codex\experience-main\dist\Experience\experience-server.exe",
    [int]$Port = 18796
)
# Stage A HTTP smoke (no LLM): ingestion guide, package -> reference entry,
# reference listing/filtering, and honest promote rejection without the LLM
# compiler enabled.
$ErrorActionPreference = "Stop"

$phaseHome = Join-Path $env:TEMP ("exp-a-absorb-" + [guid]::NewGuid().ToString('N'))
New-Item -ItemType Directory -Path $phaseHome -Force | Out-Null
$utf8NoBom = New-Object System.Text.UTF8Encoding($false)
$badAgent = '{"schema_version":2,"agents":[{"id":"bad","label":"bad","kind":"codex_cli","mode":"managed","executable":"C:\\definitely-not-real\\codex.exe"}]}'
[System.IO.File]::WriteAllText((Join-Path $phaseHome "agents.json"), $badAgent, $utf8NoBom)

$proc = Start-Process -FilePath $Server -ArgumentList @("--home", $phaseHome, "--port", "$Port") -WindowStyle Hidden -PassThru
try {
    $ready = $false
    for ($i = 0; $i -lt 20; $i++) {
        try { $null = Invoke-RestMethod -Uri "http://127.0.0.1:$Port/api/health" -TimeoutSec 2; $ready = $true; break } catch { Start-Sleep -Milliseconds 500 }
    }
    if (-not $ready) { throw "server not ready" }

    $guide = Invoke-RestMethod -Uri "http://127.0.0.1:$Port/api/ingestion/guide?agent=trae"
    $ok1 = $guide.agent -eq "trae" -and $guide.prompt_template -match "run-notes" -and @($guide.checklist).Count -ge 3
    Write-Output "guide_ok=$ok1"

    $package = @{
        source = @{ agent = "trae"; version = "x"; declared = $true }
        task = "dark theme frontend"
        scope = "scene-frontend"
        materials = @{
            run_notes = "scaffold -> deps -> components -> styles -> verify"
            steps = @("scaffold component tree", "install ui kit", "rewrite tokens", "verify screenshot")
            tools_used = @("file-edit", "terminal")
            plugins_used = @("ui-kit")
            artifacts = @("src/theme.css")
        }
        evidence = @{ diff_summary = "git diff --stat: 4 files changed"; verified_files = @(@{ path = "src/theme.css"; sha256 = "abc" }) }
        actor = "user"
        reason = "trae frontend workflow is valuable"
    } | ConvertTo-Json -Depth 8
    $ingested = Invoke-RestMethod -Uri "http://127.0.0.1:$Port/api/ingestion/package" -Method Post -Body $package -ContentType "application/json"
    $ok2 = $ingested.ok -eq $true -and $ingested.reference_id -match "ref_"
    Write-Output "ingested_ok=$ok2 id=$($ingested.reference_id)"

    $list = Invoke-RestMethod -Uri "http://127.0.0.1:$Port/api/references?scope=scene-frontend"
    $ok3 = @($list.references).Count -eq 1 -and $list.references[0].trust_level -eq "workspace_verified"
    Write-Output "reference_list_ok=$ok3"

    $ledger = Get-Content (Join-Path $phaseHome "learning-l1.json") -Raw | ConvertFrom-Json
    $ok4 = @($ledger.records | Where-Object { $_.record_type -eq "ingested" }).Count -ge 1
    Write-Output "ledger_ingested=$ok4"

    $promoteRaw = & curl.exe -s -X POST -H "Content-Type: application/json" -d '{"actor":"user"}' "http://127.0.0.1:$Port/api/references/$($ingested.reference_id)/promote"
    $rejected = $promoteRaw -match "compiler_unavailable" -and $promoteRaw -match "warnings"
    Write-Output "promote_honest_reject=$rejected"

    if ($ok1 -and $ok2 -and $ok3 -and $ok4 -and $rejected) {
        Write-Output "A ABSORB ACCEPTANCE: PASS"
    } else {
        Write-Output "A ABSORB ACCEPTANCE: FAIL"
        exit 1
    }
} finally {
    Stop-Process -Id $proc.Id -Force -ErrorAction SilentlyContinue
    Remove-Item -Recurse -Force $phaseHome -ErrorAction SilentlyContinue
}

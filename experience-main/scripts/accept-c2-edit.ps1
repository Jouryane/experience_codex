param(
    [string]$Server = "D:\experience_codex\experience-main\dist\Experience\experience-server.exe",
    [string]$Fixture = "D:\experience_codex\experience-main\crates\experience-core\tests\fixtures\c2\edit\store.json",
    [int]$Port = 18789
)
# C2 HTTP smoke (no LLM): draft -> edit body -> adopt with actor audit.
# Asserts:
#   1. POST /{name}/draft creates {name}__draft (status draft)
#   2. PUT /{draft}/body applies a new workflow body (draft only)
#   3. POST /{draft}/adopt replaces the original, preserving name/status/
#      scope; draft disappears; ledger has drafted/edited/adopted
#   4. editing a non-draft (original directly) is rejected
$ErrorActionPreference = "Stop"

$tempHome = Join-Path $env:TEMP ("exp-c2-edit-" + [guid]::NewGuid().ToString('N'))
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

    $null = Invoke-RestMethod -Uri "http://127.0.0.1:$Port/api/experiences/create_probe_a/draft" -Method Post -Body '{"actor":"alice"}' -ContentType "application/json"
    $draftName = "create_probe_a__draft"
    $draft = Invoke-RestMethod -Uri "http://127.0.0.1:$Port/api/experiences/$draftName"
    $ok1 = $draft.status -eq "draft"
    Write-Output "draft_created=$ok1"

    $body = @{
        name = "create_probe_a__draft"
        trigger = @{ tool = "exec_command"; command_pattern = "create probe file" }
        preconditions = @(@{ key = "cwd.exists"; expected = $true })
        workflow = @(@{ action = "write_file"; args = @{ path = "probe-new.txt"; content = "A-NEW" } })
        postconditions = @(
            @{ key = "file:probe-new.txt.exists"; expected = $true },
            @{ key = "file:probe-new.txt.content"; expected = "A-NEW" }
        )
        verification = @()
        failure_policy = "stop_and_report"
        undo = "unsupported"
        status = "draft"
    } | ConvertTo-Json -Depth 8
    $null = Invoke-RestMethod -Uri "http://127.0.0.1:$Port/api/experiences/$draftName/body" -Method Put -Body $body -ContentType "application/json"

    $null = Invoke-RestMethod -Uri "http://127.0.0.1:$Port/api/experiences/$draftName/adopt" -Method Post -Body '{"actor":"alice"}' -ContentType "application/json"
    $original = Invoke-RestMethod -Uri "http://127.0.0.1:$Port/api/experiences/create_probe_a"
    $ok2 = $original.status -eq "active" -and $original.scope -eq "scene-a" -and $original.workflow[0].args.content -eq "A-NEW"
    $draftGone = $true
    try { $null = Invoke-RestMethod -Uri "http://127.0.0.1:$Port/api/experiences/$draftName"; $draftGone = $false } catch { }
    Write-Output "adopt_ok=$ok2 draft_removed=$draftGone"

    $ledger = Get-Content (Join-Path $tempHome "learning-l1.json") -Raw | ConvertFrom-Json
    $types = @($ledger.records | ForEach-Object { $_.record_type })
    $ok3 = ($types -contains "drafted") -and ($types -contains "edited") -and ($types -contains "adopted")
    Write-Output "ledger_types=$($types -join ',')"

    $rejected = $false
    try {
        $null = Invoke-RestMethod -Uri "http://127.0.0.1:$Port/api/experiences/create_probe_a/body" -Method Put -Body $body -ContentType "application/json"
    } catch { $rejected = $true }
    Write-Output "direct_edit_rejected=$rejected"

    if ($ok1 -and $ok2 -and $draftGone -and $ok3 -and $rejected) {
        Write-Output "C2 EDIT ACCEPTANCE: PASS (draft -> body -> adopt, audit)"
    } else {
        Write-Output "C2 EDIT ACCEPTANCE: FAIL"
        exit 1
    }
} finally {
    Stop-Process -Id $proc.Id -Force -ErrorAction SilentlyContinue
    Remove-Item -Recurse -Force $tempHome -ErrorAction SilentlyContinue
}

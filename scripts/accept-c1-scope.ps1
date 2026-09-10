param(
    [string]$Server = "D:\experience_codex\experience-main\dist\Experience\experience-server.exe",
    [string]$Fixture = "D:\experience_codex\experience-main\crates\experience-core\tests\fixtures\c1\scope\store.json",
    [int]$Port = 18788
)
# C1 HTTP smoke (no LLM, real budget 0): scope filtering, scope mutation and
# experience-tree/filter responses against the fixture store.
# Asserts:
#   GET /api/experiences?scope=scene-a -> create_probe_a + create_probe_global
#   POST /api/experiences/create_probe_global/scope {"scope":"scene-a"} ->
#     summary.scope == scene-a
#   GET /api/experiences?scope=scene-b -> candidate + global (global unscoped)
#   GET /api/experience-tree?scope=scene-a -> contains scene-a & family nodes
$ErrorActionPreference = "Stop"

$tempHome = Join-Path $env:TEMP ("exp-c1-scope-" + [guid]::NewGuid().ToString('N'))
New-Item -ItemType Directory -Path $tempHome -Force | Out-Null
Copy-Item $Fixture (Join-Path $tempHome "store.json")
$utf8NoBom = New-Object System.Text.UTF8Encoding($false)
$agents = @{
    schema_version = 2
    agents = @()
} | ConvertTo-Json -Depth 5
[System.IO.File]::WriteAllText((Join-Path $tempHome "agents.json"), $agents, $utf8NoBom)

$proc = Start-Process -FilePath $Server -ArgumentList @("--home", $tempHome, "--port", "$Port") -WindowStyle Hidden -PassThru
try {
    $ready = $false
    for ($i = 0; $i -lt 20; $i++) {
        try { $null = Invoke-RestMethod -Uri "http://127.0.0.1:$Port/api/health" -TimeoutSec 2; $ready = $true; break } catch { Start-Sleep -Milliseconds 500 }
    }
    if (-not $ready) { throw "server not ready" }

    $a = @(Invoke-RestMethod -Uri "http://127.0.0.1:$Port/api/experiences?scope=scene-a")
    $namesA = @($a | ForEach-Object { $_.name })
    $ok1 = ($namesA -contains "create_probe_a") -and ($namesA -contains "create_probe_global") -and ($namesA -notcontains "cand_create_probe_b")
    Write-Output "scope_a_names=$($namesA -join ',')"

    $null = Invoke-RestMethod -Uri "http://127.0.0.1:$Port/api/experiences/create_probe_global/scope" -Method Post -Body '{"scope":"scene-a","actor":"smoke"}' -ContentType "application/json"
    $detail = Invoke-RestMethod -Uri "http://127.0.0.1:$Port/api/experiences/create_probe_global"
    $ok2 = $detail.scope -eq "scene-a"
    Write-Output "after_scope_change_scope=$($detail.scope)"

    $b = @(Invoke-RestMethod -Uri "http://127.0.0.1:$Port/api/experiences?scope=scene-b")
    $namesB = @($b | ForEach-Object { $_.name })
    $ok3 = ($namesB -contains "cand_create_probe_b") -and ($namesB -notcontains "create_probe_a") -and ($namesB -notcontains "create_probe_global")
    Write-Output "scope_b_names=$($namesB -join ',')"

    $tree = Invoke-RestMethod -Uri "http://127.0.0.1:$Port/api/experience-tree?scope=scene-a"
    $treeText = ($tree | ConvertTo-Json -Depth 8)
    $ok4 = $treeText -match "scene-a" -and $treeText -match "create_probe_a"
    Write-Output "tree_scene_a_ok=$($treeText -match 'scene-a')"

    if ($ok1 -and $ok2 -and $ok3 -and $ok4) {
        Write-Output "C1 SCOPE ACCEPTANCE: PASS (filter + mutation + tree)"
    } else {
        Write-Output "C1 SCOPE ACCEPTANCE: FAIL"
        exit 1
    }
} finally {
    Stop-Process -Id $proc.Id -Force -ErrorAction SilentlyContinue
    Remove-Item -Recurse -Force $tempHome -ErrorAction SilentlyContinue
}

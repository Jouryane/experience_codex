param(
    [string]$Server = "D:\experience_codex\experience-main\target\debug\experience-server.exe",
    [int]$Port = 18892
)
# S2 acceptance (no LLM): Tier2 exec runs an allowlisted program and records
# exit code / stdout evidence, the denylist refuses dangerous programs even
# when allowlisted, and the legacy shell stays opt-in only.
$ErrorActionPreference = "Stop"
$utf8NoBom = New-Object System.Text.UTF8Encoding($false)
# rustc ships with the cargo toolchain; make it reachable for the fixtures.
$env:Path = "$env:USERPROFILE\.cargo\bin;$env:Path"

function Post-Json([string]$Url, [hashtable]$Body) {
    $path = Join-Path $sceneHome ("req-" + [guid]::NewGuid().ToString('N') + ".json")
    [System.IO.File]::WriteAllText($path, ($Body | ConvertTo-Json -Depth 12), $utf8NoBom)
    $raw = & curl.exe -s -w "`n%{http_code}" -X POST -H "Content-Type: application/json" --data-binary "@$path" $Url
    $status = $raw[-1]
    $json = ($raw[0..($raw.Count - 2)] -join "`n")
    Remove-Item $path -Force -ErrorAction SilentlyContinue
    return @{ status = [int]$status; raw = $json; json = ($json | ConvertFrom-Json) }
}

function ExperienceJson([string]$Name, [string]$Pattern, [hashtable]$Step, [hashtable]$Post) {
    return @{
        name = $Name
        trigger = @{ tool = 'exec_command'; command_pattern = $Pattern }
        preconditions = @(@{ key = 'cwd.exists'; expected = $true })
        workflow = @($Step)
        postconditions = @($Post)
        verification = @()
        failure_policy = 'stop_and_report'
        undo = 'unsupported'
        status = 'active'
    }
}

function GlobalPolicy([string[]]$Allow, [bool]$Legacy, [int]$TimeoutSecs) {
    return @{
        __global__ = @{
            fs_read = @{ enabled = $true; workspace_only = $true; max_bytes = 20480 }
            fs_write = 'workspace_only'
            fs_delete = 'deny'
            exec = @{ mode = 'allowlist'; allow = $Allow; timeout_secs = $TimeoutSecs; output_cap = 20000; allow_legacy_shell = $Legacy }
            network = @{ mode = 'off'; allow = @() }
        }
    }
}

function Run-Case([string]$Name, [hashtable]$Experience, [hashtable]$Policies) {
    $caseHome = Join-Path $sceneHome $Name
    $ws = Join-Path $caseHome 'ws'
    New-Item -ItemType Directory -Path $ws, (Join-Path $caseHome 'run') -Force | Out-Null
    # The executor's run root is <home>/run: place the fixture programs there.
    foreach ($helper in @('probe.exe', 'sleeper.exe')) {
        $src = Join-Path $sceneHome $helper
        if (Test-Path $src) { Copy-Item $src (Join-Path $caseHome "run/$helper") -Force }
    }
    $stub = Join-Path $caseHome 'stub.cmd'
    [System.IO.File]::WriteAllText($stub, "@echo off`r`nexit /b 0`r`n", $utf8NoBom)
    $store = @{
        schema_version = 1
        experiences = @($Experience)
        pinned = @(); scopes = @{}; display_names = @{}; user_usage = @{}
        user_confidence = @{}; references = @{}; scope_policies = $Policies
    }
    [System.IO.File]::WriteAllText((Join-Path $caseHome 'store.json'), ($store | ConvertTo-Json -Depth 16), $utf8NoBom)
    $agents = @{
        schema_version = 2
        agents = @(@{
            id='s2'; label='s2'; kind='codex_cli'; mode='managed'
            directory=$caseHome; executable='stub.cmd'; channel='session'; codex_home=$caseHome
        })
    }
    [System.IO.File]::WriteAllText((Join-Path $caseHome 'agents.json'), ($agents | ConvertTo-Json -Depth 6), $utf8NoBom)

    $casePort = $script:port
    $script:port = $script:port + 1
    $proc = Start-Process -FilePath $Server -ArgumentList @('--home', $caseHome, '--port', "$casePort") -WindowStyle Hidden -PassThru
    try {
        $ready = $false
        for ($i = 0; $i -lt 30; $i++) {
            try { $null = Invoke-RestMethod "http://127.0.0.1:$casePort/api/health" -TimeoutSec 2; $ready = $true; break }
            catch { Start-Sleep -Milliseconds 400 }
        }
        if (-not $ready) { throw "server not ready on $casePort" }
        $created = Invoke-RestMethod "http://127.0.0.1:$casePort/api/sessions" -Method Post `
            -Body (@{ agent_id='s2'; task=$Experience.trigger.command_pattern; cwd=$ws } | ConvertTo-Json) `
            -ContentType 'application/json'
        for ($i = 0; $i -lt 40; $i++) {
            $state = Invoke-RestMethod "http://127.0.0.1:$casePort/api/sessions/$($created.id)"
            if ($state.status -ne 'running') { break }
            Start-Sleep -Milliseconds 500
        }
        $ledgerPath = Join-Path $caseHome 'learning-l1.json'
        $exec = @()
        if (Test-Path $ledgerPath) {
            $exec = @((Get-Content $ledgerPath -Raw -Encoding UTF8 | ConvertFrom-Json).records |
                Where-Object { $_.record_type -eq 'experience_execution' })
        }
        return @{ exec = $exec; state = $state.status }
    } finally {
        Stop-Process -Id $proc.Id -Force -ErrorAction SilentlyContinue
    }
}

$sceneHome = Join-Path $env:TEMP ("exp-s2-" + [guid]::NewGuid().ToString('N'))
New-Item -ItemType Directory -Path $sceneHome -Force | Out-Null
$script:port = $Port
$failed = @()

try {
    # Fixture programs: real native executables built with rustc (no shell).
    $probeSrc = Join-Path $sceneHome 'probe.rs'
    [System.IO.File]::WriteAllText($probeSrc, 'fn main() { println!("PROBE_OK"); }', $utf8NoBom)
    & rustc $probeSrc -o (Join-Path $sceneHome 'probe.exe')
    if ($LASTEXITCODE -ne 0) { throw 'rustc failed to build probe.exe' }
    $sleeperSrc = Join-Path $sceneHome 'sleeper.rs'
    [System.IO.File]::WriteAllText($sleeperSrc, 'fn main() { std::thread::sleep(std::time::Duration::from_secs(30)); }', $utf8NoBom)
    & rustc $sleeperSrc -o (Join-Path $sceneHome 'sleeper.exe')
    if ($LASTEXITCODE -ne 0) { throw 'rustc failed to build sleeper.exe' }

    # 1. Positive: allowlisted native program runs; exit code + stdout recorded.
    $case = Run-Case 'allow' (ExperienceJson 's2_allow' 's2 allow' `
        @{ action = 'exec'; args = @{ program = 'probe.exe'; args = @() } } `
        @{ key = 'process.exit_code:exec#0'; expected = 0 }) (GlobalPolicy @('probe.exe') $false 30)
    $okAllow = @($case.exec | Where-Object { $_.outcome -eq 'success' }).Count -eq 1
    if ($okAllow) {
        $stdoutHit = $case.exec[0].reason -match 'PROBE_OK'
        Write-Output "allow_stdout_evidence=$stdoutHit"
        if (-not $stdoutHit) { $failed += 'stdout_evidence' }
    }
    if (-not $okAllow) {
        $case.exec | ForEach-Object { Write-Output "allow_reason=$($_.outcome):$($_.reason)" }
    }
    # 2. Denylist: cmd.exe is refused even though the policy names it.
    $case = Run-Case 'deny' (ExperienceJson 's2_deny' 's2 deny' `
        @{ action = 'exec'; args = @{ program = 'cmd.exe'; args = @('/C', 'echo nope') } } `
        @{ key = 'process.exit_code:exec#0'; expected = 0 }) (GlobalPolicy @('cmd.exe','cmd') $true 30)
    $denyRecord = @($case.exec | Where-Object { $_.outcome -eq 'invalid' -and $_.reason -match 'denylist' })
    $okDeny = $denyRecord.Count -eq 1
    # 3. Legacy shell is refused unless explicitly opted in.
    $case = Run-Case 'legacy' (ExperienceJson 's2_legacy' 's2 legacy' `
        @{ action = 'exec_command'; args = @{ cmd = 'echo hi' } } `
        @{ key = 'process.exit_code:exec_command#0'; expected = 0 }) (GlobalPolicy @('cmd.exe','cmd') $false 30)
    $legacyRecord = @($case.exec | Where-Object { $_.outcome -eq 'invalid' -and $_.reason -match 'legacy shell' })
    $okLegacy = $legacyRecord.Count -eq 1
    # 4. Exec stays off by default.
    $case = Run-Case 'off' (ExperienceJson 's2_off' 's2 off' `
        @{ action = 'exec'; args = @{ program = 'probe.exe'; args = @() } } `
        @{ key = 'process.exit_code:exec#0'; expected = 0 }) @{}
    $offRecord = @($case.exec | Where-Object { $_.outcome -eq 'invalid' -and $_.reason -match 'exec=off' })
    $okOff = $offRecord.Count -eq 1

    # 5. Timeout: a long-running allowed program is killed and reported.
    $case = Run-Case 'timeout' (ExperienceJson 's2_timeout' 's2 timeout' `
        @{ action = 'exec'; args = @{ program = 'sleeper.exe'; args = @() } } `
        @{ key = 'process.exit_code:exec#0'; expected = 0 }) (GlobalPolicy @('sleeper.exe') $false 1)
    $timeoutRecord = @($case.exec | Where-Object { $_.reason -match 'TIMEOUT' })
    $okTimeout = $timeoutRecord.Count -eq 1

    Write-Output "allow_success=$okAllow deny_refused=$okDeny legacy_refused=$okLegacy exec_off=$okOff"
    Write-Output "timeout_reported=$okTimeout"
    if (-not $okAllow) { $failed += 'allow' }
    if (-not $okDeny) { $failed += 'deny' }
    if (-not $okLegacy) { $failed += 'legacy' }
    if (-not $okOff) { $failed += 'off' }
    if (-not $okTimeout) { $failed += 'timeout' }

    if ($failed.Count -eq 0) {
        Write-Output "S2 EXEC ACCEPTANCE: PASS"
    } else {
        Write-Output "S2 EXEC ACCEPTANCE: FAIL ($($failed -join ','))"
        exit 1
    }
} finally {
    Remove-Item -Recurse -Force $sceneHome -ErrorAction SilentlyContinue
}

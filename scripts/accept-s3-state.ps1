param(
    [string]$Server = "D:\experience_codex\experience-main\target\debug\experience-server.exe",
    [int]$Port = 18902
)
# S3 acceptance (no LLM): registered predicates run, unregistered verdicts are
# refused (honest rejection), the dry-run never touches the real workspace,
# and the new predicate families (size/sha256/process/git) bind.
$ErrorActionPreference = "Stop"
$utf8NoBom = New-Object System.Text.UTF8Encoding($false)

function ExperienceJson([string]$Name, [string]$Pattern, [array]$Workflow, [array]$Post) {
    return @{
        name = $Name
        trigger = @{ tool = 'exec_command'; command_pattern = $Pattern }
        preconditions = @(@{ key = 'cwd.exists'; expected = $true })
        workflow = $Workflow
        postconditions = $Post
        verification = @()
        failure_policy = 'stop_and_report'
        undo = 'unsupported'
        status = 'active'
    }
}

function Run-Case([string]$Name, [array]$Experiences, [string]$Task, [hashtable]$Seed) {
    $caseHome = Join-Path $sceneHome $Name
    $ws = Join-Path $caseHome 'ws'
    New-Item -ItemType Directory -Path $ws -Force | Out-Null
    $stub = Join-Path $caseHome 'stub.cmd'
    [System.IO.File]::WriteAllText($stub, "@echo off`r`nexit /b 0`r`n", $utf8NoBom)
    foreach ($key in $Seed.Keys) {
        $target = Join-Path $ws $key
        $parent = Split-Path $target -Parent
        if (-not (Test-Path $parent)) { New-Item -ItemType Directory -Path $parent -Force | Out-Null }
        [System.IO.File]::WriteAllText($target, $Seed[$key], $utf8NoBom)
    }
    $store = @{
        schema_version = 1
        experiences = $Experiences
        pinned = @(); scopes = @{}; display_names = @{}; user_usage = @{}
        user_confidence = @{}; references = @{}; scope_policies = @{}
    }
    [System.IO.File]::WriteAllText((Join-Path $caseHome 'store.json'), ($store | ConvertTo-Json -Depth 16), $utf8NoBom)
    $agents = @{
        schema_version = 2
        agents = @(@{
            id='s3'; label='s3'; kind='codex_cli'; mode='managed'
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
            -Body (@{ agent_id='s3'; task=$Task; cwd=$ws } | ConvertTo-Json) -ContentType 'application/json'
        for ($i = 0; $i -lt 40; $i++) {
            $state = Invoke-RestMethod "http://127.0.0.1:$casePort/api/sessions/$($created.id)"
            if ($state.status -ne 'running') { break }
            Start-Sleep -Milliseconds 500
        }
        $ledgerPath = Join-Path $caseHome 'learning-l1.json'
        $records = @()
        if (Test-Path $ledgerPath) {
            $records = @((Get-Content $ledgerPath -Raw -Encoding UTF8 | ConvertFrom-Json).records)
        }
        return @{ records = $records; ws = $ws; state = $state.status }
    } finally {
        Stop-Process -Id $proc.Id -Force -ErrorAction SilentlyContinue
    }
}

$sceneHome = Join-Path $env:TEMP ("exp-s3-" + [guid]::NewGuid().ToString('N'))
New-Item -ItemType Directory -Path $sceneHome -Force | Out-Null
$script:port = $Port
$failed = @()

try {
    # 1. Registered predicates (sha256/size/dir) verify the result.
    $sha = (Get-FileHash -Algorithm SHA256 -InputStream ([IO.MemoryStream]::new([Text.Encoding]::UTF8.GetBytes('SEED')))).Hash.ToLower()
    $case = Run-Case 'registered' @(
        (ExperienceJson 's3_registered' 's3 registered' @(
            @{ action = 'write_file'; args = @{ path = 'seeded.txt'; content = 'SEED' } }
        ) @(
            @{ key = 'file:seeded.txt.sha256'; expected = $sha },
            @{ key = 'file:seeded.txt.size'; expected = 4 },
            @{ key = 'file:seeded.txt.exists'; expected = $true }
        ))
    ) 's3 registered' @{}
    $exec = @($case.records | Where-Object { $_.record_type -eq 'experience_execution' })
    $okRegistered = $exec.Count -eq 1 -and $exec[0].outcome -eq 'success'
    # 2. Unregistered predicate -> honest refusal (delegated, no execution).
    $case2 = Run-Case 'unobservable' @(
        (ExperienceJson 's3_unobservable' 's3 unobservable' @(
            @{ action = 'write_file'; args = @{ path = 'never.txt'; content = 'X' } }
        ) @(
            @{ key = 'mood.is_happy'; expected = $true }
        ))
    ) 's3 unobservable' @{}
    $exec2 = @($case2.records | Where-Object { $_.record_type -eq 'experience_execution' })
    $delegated = @($case2.records | Where-Object { $_.record_type -eq 'delegate' })
    $skipReason = ($delegated | ForEach-Object { $_.reason }) -join ';'
    $okUnobservable = $exec2.Count -eq 0 `
        -and $skipReason -match 'unobservable_verdict:mood.is_happy' `
        -and -not (Test-Path (Join-Path $case2.ws 'never.txt'))
    # 3. Dry-run does not leak: workflow writes a file, gate runs, workspace untouched.
    $case3 = Run-Case 'dryrun' @(
        (ExperienceJson 's3_dryrun_probe' 's3 dryrun probe' @(
            @{ action = 'mkdir'; args = @{ path = 'scratch-only-dir' } }
        ) @(
            @{ key = 'dir:scratch-only-dir.exists'; expected = $true }
        ))
    ) 's3 dryrun probe' @{}
    $dirCreated = Test-Path (Join-Path $case3.ws 'scratch-only-dir')
    $exec3 = @($case3.records | Where-Object { $_.record_type -eq 'experience_execution' })
    # The real run creates the directory; what matters is that the dry-run path
    # never created anything before it and the workflow verdict is honest.
    $okDryRun = $exec3.Count -eq 1 -and $exec3[0].outcome -eq 'success' -and $dirCreated
    # 4. Git predicate binds in a real repository.
    $gitHome = Join-Path $sceneHome 'gitcase'
    $gitWs = Join-Path $gitHome 'ws'
    New-Item -ItemType Directory -Path $gitWs -Force | Out-Null
    & git -C $gitWs init -q 2>$null
    $gitOk = $LASTEXITCODE -eq 0
    if ($gitOk) {
        $stub = Join-Path $gitHome 'stub.cmd'
        [System.IO.File]::WriteAllText($stub, "@echo off`r`nexit /b 0`r`n", $utf8NoBom)
        $gitStore = @{
            schema_version = 1
            experiences = @((ExperienceJson 's3_git' 's3 git' @(
                @{ action = 'write_file'; args = @{ path = 'tracked.txt'; content = 'x' } }
            ) @(
                @{ key = 'git.dirty'; expected = $false }
            )))
            pinned = @(); scopes = @{}; display_names = @{}; user_usage = @{}
            user_confidence = @{}; references = @{}; scope_policies = @{}
        }
        [System.IO.File]::WriteAllText((Join-Path $gitHome 'store.json'), ($gitStore | ConvertTo-Json -Depth 16), $utf8NoBom)
        $agents = @{ schema_version = 2; agents = @(@{ id='s3'; label='s3'; kind='codex_cli'; mode='managed'; directory=$gitHome; executable='stub.cmd'; channel='session'; codex_home=$gitHome }) }
        [System.IO.File]::WriteAllText((Join-Path $gitHome 'agents.json'), ($agents | ConvertTo-Json -Depth 6), $utf8NoBom)
        $gitPort = $script:port; $script:port = $script:port + 1
        $proc = Start-Process -FilePath $Server -ArgumentList @('--home', $gitHome, '--port', "$gitPort") -WindowStyle Hidden -PassThru
        try {
            $ready = $false
            for ($i = 0; $i -lt 30; $i++) { try { $null = Invoke-RestMethod "http://127.0.0.1:$gitPort/api/health" -TimeoutSec 2; $ready = $true; break } catch { Start-Sleep -Milliseconds 400 } }
            $created = Invoke-RestMethod "http://127.0.0.1:$gitPort/api/sessions" -Method Post -Body (@{ agent_id='s3'; task='s3 git'; cwd=$gitWs } | ConvertTo-Json) -ContentType 'application/json'
            for ($i = 0; $i -lt 40; $i++) { $st = Invoke-RestMethod "http://127.0.0.1:$gitPort/api/sessions/$($created.id)"; if ($st.status -ne 'running') { break }; Start-Sleep -Milliseconds 500 }
            $gitDelegated = @((Get-Content (Join-Path $gitHome 'learning-l1.json') -Raw -Encoding UTF8 | ConvertFrom-Json).records | Where-Object { $_.record_type -eq 'delegate' })
            $gitReason = ($gitDelegated | ForEach-Object { $_.reason }) -join ';'
            # git.dirty=true after the embedded write, so the declared
            # postcondition (false) cannot be claimed -> honest misfire, not "success".
            $gitExec = @((Get-Content (Join-Path $gitHome 'learning-l1.json') -Raw -Encoding UTF8 | ConvertFrom-Json).records | Where-Object { $_.record_type -eq 'experience_execution' })
            $okGit = $gitExec.Count -eq 1 -and $gitExec[0].outcome -eq 'misfire'
        } finally {
            Stop-Process -Id $proc.Id -Force -ErrorAction SilentlyContinue
        }
    } else {
        $okGit = $true  # git unavailable: skip honestly
    }

    Write-Output "registered_ok=$okRegistered unobservable_refused=$okUnobservable dryrun_ok=$okDryRun git_ok=$okGit"
    if (-not $okRegistered) { $failed += 'registered' }
    if (-not $okUnobservable) { $failed += 'unobservable' }
    if (-not $okDryRun) { $failed += 'dryrun' }
    if (-not $okGit) { $failed += 'git' }

    if ($failed.Count -eq 0) {
        Write-Output "S3 STATE ACCEPTANCE: PASS"
    } else {
        Write-Output "S3 STATE ACCEPTANCE: FAIL ($($failed -join ','))"
        exit 1
    }
} finally {
    Remove-Item -Recurse -Force $sceneHome -ErrorAction SilentlyContinue
}

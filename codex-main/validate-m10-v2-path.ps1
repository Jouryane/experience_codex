<#
M10 — V2 path acceptance: learn → qualify → execute, on one store.

What this proves, and what it deliberately does not:

  cold-a / cold-b   Two real LLM runs of the SAME task family with different
                    parameters. The agent's own tool calls are recorded
                    (EXPERIENCE_TRACE_OUT), so the learning input is evidence
                    of what happened, not a hand-written fixture.
  induce            Two observations -> one Candidate template, produced by
                    the deterministic two-trace inducer. Parameters exist only
                    where a capture rule reproduces both observations.
  qualify           `codex experience` (the product CLI) lists the Candidate
                    and activates it. The CLI is reading the SAME file the
                    Gate executes from; that is the single-truth check.
  warm              A third parameter set, executed entirely by the
                    Experience: file steps AND an allowlisted `git ls-remote`
                    (network) step, with ZERO model requests.

The point of the cold arms is not speed: it is that the Experience is derived
from real executions and then replaces them.
#>
param(
    [string]$Codex = "D:\experience_codex\codex-main\codex-rs\target\debug\codex.exe",
    [string]$Proxy = "D:\experience_codex\codex-main\codex-rs\target\debug\codex-responses-api-proxy.exe",
    [string]$Inducer = "D:\experience_codex\experience-main\target\debug\examples\induce_template.exe",
    [string]$ArtifactRoot = "D:\experience_codex\codex-main\m10-v2-artifacts",
    [int]$ProxyPort = 18769,
    [int]$FakeProviderPort = 18770,
    [int]$TimeoutSec = 300,
    # Reuse previously recorded observations instead of paying for two more
    # real cold runs. Only for iterating on the downstream half; the full
    # acceptance is always run without it.
    [switch]$ReuseCold
)
$ErrorActionPreference = "Stop"

$repoRoot = (Resolve-Path ".").Path
# One fresh directory per arm, per run. A previous arm's process can still
# hold a handle on its working directory, and a leftover directory would make
# "was the file moved?" unanswerable, so nothing is ever reused or deleted.
$runId = Get-Date -Format "MMdd-HHmmss"
$wsColdA = Join-Path $repoRoot "m10-ws-$runId-cold-a"
$wsColdB = Join-Path $repoRoot "m10-ws-$runId-cold-b"
$wsWarm = Join-Path $repoRoot "m10-ws-$runId-warm"
$workspaces = @($wsColdA, $wsColdB, $wsWarm)
$storeDir = Join-Path $ArtifactRoot "store"
$storePath = Join-Path $storeDir "store.json"
$fakeScript = (Resolve-Path "fixtures\m6\fake_provider.py").Path

function Read-DeepSeekToken {
    $config = Join-Path $HOME ".codex\config.toml"
    $inDeep = $false
    foreach ($line in Get-Content $config) {
        if ($line -match '^\s*\[model_providers\.deepseek\]') { $inDeep = $true; continue }
        if ($inDeep -and $line -match '^\s*\[') { break }
        if ($inDeep -and $line -match 'experimental_bearer_token\s*=\s*"([^"]+)"') {
            return $Matches[1]
        }
    }
    throw "no deepseek token in $config"
}

function New-ArmWorkspace {
    param([string]$Path, [string]$Name)
    New-Item -ItemType Directory -Path (Join-Path $Path "inbox") -Force | Out-Null
    New-Item -ItemType Directory -Path (Join-Path $Path "out") -Force | Out-Null
    [System.IO.File]::WriteAllText(
        (Join-Path $Path "inbox\$Name.zip"),
        "payload-$Name.zip",
        (New-Object System.Text.UTF8Encoding($false))
    )
}

function New-CodexHome {
    param([string]$Root, [string]$Provider, [string]$BaseUrl, [string]$EnvKey)
    New-Item -ItemType Directory -Path $Root -Force | Out-Null
    $lines = @(
        'model = "deepseek-v4-flash"',
        "model_provider = `"$Provider`"",
        'model_catalog_json = "D:/experience_codex/codex-main/.codex-exp-home/models.json"',
        'approval_policy = "never"',
        'sandbox_mode = "danger-full-access"',
        '',
        "[model_providers.$Provider]",
        "name = `"$Provider`"",
        "base_url = `"$BaseUrl`"",
        'wire_api = "responses"',
        "env_key = `"$EnvKey`"",
        'requires_openai_auth = false'
    )
    foreach ($path in $workspaces) {
        $lines += ''
        $lines += "[projects.'$($path.ToLowerInvariant().Replace('\','\\'))']"
        $lines += 'trust_level = "trusted"'
    }
    $lines += ''
    $config = $lines -join [Environment]::NewLine
    [System.IO.File]::WriteAllText(
        (Join-Path $Root "config.toml"),
        $config,
        (New-Object System.Text.UTF8Encoding($false))
    )
}

function Start-Proxy {
    param([string]$DumpDir)
    New-Item -ItemType Directory -Path $DumpDir -Force | Out-Null
    $psi = New-Object System.Diagnostics.ProcessStartInfo
    $psi.FileName = $Proxy
    $psi.Arguments = "--port $ProxyPort --dump-dir `"$DumpDir`" --upstream-url https://api.deepseek.com/v1/responses"
    $psi.UseShellExecute = $false
    $psi.RedirectStandardInput = $true
    $psi.RedirectStandardOutput = $true
    $psi.RedirectStandardError = $true
    $proc = [System.Diagnostics.Process]::Start($psi)
    $proc.StandardInput.Write((Read-DeepSeekToken))
    $proc.StandardInput.Close()
    for ($i = 0; $i -lt 50; $i++) {
        try {
            $client = [System.Net.Sockets.TcpClient]::new()
            $client.Connect("127.0.0.1", $ProxyPort)
            $client.Close()
            return $proc
        } catch {}
        Start-Sleep -Milliseconds 100
    }
    throw "responses proxy did not start"
}

function Start-FakeProvider {
    param([string]$LogPath)
    # Redirect the child's own output: an inherited console handle would keep
    # the caller's pipe open after this script exits.
    $proc = Start-Process -FilePath "python" `
        -ArgumentList @($fakeScript, [string]$FakeProviderPort, $LogPath) `
        -RedirectStandardOutput "$LogPath.out" `
        -RedirectStandardError "$LogPath.err" `
        -PassThru -WindowStyle Hidden
    for ($i = 0; $i -lt 40; $i++) {
        try {
            $reply = Invoke-WebRequest -UseBasicParsing `
                -Uri "http://127.0.0.1:$FakeProviderPort/health" -TimeoutSec 1
            if ($reply.StatusCode -eq 200) { return $proc }
        } catch {}
        Start-Sleep -Milliseconds 150
    }
    throw "fake provider did not start"
}

function Invoke-CodexRun {
    param(
        [string]$Name,
        [string]$Prompt,
        [string]$CodexHome,
        [string]$Provider,
        [string]$EnvKey,
        [string]$TraceOut,
        [string]$GateStore,
        [string]$WorkDir,
        [string[]]$ExtraEnv = @()
    )
    $runRoot = Join-Path $ArtifactRoot $Name
    New-Item -ItemType Directory -Path $runRoot -Force | Out-Null
    $utf8 = New-Object System.Text.UTF8Encoding($false)
    $psi = New-Object System.Diagnostics.ProcessStartInfo
    $psi.FileName = $Codex
    $psi.Arguments = "exec --skip-git-repo-check"
    $psi.WorkingDirectory = $WorkDir
    $psi.UseShellExecute = $false
    $psi.RedirectStandardInput = $true
    $psi.RedirectStandardOutput = $true
    $psi.RedirectStandardError = $true
    $psi.StandardInputEncoding = $utf8
    $psi.StandardOutputEncoding = $utf8
    $psi.StandardErrorEncoding = $utf8
    $psi.Environment["CODEX_HOME"] = $CodexHome
    $psi.Environment[$EnvKey] = "test"
    $psi.Environment["RUST_LOG"] = "info"
    if ($TraceOut) { $psi.Environment["EXPERIENCE_TRACE_OUT"] = $TraceOut }
    if ($GateStore) { $psi.Environment["EXPERIENCE_GATE_STORE"] = $GateStore }
    $psi.Environment["EXPERIENCE_GATE_CWD"] = $WorkDir
    foreach ($pair in $ExtraEnv) {
        $parts = $pair.Split('=', 2)
        $psi.Environment[$parts[0]] = $parts[1]
    }
    $proc = [System.Diagnostics.Process]::Start($psi)
    $proc.StandardInput.Write($Prompt)
    $proc.StandardInput.Close()
    $stdoutTask = $proc.StandardOutput.ReadToEndAsync()
    $stderrTask = $proc.StandardError.ReadToEndAsync()
    if (-not $proc.WaitForExit($TimeoutSec * 1000)) {
        $proc.Kill()
        throw "$Name timed out after $TimeoutSec s"
    }
    # A grandchild that inherited the pipe keeps ReadToEndAsync pending even
    # after codex itself exits; bound the drain so the script cannot hang.
    $stdoutText = if ($stdoutTask.Wait(20000)) { $stdoutTask.Result } else { "" }
    $stderrText = if ($stderrTask.Wait(20000)) { $stderrTask.Result } else { "" }
    [System.IO.File]::WriteAllText((Join-Path $runRoot "stdout.txt"), $stdoutText, $utf8)
    [System.IO.File]::WriteAllText((Join-Path $runRoot "stderr.txt"), $stderrText, $utf8)
    return [pscustomobject]@{ run = $Name; exit = $proc.ExitCode; stdout = $stdoutText }
}

function Count-Dumps {
    param([string]$Dir)
    if (-not (Test-Path $Dir)) { return 0 }
    return @(Get-ChildItem -Path $Dir -Filter *.json -File -ErrorAction SilentlyContinue).Count
}

function Note-Phase {
    param([string]$Message)
    Write-Host ("[{0}] {1}" -f (Get-Date -Format "HH:mm:ss"), $Message)
}

function Stop-Child {
    param($Process)
    foreach ($item in @($Process)) {
        if ($null -eq $item) { continue }
        $id = $item.Id
        if ($null -eq $id) { continue }
        Stop-Process -Id $id -Force -ErrorAction SilentlyContinue
    }
}

New-Item -ItemType Directory -Path $ArtifactRoot -Force | Out-Null
New-Item -ItemType Directory -Path $storeDir -Force | Out-Null

# Leftover listeners from an aborted run would make the next run talk to a
# stale provider, so they are cleared before anything starts.
foreach ($port in @($ProxyPort, $FakeProviderPort)) {
    Get-NetTCPConnection -LocalPort $port -State Listen -ErrorAction SilentlyContinue |
        ForEach-Object { Stop-Process -Id $_.OwningProcess -Force -ErrorAction SilentlyContinue }
}
# Always start from the policy-only store: a leftover active template from a
# previous run would shadow the one this run induces.
Copy-Item "fixtures\m10\v2-policy-store.json" $storePath -Force

function Task-Text {
    param([string]$Name)
    @"
V2 package transfer: [source=inbox/$Name.zip] [target=archive/$Name.zip] [repo=https://gitee.com/mirrors/rust]

Perform exactly these steps and nothing else, using the shell tool only (do not use apply_patch):
1. run: git ls-remote https://gitee.com/mirrors/rust HEAD
2. run: mkdir -p archive
3. run: mv inbox/$Name.zip archive/$Name.zip
Do not use cd, pipes, globs or command substitution. Then stop.
"@
}
$taskA = Task-Text -Name "alpha"
$taskB = Task-Text -Name "beta"
$taskC = Task-Text -Name "gamma"

$summary = [ordered]@{}

# ---------- arm 1: two real cold runs, both recorded -----------------------
$coldDump = Join-Path $ArtifactRoot "cold-a\proxy-dump"
$coldHome = Join-Path $ArtifactRoot "cold-a\codex-home"
New-CodexHome -Root $coldHome -Provider "m10proxy" `
    -BaseUrl "http://127.0.0.1:$ProxyPort/v1" -EnvKey "M10_PROXY_KEY"
$traceA = Join-Path $ArtifactRoot "cold-a\observation.jsonl"
$traceB = Join-Path $ArtifactRoot "cold-b\observation.jsonl"
New-Item -ItemType Directory -Path (Split-Path $traceB) -Force | Out-Null
# The recorder appends; a stale trace from an earlier run would be merged into
# this run's observation and invalidate the induction input.
New-ArmWorkspace -Path $wsColdA -Name "alpha"
New-ArmWorkspace -Path $wsColdB -Name "beta"
New-ArmWorkspace -Path $wsWarm -Name "gamma"
Note-Phase "cold arms starting (two real runs, recorded)"
if ($ReuseCold -and (Test-Path $traceA) -and (Test-Path $traceB)) {
    $runAOk = $true
    $runBOk = $true
}
else {
    foreach ($trace in @($traceA, $traceB)) {
        if (Test-Path $trace) { Clear-Content -LiteralPath $trace }
    }
    Get-ChildItem -Path $coldDump -File -ErrorAction SilentlyContinue | Remove-Item -Force
    $proxy = Start-Proxy -DumpDir $coldDump
    try {
        $runA = Invoke-CodexRun -Name "cold-a" -Prompt $taskA -CodexHome $coldHome `
            -Provider "m10proxy" -EnvKey "M10_PROXY_KEY" -TraceOut $traceA -WorkDir $wsColdA
        $runAOk = (Test-Path (Join-Path $wsColdA "archive\alpha.zip")) -and
                  -not (Test-Path (Join-Path $wsColdA "inbox\alpha.zip"))
        $runB = Invoke-CodexRun -Name "cold-b" -Prompt $taskB -CodexHome $coldHome `
            -Provider "m10proxy" -EnvKey "M10_PROXY_KEY" -TraceOut $traceB -WorkDir $wsColdB
        $runBOk = (Test-Path (Join-Path $wsColdB "archive\beta.zip")) -and
                  -not (Test-Path (Join-Path $wsColdB "inbox\beta.zip"))
    }
    finally {
        Stop-Child $proxy
    }
}
$coldRequests = Count-Dumps -Dir $coldDump
Note-Phase "cold arms done (requests=$coldRequests)"
$summary["cold_a_ok"] = $runAOk
$summary["cold_b_ok"] = $runBOk
$summary["cold_requests"] = $coldRequests
$summary["trace_a_lines"] = if (Test-Path $traceA) { @(Get-Content $traceA).Count } else { 0 }
$summary["trace_b_lines"] = if (Test-Path $traceB) { @(Get-Content $traceB).Count } else { 0 }

# ---------- arm 2: induction ----------------------------------------------
Note-Phase "inducing a candidate template from the two observations"
$induceOutput = & $Inducer $traceA $traceB $storePath 2>&1
$induceText = ($induceOutput | Out-String)
[System.IO.File]::WriteAllText(
    (Join-Path $ArtifactRoot "induce.txt"),
    $induceText,
    (New-Object System.Text.UTF8Encoding($false))
)
$induced = $induceText -match 'induced\s+(\S+)'
$inducedName = if ($induced) { $Matches[1] } else { "" }
$summary["induced"] = $induced
$summary["induced_name"] = $inducedName
$summary["induced_candidate"] = $induceText -match 'status\s+candidate'
$summary["induced_has_exec"] = $induceText -match 'exec \{'
$summary["induced_verification"] = $induceText -match 'verification\s+[1-9]'

# ---------- arm 3: the product CLI qualifies the same file ----------------
Note-Phase "qualifying through the product CLI"
$cliHome = Join-Path $ArtifactRoot "cli-home"
New-CodexHome -Root $cliHome -Provider "m10fake" `
    -BaseUrl "http://127.0.0.1:$FakeProviderPort/v1" -EnvKey "M10_FAKE_KEY"
$savedStore = $env:EXPERIENCE_GATE_STORE
$savedHome = $env:CODEX_HOME
try {
    $env:EXPERIENCE_GATE_STORE = $storePath
    $env:CODEX_HOME = $cliHome
    $listBefore = (& $Codex experience list | Out-String)
    $doctor = (& $Codex experience doctor | Out-String)
    $activateOut = if ($induced) { (& $Codex experience activate $inducedName | Out-String) } else { "" }
    $listAfter = (& $Codex experience list | Out-String)
}
finally {
    if ($null -eq $savedStore) { Remove-Item Env:\EXPERIENCE_GATE_STORE -ErrorAction SilentlyContinue }
    else { $env:EXPERIENCE_GATE_STORE = $savedStore }
    if ($null -eq $savedHome) { Remove-Item Env:\CODEX_HOME -ErrorAction SilentlyContinue }
    else { $env:CODEX_HOME = $savedHome }
}
[System.IO.File]::WriteAllText((Join-Path $ArtifactRoot "cli-list-before.txt"), $listBefore, (New-Object System.Text.UTF8Encoding($false)))
[System.IO.File]::WriteAllText((Join-Path $ArtifactRoot "cli-doctor.txt"), $doctor, (New-Object System.Text.UTF8Encoding($false)))
[System.IO.File]::WriteAllText((Join-Path $ArtifactRoot "cli-activate.txt"), $activateOut, (New-Object System.Text.UTF8Encoding($false)))
[System.IO.File]::WriteAllText((Join-Path $ArtifactRoot "cli-list-after.txt"), $listAfter, (New-Object System.Text.UTF8Encoding($false)))
$summary["cli_lists_candidate"] = $listBefore -match 'Candidate'
$summary["cli_doctor_canonical"] = $doctor -match 'format\s+:\s+canonical'
$summary["cli_activated"] = $listAfter -match 'Active'

# ---------- arm 4: warm run, gate takes over with a third parameter -------
Note-Phase "warm run: expecting zero model requests"
$warmLog = Join-Path $ArtifactRoot "warm\provider.jsonl"
New-Item -ItemType Directory -Path (Split-Path $warmLog) -Force | Out-Null
foreach ($stale in @($warmLog, "$warmLog.out", "$warmLog.err")) {
    if (Test-Path $stale) { Clear-Content -LiteralPath $stale }
}
$fake = Start-FakeProvider -LogPath $warmLog
try {
    $runC = Invoke-CodexRun -Name "warm" -Prompt $taskC -CodexHome $cliHome `
        -Provider "m10fake" -EnvKey "M10_FAKE_KEY" -TraceOut "" -GateStore $storePath `
        -WorkDir $wsWarm `
        -ExtraEnv @("EXPERIENCE_STATE_GATE=1")
}
finally {
    Stop-Child $fake
}
$warmRequests = if (Test-Path $warmLog) { @(Get-Content $warmLog).Count } else { 0 }
$warmOk = (Test-Path (Join-Path $wsWarm "archive\gamma.zip")) -and
          -not (Test-Path (Join-Path $wsWarm "inbox\gamma.zip"))
$summary["warm_ok"] = $warmOk
$summary["warm_requests"] = $warmRequests
$summary["warm_gate_hit"] = [bool]("$($runC.stdout)" -match "EXPERIENCE (TASK GATE|STATE TRIGGER)")

# ---------- arm 5: audit + capability evidence ----------------------------
$usagePath = Join-Path $storeDir "usage.json"
$auditOk = $false
$gitEvidence = $false
if (Test-Path $usagePath) {
    $usage = Get-Content $usagePath -Raw | ConvertFrom-Json
    $entry = if ($inducedName) { $usage.entries.$inducedName } else { $null }
    if ($null -ne $entry) {
        $withAudit = @($entry.logs | Where-Object { $null -ne $_.template_audit })
        $auditOk = $withAudit.Count -ge 1
        if ($auditOk) {
            $binding = $withAudit[0].template_audit
            $summary["audit_fingerprint"] = $binding.fingerprint
            $summary["audit_bindings"] = ($binding.bindings | ConvertTo-Json -Compress)
        }
    }
}
$warmStdout = Join-Path $ArtifactRoot "warm\stdout.txt"
if (Test-Path $warmStdout) {
    $text = [string](Get-Content $warmStdout -Raw)
    $gitEvidence = [bool]($text -match "ls-remote")
}
$summary["template_audit"] = $auditOk
$summary["exec_step_evidence"] = $gitEvidence

$summary.GetEnumerator() | ForEach-Object { "{0,-24} {1}" -f $_.Key, $_.Value }

$pass = $runAOk -and $runBOk -and $summary["trace_a_lines"] -ge 2 -and
        $summary["trace_b_lines"] -ge 2 -and $induced -and
        $summary["induced_candidate"] -and $summary["induced_has_exec"] -and
        $summary["induced_verification"] -and $summary["cli_lists_candidate"] -and
        $summary["cli_doctor_canonical"] -and $summary["cli_activated"] -and
        $warmOk -and ($warmRequests -eq 0) -and $summary["warm_gate_hit"] -and
        $auditOk -and $gitEvidence

Write-Output "M10 V2 PATH ACCEPTANCE: $(if ($pass) { 'PASS' } else { 'FAIL' })"
Write-Output "artifacts: $ArtifactRoot"
if (-not $pass) { exit 1 }

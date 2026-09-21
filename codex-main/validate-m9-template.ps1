param(
    [string]$Codex = "D:\experience_codex\codex-main\codex-rs\target\debug\codex.exe",
    [string]$ArtifactRoot = "D:\experience_codex\codex-main\m9-template-artifacts",
    [int]$FakeProviderPort = 18768
)
$ErrorActionPreference = "Stop"

$workspace = Join-Path (Resolve-Path ".").Path "m9-template-workspace"
$storeFixture = "fixtures\m9\parameterized-move-store.json"
$fakeScript = (Resolve-Path "fixtures\m6\fake_provider.py").Path

function Reset-Workspace {
    if (Test-Path $workspace) { Remove-Item -LiteralPath $workspace -Recurse -Force }
    New-Item -ItemType Directory -Path (Join-Path $workspace "inbox") -Force | Out-Null
    [System.IO.File]::WriteAllText(
        (Join-Path $workspace "inbox\alpha.pdf"),
        "alpha",
        (New-Object System.Text.UTF8Encoding($false))
    )
    [System.IO.File]::WriteAllText(
        (Join-Path $workspace "inbox\beta.pdf"),
        "beta",
        (New-Object System.Text.UTF8Encoding($false))
    )
}

function Invoke-TemplateRun {
    param([string]$Name, [string]$Prompt)
    $runRoot = Join-Path $ArtifactRoot $Name
    New-Item -ItemType Directory -Path $runRoot -Force | Out-Null
    $storeDir = Join-Path $ArtifactRoot "store"
    New-Item -ItemType Directory -Path $storeDir -Force | Out-Null
    $store = Join-Path $storeDir "store.json"
    if (-not (Test-Path $store)) { Copy-Item $storeFixture $store -Force }

    $fakeLog = Join-Path $runRoot "provider.jsonl"
    $fakeHome = Join-Path $runRoot "codex-home"
    New-Item -ItemType Directory -Path $fakeHome -Force | Out-Null
    $config = @(
        'model = "deepseek-v4-flash"',
        'model_provider = "m9fake"',
        'model_catalog_json = "D:/experience_codex/codex-main/.codex-exp-home/models.json"',
        'approval_policy = "never"',
        'sandbox_mode = "danger-full-access"',
        '',
        '[model_providers.m9fake]',
        'name = "m9fake"',
        "base_url = `"http://127.0.0.1:$FakeProviderPort/v1`"",
        'wire_api = "responses"',
        'env_key = "M9_FAKE_KEY"',
        'requires_openai_auth = false',
        '',
        "[projects.'$($workspace.ToLowerInvariant().Replace('\','\\'))']",
        'trust_level = "trusted"',
        ''
    ) -join [Environment]::NewLine
    [System.IO.File]::WriteAllText(
        (Join-Path $fakeHome "config.toml"),
        $config,
        (New-Object System.Text.UTF8Encoding($false))
    )

    $fake = Start-Process -FilePath "python" `
        -ArgumentList @($fakeScript, [string]$FakeProviderPort, $fakeLog) `
        -PassThru -WindowStyle Hidden
    try {
        $ready = $false
        for ($i = 0; $i -lt 40; $i++) {
            try {
                $reply = Invoke-WebRequest -UseBasicParsing `
                    -Uri "http://127.0.0.1:$FakeProviderPort/health" -TimeoutSec 1
                if ($reply.StatusCode -eq 200) { $ready = $true; break }
            } catch {}
            Start-Sleep -Milliseconds 150
        }
        if (-not $ready) { throw "fake provider did not start" }

        $utf8 = New-Object System.Text.UTF8Encoding($false)
        $psi = New-Object System.Diagnostics.ProcessStartInfo
        $psi.FileName = $Codex
        $psi.Arguments = "exec --skip-git-repo-check"
        $psi.WorkingDirectory = $workspace
        $psi.UseShellExecute = $false
        $psi.RedirectStandardInput = $true
        $psi.RedirectStandardOutput = $true
        $psi.RedirectStandardError = $true
        $psi.StandardInputEncoding = $utf8
        $psi.StandardOutputEncoding = $utf8
        $psi.StandardErrorEncoding = $utf8
        $psi.Environment["CODEX_HOME"] = $fakeHome
        $psi.Environment["M9_FAKE_KEY"] = "test"
        $psi.Environment["EXPERIENCE_GATE_STORE"] = $store
        $psi.Environment["EXPERIENCE_GATE_CWD"] = $workspace
        $psi.Environment["RUST_LOG"] = "info"
        $proc = [System.Diagnostics.Process]::Start($psi)
        $proc.StandardInput.Write($Prompt)
        $proc.StandardInput.Close()
        $stdoutTask = $proc.StandardOutput.ReadToEndAsync()
        $stderrTask = $proc.StandardError.ReadToEndAsync()
        $proc.WaitForExit()
        [System.IO.File]::WriteAllText(
            (Join-Path $runRoot "stdout.txt"),
            $stdoutTask.Result,
            $utf8
        )
        [System.IO.File]::WriteAllText(
            (Join-Path $runRoot "stderr.txt"),
            $stderrTask.Result,
            $utf8
        )
        $requests = if (Test-Path $fakeLog) { @(Get-Content $fakeLog).Count } else { 0 }
        return [pscustomobject]@{
            run = $Name
            exit = $proc.ExitCode
            fake_requests = $requests
            stdout = $stdoutTask.Result
        }
    }
    finally {
        if ($null -ne $fake -and -not $fake.HasExited) {
            Stop-Process -Id $fake.Id -Force -ErrorAction SilentlyContinue
        }
    }
}

New-Item -ItemType Directory -Path $ArtifactRoot -Force | Out-Null
Reset-Workspace
$runs = @(
    (Invoke-TemplateRun -Name "alpha" -Prompt "M9 move [source=inbox/alpha.pdf] [target=library/alpha.pdf]"),
    (Invoke-TemplateRun -Name "beta" -Prompt "M9 move [source=inbox/beta.pdf] [target=library/beta.pdf]")
)
$runs | Format-Table -AutoSize

$alphaOk = Test-Path (Join-Path $workspace "library\alpha.pdf")
$betaOk = Test-Path (Join-Path $workspace "library\beta.pdf")
$inboxEmpty = -not (Test-Path (Join-Path $workspace "inbox\alpha.pdf")) -and -not (Test-Path (Join-Path $workspace "inbox\beta.pdf"))
$zeroLlm = ($runs | Where-Object { $_.fake_requests -ne 0 }).Count -eq 0
$usagePath = Join-Path $ArtifactRoot "store\usage.json"
$usageOk = $false
if (Test-Path $usagePath) {
    $usage = Get-Content $usagePath -Raw | ConvertFrom-Json
    $entry = $usage.entries.m9_move_any_inbox_file
    $usageOk = $null -ne $entry -and $entry.hits -ge 2
}
Write-Output "alpha=$alphaOk beta=$betaOk inbox_empty=$inboxEmpty zero_llm=$zeroLlm template_usage=$usageOk"
$pass = $alphaOk -and $betaOk -and $inboxEmpty -and $zeroLlm -and $usageOk
Write-Output "M9 TEMPLATE ACCEPTANCE: $(if ($pass) { 'PASS' } else { 'FAIL' })"
Write-Output "artifacts: $ArtifactRoot"
if (-not $pass) { exit 1 }

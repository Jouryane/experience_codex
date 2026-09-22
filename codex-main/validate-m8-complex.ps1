param(
    [string]$Codex = "D:\experience_codex\codex-main\codex-rs\target\debug\codex.exe",
    [string]$Proxy = "D:\experience_codex\codex-main\codex-rs\target\debug\codex-responses-api-proxy.exe",
    [string]$ArtifactRoot = "D:\experience_codex\codex-main\m8-complex-artifacts",
    [int]$ProxyPort = 18767,
    [int]$TimeoutSec = 240,
    # The M8 run copies two real PDFs into its workspace. They are inputs, not
    # fixtures: pass your own (any two PDFs) so no personal path is baked into
    # this file.
    [string]$SourcePdfA = "",
    [string]$SourcePdfB = ""
)
$ErrorActionPreference = "Stop"

$workspace = Join-Path (Resolve-Path ".").Path "m8-complex-workspace"
if (-not $SourcePdfA -or -not (Test-Path $SourcePdfA)) { throw "pass -SourcePdfA <first PDF> (a real file, used as 2407.09450v3.pdf)" }
if (-not $SourcePdfB -or -not (Test-Path $SourcePdfB)) { throw "pass -SourcePdfB <second PDF> (a real file, used as 2603.07670v1.pdf)" }
$source2407 = $SourcePdfA
$source2603 = $SourcePdfB
$storeFixture = "fixtures\m8\known-prefix-store.json"

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
    throw "no deepseek token"
}

function Reset-Workspace {
    if (Test-Path $workspace) {
        Remove-Item -LiteralPath $workspace -Recurse -Force
    }
    New-Item -ItemType Directory -Path (Join-Path $workspace "inbox") -Force | Out-Null
    Copy-Item $source2407 (Join-Path $workspace "inbox\2407.09450v3.pdf") -Force
    Copy-Item $source2603 (Join-Path $workspace "inbox\2603.07670v1.pdf") -Force
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
    $ready = $false
    for ($i = 0; $i -lt 50; $i++) {
        try {
            $client = [System.Net.Sockets.TcpClient]::new()
            $client.Connect("127.0.0.1", $ProxyPort)
            $client.Close()
            $ready = $true
            break
        } catch {}
        Start-Sleep -Milliseconds 100
    }
    if (-not $ready) { throw "responses proxy did not start" }
    return $proc
}

function Invoke-Arm {
    param([string]$Name, [bool]$WithExperience)
    $armRoot = Join-Path $ArtifactRoot $Name
    New-Item -ItemType Directory -Path $armRoot -Force | Out-Null
    Reset-Workspace

    $dumpDir = Join-Path $armRoot "proxy-dump"
    $proxyProc = Start-Proxy -DumpDir $dumpDir
    try {
        $codexHome = Join-Path $armRoot "codex-home"
        New-Item -ItemType Directory -Path $codexHome -Force | Out-Null
        $wsLower = $workspace.ToLowerInvariant().Replace('\', '\\')
        $config = @(
            'model = "deepseek-v4-flash"',
            'model_provider = "m8proxy"',
            'model_catalog_json = "D:/experience_codex/codex-main/.codex-exp-home/models.json"',
            'approval_policy = "never"',
            'sandbox_mode = "danger-full-access"',
            '',
            '[model_providers.m8proxy]',
            'name = "m8proxy"',
            "base_url = `"http://127.0.0.1:$ProxyPort/v1`"",
            'wire_api = "responses"',
            'env_key = "M8_PROXY_KEY"',
            'requires_openai_auth = false',
            '',
            "[projects.'$wsLower']",
            'trust_level = "trusted"',
            ''
        ) -join [Environment]::NewLine
        [System.IO.File]::WriteAllText(
            (Join-Path $codexHome "config.toml"),
            $config,
            (New-Object System.Text.UTF8Encoding($false))
        )

        $storeDir = Join-Path $armRoot "store"
        New-Item -ItemType Directory -Path $storeDir -Force | Out-Null
        $store = Join-Path $storeDir "store.json"
        Copy-Item $storeFixture $store -Force
        $backup = Join-Path $armRoot "backup"
        New-Item -ItemType Directory -Path $backup -Force | Out-Null

        $prompt = @(
            'M8 complex task. Work only in this directory:',
            $workspace,
            '',
            'Complete all of the following:',
            '1. Create docs/index.md containing exactly "# Papers\n\n- 2407.09450v3\n- 2603.07670v1\n".',
            '2. Create config/settings.json containing exactly {"source":"inbox","network":"github","verified":true} plus newline.',
            '3. Move inbox/2407.09450v3.pdf and inbox/2603.07670v1.pdf into library/.',
            '4. Run git --version and python --version; write both outputs to env/versions.txt.',
            '5. Connect to GitHub: run git ls-remote https://github.com/openai/codex.git HEAD --refs and write the first line to env/github.txt.',
            '6. Create manifest.json containing SHA256 hashes for docs/index.md, config/settings.json, library/2407.09450v3.pdf, library/2603.07670v1.pdf.',
            'Verify every artifact before finishing. Do not use files outside this workspace except the inbox sources.'
        ) -join [Environment]::NewLine

        $utf8 = New-Object System.Text.UTF8Encoding($false)
        $psi = New-Object System.Diagnostics.ProcessStartInfo
        $psi.FileName = $Codex
        $psi.Arguments = "exec --json --skip-git-repo-check"
        $psi.WorkingDirectory = $workspace
        $psi.UseShellExecute = $false
        $psi.RedirectStandardInput = $true
        $psi.RedirectStandardOutput = $true
        $psi.RedirectStandardError = $true
        $psi.StandardInputEncoding = $utf8
        $psi.StandardOutputEncoding = $utf8
        $psi.StandardErrorEncoding = $utf8
        $psi.Environment["CODEX_HOME"] = $codexHome
        $psi.Environment["M8_PROXY_KEY"] = "proxy"
        $psi.Environment["RUST_LOG"] = "warn"
        if ($WithExperience) {
            $psi.Environment["EXPERIENCE_GATE_STORE"] = $store
            $psi.Environment["EXPERIENCE_GATE_CWD"] = $workspace
            $psi.Environment["EXPERIENCE_GATE_BACKUP_ROOT"] = $backup
        }
        $proc = [System.Diagnostics.Process]::Start($psi)
        $proc.StandardInput.Write($prompt)
        $proc.StandardInput.Close()
        $stdoutTask = $proc.StandardOutput.ReadToEndAsync()
        $stderrTask = $proc.StandardError.ReadToEndAsync()
        if (-not $proc.WaitForExit($TimeoutSec * 1000)) {
            $proc.Kill()
            throw "codex arm '$Name' timed out after $TimeoutSec seconds"
        }
        [System.IO.File]::WriteAllText(
            (Join-Path $armRoot "stdout.jsonl"),
            $stdoutTask.Result,
            $utf8
        )
        [System.IO.File]::WriteAllText(
            (Join-Path $armRoot "stderr.txt"),
            $stderrTask.Result,
            $utf8
        )

        $requests = @(Get-ChildItem $dumpDir -Filter "*-request.json" -File).Count
        $result = [ordered]@{
            arm = $Name
            exit = $proc.ExitCode
            model_requests = $requests
            docs = (Test-Path (Join-Path $workspace "docs\index.md"))
            config = (Test-Path (Join-Path $workspace "config\settings.json"))
            library_2407 = (Test-Path (Join-Path $workspace "library\2407.09450v3.pdf"))
            library_2603 = (Test-Path (Join-Path $workspace "library\2603.07670v1.pdf"))
            inbox_empty = -not (Test-Path (Join-Path $workspace "inbox\2407.09450v3.pdf")) -and -not (Test-Path (Join-Path $workspace "inbox\2603.07670v1.pdf"))
            versions = (Test-Path (Join-Path $workspace "env\versions.txt"))
            github = (Test-Path (Join-Path $workspace "env\github.txt"))
            manifest = (Test-Path (Join-Path $workspace "manifest.json"))
        }
        return [pscustomobject]$result
    }
    finally {
        if ($null -ne $proxyProc -and -not $proxyProc.HasExited) {
            Stop-Process -Id $proxyProc.Id -Force -ErrorAction SilentlyContinue
        }
    }
}

New-Item -ItemType Directory -Path $ArtifactRoot -Force | Out-Null
$baseline = Invoke-Arm -Name "baseline" -WithExperience $false
$experience = Invoke-Arm -Name "experience" -WithExperience $true
$rows = @($baseline, $experience)
$rows | Format-Table -AutoSize
$rows | ConvertTo-Json -Depth 4 | Set-Content (Join-Path $ArtifactRoot "summary.json") -Encoding utf8

$expected = $rows | Where-Object {
    $_.exit -ne 0 -or -not $_.docs -or -not $_.config -or
    -not $_.library_2407 -or -not $_.library_2603 -or -not $_.inbox_empty -or
    -not $_.versions -or -not $_.github -or -not $_.manifest
}
if ($expected) { exit 1 }
Write-Output "M8 COMPLEX ACCEPTANCE: PASS"

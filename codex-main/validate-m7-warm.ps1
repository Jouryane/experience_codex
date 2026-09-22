param(
    [string]$RunName = "warm1",
    [string]$Codex = "D:\experience_codex\codex-main\codex-rs\target\debug\codex.exe",
    # This is a real-machine test: a PDF sitting in $Desktop is moved into
    # $Desktop\$TargetFolder. Both are parameters, so no machine-specific path
    # is baked into this file. Put any PDF named 2407.09450v3.pdf on the
    # Desktop, or pass -Desktop/-TargetFolder.
    [string]$Desktop = [Environment]::GetFolderPath('Desktop'),
    [string]$TargetFolder = "papers-archive",
    [string]$ArtifactRoot = "D:\experience_codex\codex-main\m7-live-artifacts",
    [int]$FakeProviderPort = 18766
)
$ErrorActionPreference = "Stop"

$targetDir = Join-Path $Desktop $TargetFolder
$desktopKey = $Desktop.ToLowerInvariant().Replace('\', '\\')
$storeDir = Join-Path $ArtifactRoot "store"
$store = Join-Path $storeDir "store.json"
$fakeLog = Join-Path $ArtifactRoot "$RunName-provider.jsonl"
$fakeHome = Join-Path $ArtifactRoot "$RunName-fake-home"
$backup = Join-Path $ArtifactRoot "$RunName-backup"

New-Item -ItemType Directory -Path $ArtifactRoot -Force | Out-Null
New-Item -ItemType Directory -Path $storeDir -Force | Out-Null
if (-not (Test-Path $store)) {
    Copy-Item "fixtures\m7\move-2407-store.json" $store -Force
}
if (Test-Path $fakeLog) { Remove-Item -LiteralPath $fakeLog -Force }
New-Item -ItemType Directory -Path $fakeHome -Force | Out-Null
New-Item -ItemType Directory -Path $backup -Force | Out-Null

$fakeScript = (Resolve-Path "fixtures\m6\fake_provider.py").Path
$fakeProc = Start-Process -FilePath "python" `
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

    $config = @(
        'model = "deepseek-v4-flash"',
        'model_provider = "m7fake"',
        'model_catalog_json = "D:/experience_codex/codex-main/.codex-exp-home/models.json"',
        'approval_policy = "never"',
        'sandbox_mode = "danger-full-access"',
        '',
        '[model_providers.m7fake]',
        'name = "m7fake"',
        ('base_url = "http://127.0.0.1:{0}/v1"' -f $FakeProviderPort),
        'wire_api = "responses"',
        'env_key = "M7_FAKE_KEY"',
        'requires_openai_auth = false',
        '',
        "[projects.'$desktopKey']",
        'trust_level = "trusted"',
        ''
    ) -join [Environment]::NewLine
    [System.IO.File]::WriteAllText(
        (Join-Path $fakeHome "config.toml"),
        $config,
        (New-Object System.Text.UTF8Encoding($false))
    )

    $utf8 = New-Object System.Text.UTF8Encoding($false)
    $psi = New-Object System.Diagnostics.ProcessStartInfo
    $psi.FileName = $Codex
    $psi.Arguments = "exec --skip-git-repo-check"
    $psi.WorkingDirectory = $Desktop
    $psi.UseShellExecute = $false
    $psi.RedirectStandardInput = $true
    $psi.RedirectStandardOutput = $true
    $psi.RedirectStandardError = $true
    $psi.StandardInputEncoding = $utf8
    $psi.StandardOutputEncoding = $utf8
    $psi.StandardErrorEncoding = $utf8
    $psi.Environment["CODEX_HOME"] = $fakeHome
    $psi.Environment["M7_FAKE_KEY"] = "test"
    $psi.Environment["EXPERIENCE_GATE_STORE"] = $store
    $psi.Environment["EXPERIENCE_GATE_CWD"] = $Desktop
    $psi.Environment["EXPERIENCE_GATE_BACKUP_ROOT"] = $backup
    $psi.Environment["RUST_LOG"] = "info"
    $proc = [System.Diagnostics.Process]::Start($psi)
    $proc.StandardInput.Write("请把桌面论文 2407.09450v3 移动到 $TargetFolder 文件夹。")
    $proc.StandardInput.Close()
    $stdoutTask = $proc.StandardOutput.ReadToEndAsync()
    $stderrTask = $proc.StandardError.ReadToEndAsync()
    $proc.WaitForExit()
    [System.IO.File]::WriteAllText(
        (Join-Path $ArtifactRoot "$RunName.stdout.txt"),
        $stdoutTask.Result,
        $utf8
    )
    [System.IO.File]::WriteAllText(
        (Join-Path $ArtifactRoot "$RunName.stderr.txt"),
        $stderrTask.Result,
        $utf8
    )

    $requests = if (Test-Path $fakeLog) { @(Get-Content $fakeLog).Count } else { 0 }
    Write-Output "run=$RunName exit=$($proc.ExitCode)"
    Write-Output "source_exists=$(Test-Path (Join-Path $Desktop '2407.09450v3.pdf'))"
    Write-Output "target_exists=$(Test-Path (Join-Path $targetDir '2407.09450v3.pdf'))"
    Write-Output "fake_requests=$requests"
    if (Test-Path (Join-Path $storeDir "usage.json")) {
        Write-Output "usage:"
        Get-Content (Join-Path $storeDir "usage.json") -Raw
    }
    if ($proc.ExitCode -ne 0) { exit $proc.ExitCode }
}
finally {
    if ($null -ne $fakeProc -and -not $fakeProc.HasExited) {
        Stop-Process -Id $fakeProc.Id -Force -ErrorAction SilentlyContinue
    }
}

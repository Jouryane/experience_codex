<#
Scan the *tracked* files of both repositories for personal data before
publishing: secrets, the local account name, absolute personal paths and
personal folder names.

Only tracked files are scanned — that is exactly what a publish would upload.
Any secret-looking match is masked in the output.

Usage: powershell -NoProfile -ExecutionPolicy Bypass -File audit-privacy.ps1
#>
param(
    [string]$Workspace = "D:\experience_codex",
    # Named $Repos: PowerShell variables are case-insensitive, so a parameter
    # called $Repo would collide with the loop variable below.
    [string[]]$Repos = @("experience-main", "codex-main"),
    # Project- or person-specific words to look for as well (they are not baked
    # into this file so the file itself stays publishable).
    [string[]]$ExtraPattern = @()
)
$ErrorActionPreference = "Stop"

# The token itself never appears in the output; it is only used to detect leaks.
$configPath = Join-Path $env:USERPROFILE ".codex\config.toml"
$token = $null
if (Test-Path $configPath) {
    $match = [regex]::Match((Get-Content $configPath -Raw), 'experimental_bearer_token\s*=\s*"([^"]+)"')
    if ($match.Success) { $token = $match.Groups[1].Value }
}

$account = $env:USERNAME
$patterns = [ordered]@{
    'openai-key'         = 'sk-[A-Za-z0-9]{20,}'
    'github-token'       = 'gh[pousr]_[A-Za-z0-9]{20,}'
    'local-username'     = [regex]::Escape($account)
    'local-user-path'    = 'Users[\\/]+' + [regex]::Escape($account)
    'onedrive-path'      = 'OneDrive'
    'desktop-path'       = 'Desktop\\|/Desktop/'
}
# Upstream-generated lock files legitimately mention these words; scanning them
# only produces noise.
$skipFiles = @('MODULE.bazel.lock', 'pnpm-lock.yaml', 'Cargo.lock')
foreach ($pattern in $ExtraPattern) { $patterns["extra:$pattern"] = $pattern }
if ($token) { $patterns['the-token-itself'] = [regex]::Escape($token) }

$total = 0
foreach ($repo in $Repos) {
    $repoPath = Join-Path $Workspace $repo
    Write-Host "=== $repo ==="
    $hits = 0
    $files = & git -C $repoPath ls-files
    foreach ($file in $files) {
        if ($skipFiles -contains (Split-Path $file -Leaf)) { continue }
        $full = Join-Path $repoPath $file
        if (-not (Test-Path -LiteralPath $full)) { continue }
        if ((Get-Item -LiteralPath $full).Length -gt 5MB) { continue }
        $text = Get-Content -LiteralPath $full -Raw -ErrorAction SilentlyContinue
        if (-not $text) { continue }
        foreach ($name in $patterns.Keys) {
            foreach ($found in [regex]::Matches($text, $patterns[$name])) {
                $line = ($text.Substring(0, $found.Index) -split "`n").Count
                $preview = $found.Value
                if ($name -in @('the-token-itself', 'openai-key', 'github-token')) {
                    $preview = $preview.Substring(0, [Math]::Min(6, $preview.Length)) + '...<masked>'
                }
                Write-Host ("  [{0}] {1}:{2}  {3}" -f $name, $file, $line, $preview)
                $hits++
                $total++
            }
        }
    }
    Write-Host ("  -> {0} hit(s)" -f $hits)
}
Write-Host ""
Write-Host "TOTAL: $total hit(s)"
if ($total -gt 0) { exit 1 }

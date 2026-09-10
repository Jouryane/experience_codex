param(
    [string]$RepoDir = "D:\experience_codex\experience-main",
    [string]$Owner = "<owner>",
    [string]$Repo = "experience_codex",
    [string]$Branch = "main",
    [string]$Message = "sync: Experience project",
    [string]$Description = "",
    [string]$Topics = ""
)
# Push a local git worktree to GitHub via the REST API when the git protocol
# (github.com:443) is unreachable but api.github.com works. Creates blobs,
# a tree, a commit and the branch ref; optionally updates description/topics.
$ErrorActionPreference = "Stop"

$cred = ("protocol=https`nhost=github.com`n`n" | git credential fill) 2>$null
$token = ($cred | Where-Object { $_ -like 'password=*' } | Select-Object -First 1) -replace '^password=', ''
if (-not $token) { throw "no github credential found (git credential fill)" }
$headers = @{
    Authorization = "token $token"
    "User-Agent" = "experience-sync"
    Accept = "application/vnd.github+json"
}
$api = "https://api.github.com/repos/$Owner/$Repo"

Write-Output "== api check =="
$repository = Invoke-RestMethod -Uri $api -Headers $headers
Write-Output "repo=$($repository.full_name) default=$($repository.default_branch)"

$headSha = $null
try {
    $headRef = Invoke-RestMethod -Uri "$api/git/ref/heads/$Branch" -Headers $headers
    $headSha = $headRef.object.sha
} catch {
    # Empty repository: Git Data API needs at least one commit, so seed the
    # branch through the Contents API (README first).
    Write-Output "empty repo detected: seeding initial commit via Contents API"
    $seedPath = Join-Path $RepoDir "README.md"
    $seedBody = @{
        message = "chore: seed repository"
        content = [Convert]::ToBase64String([System.IO.File]::ReadAllBytes($seedPath))
        branch = $Branch
    } | ConvertTo-Json -Compress
    Invoke-RestMethod -Uri "$api/contents/README.md" -Method Put -Headers $headers -Body $seedBody -ContentType "application/json" | Out-Null
    $headRef = Invoke-RestMethod -Uri "$api/git/ref/heads/$Branch" -Headers $headers
    $headSha = $headRef.object.sha
}
Write-Output "head=$headSha"

Push-Location $RepoDir
try {
    # Snapshot HEAD (not the dirty worktree) so uncommitted local edits and
    # deletions never leak into the push.
    $snapshot = Join-Path $env:TEMP ("exp-sync-" + [guid]::NewGuid().ToString('N'))
    New-Item -ItemType Directory -Path $snapshot -Force | Out-Null
    $archive = Join-Path $snapshot "head.tar"
    git archive --format=tar -o $archive HEAD | Out-Null
    tar -xf $archive -C $snapshot
    Remove-Item $archive -Force

    $entries = @()
    $lines = git ls-tree -r --full-tree HEAD
    $index = 0
    foreach ($line in $lines) {
        $parts = $line -split "`t"
        $meta = $parts[0] -split ' '
        $mode = $meta[0]
        $type = $meta[1]
        $path = $parts[1]
        if ($type -ne "blob") { continue }
        $index++
        $full = Join-Path $snapshot $path
        $bytes = [System.IO.File]::ReadAllBytes($full)
        $body = @{
            content = [Convert]::ToBase64String($bytes)
            encoding = "base64"
        } | ConvertTo-Json -Compress
        $blob = Invoke-RestMethod -Uri "$api/git/blobs" -Method Post -Headers $headers -Body $body -ContentType "application/json"
        $entries += @{ path = $path; mode = $mode; type = "blob"; sha = $blob.sha }
        if ($index % 50 -eq 0) { Write-Output "blobs=$index/$($lines.Count)" }
    }
    Write-Output "blobs=$($entries.Count) total"

    $treeBody = @{ tree = $entries } | ConvertTo-Json -Depth 6 -Compress
    $tree = Invoke-RestMethod -Uri "$api/git/trees" -Method Post -Headers $headers -Body $treeBody -ContentType "application/json"
    Write-Output "tree=$($tree.sha)"

    $parents = @()
    if ($headSha) { $parents = @($headSha) }
    $commitBody = @{ message = $Message; tree = $tree.sha; parents = $parents } | ConvertTo-Json -Compress
    $commit = Invoke-RestMethod -Uri "$api/git/commits" -Method Post -Headers $headers -Body $commitBody -ContentType "application/json"
    Write-Output "commit=$($commit.sha)"

    $refUri = "$api/git/refs/heads/$Branch"
    $refBody = @{ sha = $commit.sha; force = $false } | ConvertTo-Json -Compress
    Invoke-RestMethod -Uri $refUri -Method Patch -Headers $headers -Body $refBody -ContentType "application/json" | Out-Null
    Write-Output "ref updated: $Branch -> $($commit.sha)"
    } finally {
    if ($snapshot -and (Test-Path $snapshot)) { Remove-Item -Recurse -Force $snapshot -ErrorAction SilentlyContinue }
    Pop-Location
}

if ($Description) {
    $patch = @{ description = $Description } | ConvertTo-Json -Compress
    Invoke-RestMethod -Uri $api -Method Patch -Headers $headers -Body $patch -ContentType "application/json" | Out-Null
    Write-Output "description updated"
}
$topicNames = @($Topics -split ',' | ForEach-Object { $_.Trim() } | Where-Object { $_ })
if ($topicNames.Count -gt 0) {
    $topicBody = @{ names = $topicNames } | ConvertTo-Json -Compress
    Invoke-RestMethod -Uri "$api/topics" -Method Put -Headers $headers -Body $topicBody -ContentType "application/json" | Out-Null
    Write-Output "topics updated"
}
Write-Output "SYNC DONE"

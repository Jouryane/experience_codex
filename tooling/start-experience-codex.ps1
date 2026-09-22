<#
Start the rebuilt Codex (the fork with Experience embedded).

What it guarantees:
  * the binary is the local fork build, not some other codex on PATH;
  * the Experience store the agent will read/write exists and is canonical, so
    the turn-level seat and the dispatch seam are actually armed;
  * the same store is what `codex experience list / doctor / html` shows —
    there is no second copy.

Everything else is left to the user's own ~/.codex/config.toml (model, provider,
approval policy), so this shortcut never touches credentials.

Usage:
  .\start-experience-codex.ps1                 # interactive TUI
  .\start-experience-codex.ps1 exec "..."      # any codex subcommand
  .\start-experience-codex.ps1 experience html --open
#>
param(
    # Declared first so bare tokens (`exec "..."`, `experience html`) are
    # forwarded to codex instead of being bound positionally to the options.
    [Parameter(Position = 0, ValueFromRemainingArguments = $true)]
    [string[]]$CodexArgs,
    [string]$Workspace = "D:\experience_codex",
    [switch]$NoStateSeat,
    [switch]$NoBackup
)
$ErrorActionPreference = "Stop"

$exe = Join-Path $Workspace "codex-main\codex-rs\target\debug\codex.exe"
if (-not (Test-Path $exe)) {
    throw "fork build not found: $exe`nbuild it with: cd $Workspace\codex-main\codex-rs; cargo build -p codex-cli"
}

# The fork's own home. Sharing %USERPROFILE%\.codex with the installed Codex
# desktop app makes them fight over the SQLite state DB and mixes their state.
$codexHome = if ($env:CODEX_HOME) { $env:CODEX_HOME } else { Join-Path $env:USERPROFILE ".codex-experience" }
$env:CODEX_HOME = $codexHome
if (-not (Test-Path (Join-Path $codexHome "config.toml"))) {
    throw "the fork home is not configured: $codexHome`nrun once: powershell -NoProfile -ExecutionPolicy Bypass -File `"$Workspace\setup-fork-home.ps1`""
}
$storeDir = Join-Path $codexHome "experience"
$storePath = Join-Path $storeDir "store.json"
New-Item -ItemType Directory -Path $storeDir -Force | Out-Null
if (-not (Test-Path $storePath)) {
    # An empty canonical envelope: the Gate only arms itself when the store it
    # resolves is canonical, so "no file yet" would silently mean "no seat".
    [System.IO.File]::WriteAllText(
        $storePath,
        '{"schema_version":1,"experiences":[],"templates":[]}',
        (New-Object System.Text.UTF8Encoding($false))
    )
    Write-Host "created empty experience store: $storePath"
}

$env:EXPERIENCE_ENABLED = "1"
if ($NoStateSeat) {
    # The world-state seat is on by default once a canonical store is present:
    # experiences whose trigger is `state` may run at turn start without the
    # model proposing anything. This switch turns it off.
    $env:EXPERIENCE_STATE_GATE = "0"
}
if (-not $NoBackup) {
    # Gate-taken write steps are backed up before they change anything.
    $env:EXPERIENCE_GATE_BACKUP_ROOT = Join-Path $Workspace ".experience-backups"
}

Write-Host "codex        : $exe"
Write-Host "experience   : $storePath"
Write-Host "state seat   : $(if ($env:EXPERIENCE_STATE_GATE -eq '0') { 'off' } else { 'on (default)' })"
Write-Host "backup root  : $(if ($env:EXPERIENCE_GATE_BACKUP_ROOT) { $env:EXPERIENCE_GATE_BACKUP_ROOT } else { 'off' })"
Write-Host ""

if ($CodexArgs.Count -gt 0) {
    & $exe @CodexArgs
} else {
    & $exe
}
exit $LASTEXITCODE

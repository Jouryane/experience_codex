# tooling — launchers and setup for the rebuilt Codex

Two independent applications, one build. The installed Codex app keeps its own
home; everything here drives the fork next to it.

| File | What it does |
|---|---|
| `setup-fork-home.ps1` | Creates the fork's own CODEX_HOME (`%USERPROFILE%\.codex-experience`) from your real config: model / provider / token / reasoning / auth / project trust only. Deliberately drops `notify`, `[plugins.*]` and MCP paths that belong to the desktop app. `-Force` regenerates it. The token is copied, never printed. |
| `prepare-fork-runtime.ps1` | Builds `codex-windows-sandbox-setup.exe` and `codex-command-runner.exe` from this fork and puts them next to `codex.exe`, then copies `codex-code-mode-host.exe` from the installed app (it cannot be built here: v8 needs symlink privileges). Without this the desktop UI fails with “cannot find codex-windows-sandbox-setup.exe”. Re-run after `cargo clean` or an app update. |
| `start-codex-ui.ps1` / `.cmd` | Starts the **original desktop UI** with the fork as its backend: sets `CODEX_CLI_PATH` (the app's documented override), `CODEX_HOME` and `CODEX_APP_SERVER_FORCE_CLI`, plus a dedicated `--user-data-dir` so the running app is untouched. |
| `start-experience-codex.ps1` / `.cmd` | Starts the fork's own terminal UI (handy for `codex exec` and CLI work). |
| `view-experience.cmd` | Regenerates the read-only HTML report (`codex experience html --open --store <fork store>`) and opens it. |
| `clean-real-home-leftovers.ps1` | Removes the two files an earlier version of this tooling created inside the *installed* app's home. Idempotent. |
| `audit-privacy.ps1` | Scans the tracked files of both repositories for secrets, the local account name and personal paths. The only expected hits are the deliberately fake tokens in the redaction test corpus. |
| `assets/make-icon.ps1` | Builds `assets/experience.ico` (16/32/48/256, PNG-compressed). |
| `assets/make-shortcuts.ps1` | Creates the three Desktop shortcuts (UI / terminal / viewer) with that icon. |

Order for a fresh machine:

```powershell
powershell -NoProfile -ExecutionPolicy Bypass -File .\setup-fork-home.ps1
powershell -NoProfile -ExecutionPolicy Bypass -File .\prepare-fork-runtime.ps1
powershell -NoProfile -ExecutionPolicy Bypass -File .\assets\make-icon.ps1
powershell -NoProfile -ExecutionPolicy Bypass -File .\assets\make-shortcuts.ps1
```

They need Windows PowerShell (present everywhere); `pwsh` is not required.
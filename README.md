# Experience × Codex — built-in mode

Experience is the **verified execution layer** between an Agent and the world:
the transitions you have already proven get compiled into executable bodies, the
layer takes over at the side-effect boundary, and it hands control back to the
model when the evidence is not there.

This repository now holds the **built-in mode**: Experience's canonical runtime
is embedded in a Codex fork and takes over at three seams. The previous
"external mode" layout of this repository (Experience as a standalone process
driving Codex through an Executor adapter) is still reachable in this branch's
history.

## Layout

| Path | What it is |
|---|---|
| `experience-main/` | The Experience itself: canonical runtime (core + controller), the three contracts, the Gate, learning and qualification, the management surface, UI and docs |
| `codex-main/` | The Codex fork that hosts it: two execution seams, the world-state seat, the management CLI |
| `tooling/` | Workspace scripts: launchers (desktop UI / terminal), the HTML viewer, fork-home setup, runtime readiness, privacy audit |

The two directories must stay siblings: `codex-main/codex-rs/core/Cargo.toml`
references `../../../experience-main/crates/...` as a path dependency.

## Two independent applications, one build

The rebuilt Codex runs **next to** the installed Codex app, sharing nothing.
The desktop app resolves its backend in this order (`app.asar`,
`hostConfig.codex_cli_command` → `CODEX_CLI_PATH` → its bundled codex), so the
official UI can drive the fork:

```powershell
# tooling/setup-fork-home.ps1      create the fork's own CODEX_HOME
#                                    (%USERPROFILE%\\.codex-experience) from your
#                                    real config: model/provider/token only,
#                                    no notify hook, no plugin paths
# tooling/prepare-fork-runtime.ps1 build the Windows helpers the app looks for
#                                    next to the CLI it runs
# tooling/start-codex-ui.ps1       original desktop UI + fork backend + fork home
# tooling/start-experience-codex.ps1  the fork's own terminal UI
# tooling/view-experience.cmd      write + open the read-only HTML report
```

`tooling/audit-privacy.ps1` re-runs the check that this repository contains no
personal paths, account names or secrets (the only matches it reports are the
deliberately fake tokens inside the redaction test corpus).

## What the control relationship actually is

Codex owns the loop and the proposal. Experience owns a **preemption seat that
evidence has to unlock**: it may take over only when

1. an ACTIVE, already-verified transition exists,
2. its trigger matches mechanically (task text, tool call, or world state), and
3. every precondition is observed to be true right now.

Anything unobservable is `Unknown`, and `Unknown` does not execute. The three
seams are the request (before sampling), the model's tool call (before
dispatch), and the world state (an explicit turn-start seat). A takeover must
end inside the same synchronous call; failures cannot be swallowed — they come
back as `partial`/`failed` with the side effects that really happened.

```
user request ─┬─► Experience (verified transition) ──► verified result
              └─► LLM loop ──► tool call ─┬─► Experience ──► synthesized result
                                          └─► native tool
world state ─────► Experience (opt-in seat)
```

## Build and verify

Experience alone (no token needed):

```powershell
cd experience-main
cargo test --workspace --offline      # experience-core 215 passed / 1 ignored, controller 16 passed
```

The fork (Windows GNU toolchain, as used for the recorded runs):

```powershell
$env:RUSTUP_TOOLCHAIN = 'stable-x86_64-pc-windows-gnu'
$env:PATH = 'C:\Users\<you>\.local\w64devkit\w64devkit\bin;C:\Users\<you>\.cargo\bin;' +
            'C:\Users\<you>\.rustup\toolchains\stable-x86_64-pc-windows-gnu\bin;' +
            'C:\Users\<you>\.rustup\toolchains\stable-x86_64-pc-windows-gnu\lib\rustlib\x86_64-pc-windows-gnu\bin\self-contained;' + $env:PATH
$env:CC = 'gcc'
cd codex-main\codex-rs
cargo build -p codex-cli --jobs 4
```

Real-machine acceptance (each script states its own budget):

| Script | What it proves |
|---|---|
| `codex-main\accept-m6.ps1` | canonical runtime, four scenarios (full hit / mid-action seam / prefix / unknown baseline) |
| `codex-main\validate-m7-warm.ps1` | re-entry after completion is a successful no-op, not a fallback to the model |
| `codex-main\validate-m8-complex.ps1` | multi-step task with terminal + network + manifest: baseline vs Experience arm |
| `codex-main\validate-m9-template.ps1` | one parameterized template moves two different files, zero model requests |
| `codex-main\validate-m10-v2-path.ps1` | V2 path end to end: two real cold runs → deterministic two-trace induction → CLI activation in the *same* store → warm run with zero model requests |

`codex-main\docs\architecture\m10-v2-path.md` is the current acceptance record.

## Single source of truth for the store

Both planes resolve the store through one function (`codex-rs/core/src/experience_paths.rs`):
`EXPERIENCE_GATE_STORE` when set, otherwise `<codex_home>/experience/store.json`.
The *file's own format* picks the writer, so the legacy writer can never
overwrite a canonical store. `codex experience doctor` prints the resolved path,
the format, the contents and any drift.

## Where to read

- `experience-main/docs/README.md` — the document map
- `experience-main/docs/decisions/0001..0004` — the decisions currently in force
  (goals and drift guard, canonical runtime and the two seams, template binding,
  single truth and where experience comes from)
- `experience-main/docs/discussions/` — the immutable experiment ledger
- `codex-main/docs/architecture/` — the Codex-side acceptance records (m6–m10)

## Privacy

Everything published here is checked by `tooling/audit-privacy.ps1`: no personal
paths, no account names, no prior-project references, no tokens. Machine-specific
paths in the acceptance scripts are parameters (`-Desktop`, `-SourcePdfA`,
`-TargetFolder`) or derived at runtime from `$env:USERPROFILE` / `Get-AppxPackage`.
The M7 record and its fixture use `papers-archive/` as the anonymised folder name.

## How this repository is produced

`publish-experience-codex.ps1` (kept in the workspace, not in the repository)
stages a fresh clone of this branch, replaces the tree with the committed
trees of `experience-main/` and `codex-main/` taken from each repository's
index, and commits. Build output, scratch workspaces and acceptance artifacts
are never published, and the replaced content stays in history.
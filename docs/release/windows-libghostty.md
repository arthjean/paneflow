# Windows libghostty qualification and rollback

This runbook qualifies the statically linked libghostty-vt backend before it
can become the Windows `auto` backend. It complements
`docs/WINDOWS-SMOKE-TEST.md`: the existing runbook covers the general PaneFlow
installer and UI, while this document owns QG-008 through QG-011 from
`tasks/prd-windows-libghostty-backend-2026-Q3.md`.

Until EP-005 is complete, `terminal.backend = "auto"` still resolves to
Alacritty on Windows. Select Ghostty explicitly during qualification.

## Automated evidence

The `libghostty Windows` workflow has two independent lanes:

1. `native-rebuild` checks out the pinned Ghostty commit, selects the manifest
   MSVC, Windows SDK, LLVM and Zig versions, performs two clean builds, and
   uploads the archive, symbols, header inventory, build-info, hashes, notices,
   SBOM and provenance.
2. `consumer-qualification` removes `PANEFLOW_LIBGHOSTTY_DIR`, consumes only
   `native/libghostty/prebuilt/x86_64-pc-windows-msvc`, and runs the
   differential corpus, release performance gates, 100-host startup gate,
   eight-pane GPUI input-to-paint P95 gate, 200-cycle lifecycle stress,
   32-pane stress, PE import inspection and the 15 MiB binary budget.

Run the same consumer gates locally from an x64 MSVC developer shell:

```powershell
pwsh -NoProfile -File .\scripts\qualify-libghostty-windows.ps1
```

Evidence is written below
`target/libghostty-windows-qualification/<commit>/`. Performance failures get
one bounded rerun only when every failed measurement remains inside its 5
percent band: parser ratio at least 0.855, host-creation P95 at most 525 ms and
frame P95 at most 17.535 ms for the 16.7 ms budget. A regression outside that
band fails immediately, and a second failure is final. The parser gate uses
two discarded warmup rounds followed by 20 measured rounds. The eight-pane
frame gate applies output and resize pressure to the backend-neutral GPUI
renderer; Ghostty parsing and ConPTY host behavior are covered by the separate
corpus and lifecycle gates. Do not replace a failing baseline or raise a
budget without a versioned rationale and review.

The signed release workflow first records a static candidate from the release
PE and its imports, then binds the exact binary hash to a hosted-desktop
Ghostty launch before calling the linkage runtime-verified. It also verifies
the installed MSI inventory. The package must contain `LICENSE.txt`,
`THIRD_PARTY_NOTICES.md`, `libghostty-sbom.cdx.json`,
`libghostty-manifest.toml` and `libghostty-build-info.txt`. It must not contain
or import a Ghostty DLL.

When the checked-out source and the pinned upgrade baseline are both `0.8.0`,
dispatch the dry-run with `version=0.8.0` and
`msi_upgrade_version=0.8.1`. The override changes only the non-published MSI
ProductVersion so Windows performs a real major upgrade; the binary and every
other dry-run artifact continue to report the checked-out source version. A
tag-triggered release rejects any MSI ProductVersion that differs from its tag.

## Evidence privacy

Record only these fields:

- commit SHA, MSI SHA-256, Windows edition/build, target triple and GPU/driver;
- requested backend, effective backend, failure phase, Ghostty version and OS
  error code;
- shell name/version, layout or IME name, scenario ID, pass/fail/skip and the
  evidence file name;
- aggregate duration, throughput, handles, RSS, binary size and process counts.

Never record commands, terminal output, clipboard values, OSC payloads,
environment values, usernames, full user paths, repository contents or CI
secrets. Use synthetic markers such as `PANEFLOW_QG_OK`. Redact a non-ASCII
profile path to `%USERPROFILE%\<non-ascii-profile>`.

## Required desktop matrix

Run the matrix on a clean Windows 10 22H2 x64 VM and a clean Windows 11 x64 VM.
Use a supported GPU/driver. RDP is conditional: record `SKIP(rdp-unavailable)`
when the VM host cannot expose a usable remote session. Sleep/resume and a
non-ASCII Windows user profile are required on at least one physical or nested
virtualization host that supports them.

Create an evidence directory named
`windows-libghostty-<commit>-<win10-22h2|win11>-<build>`. Its `result.json`
must contain only the privacy-safe fields above. Screenshots must be cropped to
PaneFlow chrome and synthetic fixture text.

| Area | Required cases | Pass condition |
|---|---|---|
| Platform | Windows 10 22H2 x64, Windows 11 x64, supported GPU | Signed MSI launches with no fallback or native DLL |
| Session | local desktop, optional RDP, sleep/resume | Existing panes recover input, resize and output without duplicate children |
| Profile | ASCII and non-ASCII user path | Config, hooks, shell spawn and uninstall complete without exposing the path |
| Backend | explicit `ghostty`, explicit `alacritty` | Requested and effective backend match before the child is used |

## QG-008 input and protocol matrix

Run every case with a US keyboard and at least one AltGr layout. Use a real IME,
not simulated key bytes.

| ID | Action | Expected result |
|---|---|---|
| INPUT-01 | Type ASCII, Unicode and an AltGr character | Exact committed text reaches the shell once |
| INPUT-02 | Compose a dead key, then cancel another | One composed character, no partial byte or shortcut on cancel |
| INPUT-03 | IME preedit, commit and cancel | Only committed UTF-8 reaches the PTY |
| INPUT-04 | Kitty keyboard press, repeat and release | Encoded sequence matches the active protocol and modifiers |
| INPUT-05 | Bracketed paste a synthetic multiline marker | One bounded paste, no command auto-execution |
| INPUT-06 | Mouse press, drag, wheel and release | Coordinates and button state remain coherent after resize |
| INPUT-07 | Move focus away and back | Focus reports are ordered and input is not duplicated |
| INPUT-08 | Request OSC 52 while unfocused, oversized and allowed | First two are rejected; focused policy-approved payload succeeds |
| INPUT-09 | Render valid and malformed hyperlinks | Valid URI requires explicit activation; malformed URI never executes |

Record the clipboard case as changed/unchanged only. Never save its value.

## QG-009 shells and PaneFlow workflows

For PowerShell 7, Windows PowerShell 5.1, `cmd.exe` and Git Bash, verify:

1. spawn, Unicode, colors, scrollback, resize, exit and Ctrl-C recovery;
2. OSC 7 working-directory tracking and OSC 133 command boundaries;
3. pane split/close, workspace restore and process-tree cleanup;
4. PaneFlow commands, hooks and one agent lifecycle with cwd/env preserved;
5. abrupt pane close while a child and grandchild are active.

When WSL is installed, record its distribution and WSL version, then repeat the
same cases plus Windows to `/mnt/c` path conversion and a real TUI resize. When
`wsl.exe --list --verbose` has no distribution, record
`SKIP(wsl-not-installed)`. Do not install or register a distribution as part of
release qualification.

## QG-010 Windows resilience

On both supported Windows legs, exercise a resize storm, sustained synthetic
output, mixed-width Unicode, Ctrl-C, pane close, application close and restart.
On the capable host, suspend Windows for at least 30 seconds with active panes,
resume, resize once, then verify input and one child exit. Repeat one shell and
one hook case under the non-ASCII profile. Any deadlock, duplicate child,
orphan, lost final output or backend change after spawn is a failure.

For optional RDP, connect before launching PaneFlow and repeat launch, resize,
Unicode and close. A GPUI/GPU initialization failure is recorded with phase,
target and OS code, then Alacritty is tested as the recovery path.

## QG-011 MSI, upgrade and uninstall

Use the signed candidate MSI and the signed v0.8.0 MSI as the pinned
Alacritty-only upgrade baseline.

1. Fresh-install the candidate and verify its Authenticode publisher and
   SHA-256 sidecar.
2. Inspect `%ProgramFiles%\PaneFlow`: the compliance files listed above are
   present, `paneflow.exe` is x64, imports are approved, and no `*ghostty*.dll`
   exists.
3. Launch once with an isolated profile containing
   `"terminal": { "backend": "ghostty" }`, then once with `alacritty`.
   Confirm the requested and effective values in the backend diagnostic.
4. Uninstall the candidate, install v0.8.0, create a harmless configuration
   marker, upgrade to the candidate, and confirm the marker plus explicit
   Alacritty selection survive.
5. Uninstall. No Ghostty DLL, notice, SBOM, manifest, build-info, executable or
   helper may remain under `%ProgramFiles%\PaneFlow`. User configuration is
   intentionally preserved.

The automated signed-MSI smoke covers the non-interactive subset. UAC UI,
SmartScreen presentation and real desktop backend observation remain manual.

## Diagnostic and rollback

Enable only the backend log target:

```powershell
$env:RUST_LOG = "paneflow::terminal::backend=info"
& "$env:ProgramFiles\PaneFlow\paneflow.exe"
```

A useful diagnostic identifies the requested/effective backend, startup phase,
pinned Ghostty build, target and OS code. It must not contain the command,
terminal output, clipboard, environment or a full user path.

Rollback needs one configuration edit and no reinstall. In
`%APPDATA%\paneflow\paneflow.json`, set:

```json
{
  "terminal": {
    "backend": "alacritty"
  }
}
```

Restart PaneFlow so new sessions use Alacritty. Existing sessions never switch
backend after spawning a child. Do not delete workspace or user data.

| Failure | Required diagnostic | Recovery |
|---|---|---|
| Ghostty initialization | phase `initialization`, target, version, OS code if present | Pre-spawn fallback or explicit Alacritty |
| ConPTY unavailable | phase `open_pty`, Windows build and OS code | Verify Windows support, then explicit Alacritty |
| Child spawn denied by antivirus/policy | phase `spawn`, OS code | Review policy, keep Alacritty selected |
| Failure after child creation | phase `post_spawn`, child cleanup result | No second child; close pane and select Alacritty |
| GPU/driver or RDP failure | GPUI phase and Windows build | Update driver or use a supported local session |

Qualification is complete only when automated artifacts are green, both
desktop result files are present, no failure is unexplained and every
conditional skip is justified.

# Windows browser host qualification

The Windows browser target is `x86_64-pc-windows-msvc`. EP-001 records the
signed CEF Windows64 archive at the exact CEF/Chromium/cef-rs pins shared with
the Linux contract. The target remains development-only and its native
qualification is control-only on Windows 11; GPU, presentation and public
availability remain owned by EP-002 and later epics.

The declared operating-system floor is Windows 10 version 1809, build 17763,
with Windows 11 x64 included. Windows 11 build 26200 has executed the pinned
bootstrap, sandbox entry point and control contract. This does not claim
Windows 10 execution, GPU presentation, browser-page qualification or release
signing.

## Runtime and client staging

Fetch is explicit and verifies the archive size, SHA-1, SHA-256, per-file
hashes and the complete extracted-tree digest:

```powershell
python scripts/fetch-browser.py --target x86_64-pc-windows-msvc
```

The fetch script never downloads from `build.rs` or on first browser use. The
signed runtime archive has no CEF headers or import library. The native client
build therefore also needs the standard CEF development archive at the same
`version_full` and `abi_hash`, while staging always copies the signed runtime.
Both `PANEFLOW_CEF_ROOT` and `CEF_PATH` point to the development root during
compilation:

```powershell
pwsh scripts/build-browser-host.ps1 `
  -Runtime native/browser/prebuilt/x86_64-pc-windows-msvc/<archive-sha256> `
  -CefDevelopmentRoot path/to/cef_binary_<same-version>_windows64 `
  -Bootstrap path/to/cef_binary_<same-version>_windows64/Release/bootstrap.exe
```

The script validates PE headers, checks the `RunWinMain` export when `dumpbin`
is available, records Authenticode status and writes a staging receipt. A
release build must pass `-RequireSignature`.

## Process contract

Paneflow launches the bootstrap executable, never `libcef.dll` directly. The
bootstrap loads the client DLL and invokes its `RunWinMain` export with the
CEF `sandbox_info` pointer. The `browser_subprocess_path` setting is not used
to bypass this bootstrap contract. Renderer descendants are assigned to a
Windows Job Object with `KILL_ON_JOB_CLOSE`.

Control traffic uses a per-host named pipe under `\\.\pipe\paneflow-browser-*`.
The pipe descriptor grants full access only to SYSTEM, built-in administrators
and the object owner. The host receives the workspace/session owner and the
profile path through the control bootstrap. Profile roots and the owner lock
remain inside Paneflow's per-user data directory.

The supervisor validates the manifest stamp, pinned Windows runtime files,
bootstrap PE identity, capability version and reported process identity before
the dock receives `Ready`. Any missing archive, checksum, ABI, bootstrap or
runtime contract leaves Browser unavailable and preserves terminal behavior.

## Control-only qualification receipt

The local qualification harness signs its private copy of `chrome_elf.dll`,
the bootstrap and the client with one short-lived test certificate. This is
needed because CEF requires those three binaries to share a signer, and it
must never be confused with release signing. The harness validates the real
`RunWinMain` path, `sandbox_info`, contract version, control-only presentation
and a create/close protocol round trip:

```powershell
pwsh scripts/browser-qualification/windows-bootstrap.ps1 `
  -Runtime native/browser/prebuilt/x86_64-pc-windows-msvc/<archive-sha256> `
  -Bootstrap target/windows-browser-bundle-final/lib/paneflow/paneflow-browser-host.exe `
  -ClientDll target/windows-browser-bundle-final/lib/paneflow/paneflow-browser-host.dll `
  -Output target/windows-browser-qualification `
  -SignWithTestCertificate
```

The resulting receipt is
`target/windows-browser-qualification/windows-bootstrap-qualification.json`.
The process is reclaimed by the bounded cleanup fallback after the control
round trip; the receipt records that exit status explicitly.

## Qualification still required

EP-002 owns the first browser-page execution and D3D/external-surface path.
The following evidence must be collected before changing the target to a
qualified availability:

- Windows 10 1809 and Windows 11 x64 bootstrap and sandbox harness results.
- Authenticode and client export verification on the release artifacts.
- Unknown-DLL, wrong-ABI, root-in-use, dead-host and refused-peer tests.
- Job Object descendant cleanup and profile ACL checks under a normal user.
- Native browser creation, navigation, renderer crash recovery and presentation
  smoke results.

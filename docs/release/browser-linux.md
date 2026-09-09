# Linux browser runtime packaging and maintenance

The Browser dock runs in a separate `paneflow-browser-host` process that links
the Chromium Embedded Framework runtime pinned by
[native/browser/manifest.toml](../../native/browser/manifest.toml). The terminal
application does not depend on it: a package without the runtime keeps every
terminal, editor and diff capability, and the Browser entry simply does not
appear in the dock picker.

Browser is not part of a release yet. This document is the packaging and
maintenance contract the release verdict will be checked against, not a claim
that any of it has been qualified on the distribution matrix.

## Installed layout

Every Linux artifact installs the same prefix-relative layout, so a single
resolution rule covers deb, rpm, AppImage and tar.gz:

```
<prefix>/bin/paneflow
<prefix>/lib/paneflow/paneflow-browser-host
<prefix>/lib/paneflow/browser/Release/...
<prefix>/lib/paneflow/browser/Resources/...
<prefix>/lib/paneflow/browser/verified-manifest.sha256
<prefix>/share/doc/paneflow/BROWSER_THIRD_PARTY_NOTICES.md
<prefix>/share/doc/paneflow/browser-sbom.json
```

`<prefix>` is `/usr` for deb and rpm, the AppDir's `usr` inside an AppImage, and
the extracted `paneflow.app` directory for the tar.gz.
`src-app/src/browser/install.rs` resolves the runtime from the real executable
path (`/proc/self/exe`, so the tar.gz installer's `~/.local/bin` symlink
resolves correctly), which means a package needs neither a developer checkout,
nor an installed Chrome, nor any download on first use.

`PANEFLOW_CEF_ROOT` and `PANEFLOW_BROWSER_HOST` still override the resolution
for development builds. They take precedence over the installed layout.

## Building an artifact with the browser runtime

```bash
python3 scripts/fetch-browser.py --target x86_64-unknown-linux-gnu --verify-only
cargo build --release --locked -p paneflow-app
cargo build --release --locked -p paneflow-browser-host --features cef-runtime
python3 scripts/browser-package.py stage --target x86_64-unknown-linux-gnu
```

`stage` verifies the payload it just produced and exits non-zero when a release
gate fails, so no artifact can be built from a non-compliant stage. It records
the verdict in `stage.json` either way. `--allow-open-gates` stages anyway and
still reports the failures; use it only for development against a runtime whose
gates are known to be open, never for a release.

`stage` copies the pinned runtime byte for byte. It never strips, never runs
`patchelf`, and never rewrites a resource: `verify_runtime` in
`src-app/src/browser/supervisor.rs` re-checks every manifest digest before the
host process is allowed to load `libcef`, so any post-hoc rewrite would make
Browser refuse to start. A runtime that does not fit the size budget has to be
fixed in the upstream build, not by stripping the shipped bytes.

The four formats then consume the staged prefix:

```bash
PANEFLOW_BROWSER_STAGE=target/browser-stage bash scripts/bundle-tarball.sh
PANEFLOW_BROWSER_STAGE=target/browser-stage bash scripts/bundle-appimage.sh
cargo deb --no-build --variant browser -p paneflow-app --target x86_64-unknown-linux-gnu
cargo generate-rpm -p src-app --variant browser --target x86_64-unknown-linux-gnu
```

Without `PANEFLOW_BROWSER_STAGE`, and without `--variant browser`, every script
produces exactly the terminal-only artifact it produces today.

## Verification

```bash
python3 scripts/browser-package.py verify --format deb --prefix <staged prefix> \
    --archive paneflow_<version>_amd64.deb --baseline <same package without browser>
```

`--archive` with `--baseline` measures the NFR-13 compressed overhead against
the same package built without the browser variant. `stage` already writes the
SBOM into the prefix; `sbom` regenerates it for a prefix that was assembled
elsewhere.

`verify` exits non-zero on the first failing check and prints a JSON verdict.
The checks are the layout, the manifest stamp, every pinned file digest, the
host binary's mode and RUNPATH, `libcef.so`'s RUNPATH and SONAME dependencies,
the sandbox rights, the absence of any reference to the build checkout, the
NFR-13 installed and compressed budgets, the presence of Chromium's credits
document and of the notices, and the capability declared by the manifest.

The tar.gz and AppImage bundlers regenerate the SBOM for their own prefix and
then run `verify`, printing the verdict and aborting rather than emitting an
artifact that does not pass. `cargo deb` and `cargo generate-rpm` have no such
hook, which is why `stage` refuses to produce a non-compliant prefix in the
first place: the variants can only ever consume a stage that verified.

## Sandbox rights per format

Chromium needs one of two sandbox mechanisms. `install::sandbox_mechanism`
decides which one applies when the application resolves its installation, and an
installation with neither reports Browser as unavailable with the reason instead
of opening a page. No release ever passes `--no-sandbox`; the host rejects that
switch outright.

- deb and rpm install `chrome-sandbox` mode 4755 owned by root. This is the
  only mechanism a system package can guarantee on distributions that restrict
  unprivileged user namespaces, notably Ubuntu's
  `kernel.apparmor_restrict_unprivileged_userns` policy.
- AppImage and tar.gz cannot set a setuid bit, so they fall back to the
  unprivileged user-namespace sandbox. On a host where
  `user.max_user_namespaces` is 0, `kernel.unprivileged_userns_clone` is 0, or
  `kernel.apparmor_restrict_unprivileged_userns` is enabled, the dock picker
  states that Browser is unavailable and why; terminals are unaffected. Which
  distributions restrict user namespaces in practice, and whether the setuid
  helper is enough there, is part of the US-026 matrix and is not settled by
  this rule.

## Engine and profile coupling

The application and the runtime are replaced together by one artifact. There is
no independent CEF updater and none is to be added.

- The in-app updater refuses to install while a browser page is live, so an
  operational runtime is never replaced under an open host.
- After an update, `install::detect` compares the installed runtime's
  verification stamp with the manifest digest the application was built from. A
  mismatch does not silently hide Browser: availability drops to absent and the
  dock picker states that the installation has to be repaired.
- A profile records the engine that wrote it. A profile written by a newer
  engine is refused with "This data needs a newer browser version" rather than
  being reopened blindly; an older one is migrated and keeps a `migrated_from`
  field. The marker is written through a temporary file and a rename, so a
  failed write leaves the previous marker intact.

## Engine security maintenance

This is the NFR-12 contract for the pinned runtime.

- Watch the Chrome release blog's stable-channel security posts and the CEF
  forum's release announcements for the branch the manifest pins. Chromium CVEs
  reach Paneflow through CEF, so the CEF branch build is the artifact to wait
  for, not the Chrome version number.
- Triage an applicable critical vulnerability within 48 hours of learning about
  it: decide whether the pinned branch is affected, and record the decision with
  its evidence.
- Target a corrected package within 7 days of a distributable CEF build being
  available. When upstream has no build yet, record that unavailability and the
  mitigation in the same place, within the same delay. Never publish a version
  claimed as corrected when the fix is not actually in the shipped bytes.
- Re-pin with `scripts/fetch-browser.py`, then re-run the packaging
  verification, the SBOM and the notices. A re-pin is a new qualification of the
  runtime, not a version-string edit.

## Reference qualification and open release gates

EP-005 certifies the current Paneflow application on the real Linux x86_64
reference recorded in its local receipts. The hardened candidate, four local
package variants, functional Browser path, automated security baseline and
native sandbox checks pass within that boundary. The Xorg functional/input
evidence is owner-accepted by the receipt named in the tracker; its X11-specific
M1 measurement was not executed and is not represented as measured.

The manifest still pins a stripped hardened candidate with CFI, icall CFI,
ThinLTO, PGO and official-build provenance. Its installed payload is below the
600 MiB NFR-13 ceiling and it carries Chromium's generated credits. It remains
`availability = "development"` and
`native_qualification = "hardened_candidate_not_qualified"`: EP-005 is an
internal reference certification, not a public release verdict.

EP-007 owns the open release work: presented-pixel and resource proof,
complete M1/NFR-02/03/04, the Ubuntu/Debian/Fedora/Arch/openSUSE and
compositor/GPU matrix, native-only security scenarios, and aarch64 on real ARM
GPU hardware. The manifest declares no `aarch64-unknown-linux-gnu` candidate,
so no ARM artifact can be staged today. No upload, tag, push or release was
performed for EP-005.

## EP-007 internal qualification contract

The tracked contract at
[`native/browser/linux-qualification-contract.toml`](../../native/browser/linux-qualification-contract.toml)
closes the implementation part of EP-007 without promoting a release. It
requires every extended target and every SEC-01 through SEC-08 case to carry an
explicit proof kind and status. Missing native evidence is represented as
`NATIVE_DEFERRED`; it is never inferred from a build, an emulator, a CI runner
or an unrelated Linux reference.

The ARM64 development path is verified by
`ubuntu-22.04-arm` in GitHub Actions with the pinned upstream CEF archive,
archive digests, safe extraction, ELF architecture, CEF-host tests, Clippy and
release builds. Its artifact is `CI_CODE_TEST_BUILD`, not native ARM GPU
qualification. The workflow is wired and its local runtime verifier passes;
the first hosted receipt still requires a pushed branch. The strict M1 and
presentation analyzers remain covered by rejection tests and preserve
`NOT_EXECUTED` when a physical compositor oracle is absent.

EP-007 being internally complete therefore does not change
`availability = "development"`,
`native_qualification = "hardened_candidate_not_qualified"`, or the public
release verdict `NOT_QUALIFIED`. A future native campaign must replace the
deferred statuses before those capabilities can be announced.

The capability the application reports comes from the manifest's per-target
`availability` field. Raising it above `development` requires the separate
release verdict and does not happen as a side effect of epic certification.

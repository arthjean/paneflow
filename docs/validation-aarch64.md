# aarch64 artifact validation runbook

On-device validation of every aarch64 artifact produced by the release
workflow, before a release is promoted to the public "latest" tag.
It implements the historical US-020 validation contract after the US-018
go-decision and uses the US-019 matrix in
[`.github/workflows/release.yml`](../.github/workflows/release.yml).

CI cannot exercise this path - GitHub's `ubuntu-22.04-arm` runner has no
GPU, so `paneflow` (which initializes Vulkan at startup) crashes pre-main
on a headless runner. The only way to confirm the aarch64 build actually
renders is to run it on a real aarch64 machine with a display.

---

## When to run

Run this runbook against every release candidate **before** the aarch64
assets are attached to the public GitHub Release. The expected cadence is:

1. Maintainer pushes `vX.Y.Z-rc.1` - the release workflow cuts a
   **pre-release** (tag-name match `rc` sets `prerelease: true` in
   `release.yml`). Both arch artifact sets land on the pre-release page.
2. Maintainer executes this runbook on at least one aarch64 machine.
3. If every check in §4 passes, maintainer retags as `vX.Y.Z` - the
   workflow cuts the final release. If any check fails, see §6.

Do NOT skip steps because "nothing changed on aarch64 since last
release" - GPUI commits bump frequently and silently shift Vulkan
behaviour; every release is a fresh validation.

---

## 1. Target devices

| Tier | Device | Distro | Rationale |
|------|--------|--------|-----------|
| **Primary** | Raspberry Pi 5 (8 GB) | Ubuntu 24.04 Desktop (arm64) | Real GPU (VideoCore VII), mainline Mesa, Wayland + X11 both supported, glibc 2.39. Highest-fidelity signal for "mainstream aarch64 user". |
| Secondary | Apple Silicon laptop | Asahi Linux (Fedora 40 remix) | Catches Asahi-Mesa-specific bugs that a stock Mesa user never sees. Use when the primary device is unavailable or when the release notes will mention Asahi support. |
| Tertiary | AWS Graviton EC2 (c7g.xlarge or larger) | Ubuntu 22.04 with a virtual framebuffer (Xvfb) | Last-resort, low-fidelity - no real GPU, so GPUI falls back to Vulkan's software renderer (`lavapipe`). Only use to prove the **binary loads** when no hardware is available. Not a substitute for primary/secondary. |

**glibc floor:** 2.35. Release notes should flag the floor so users on
older ARM distros (Ubuntu 20.04 aarch64, Debian 11 aarch64) aren't
surprised. This matches the glibc floor on x86_64 - same GPUI pinned
commit, same standard library surface.

---

## 2. Prerequisites

On the test device:

```bash
# Create a scratch workspace for the validation run
mkdir -p ~/paneflow-validation && cd ~/paneflow-validation

# Record the environment context for the release-notes artifact.
# `uname -srvmpio` drops the hostname that plain `uname -a` would
# include - `env.txt` is uploaded to the public release page, so
# we don't want a maintainer's `mylaptop.home.arpa` showing up in
# there. Same idea for `glxinfo`: `-B` is the brief form that
# avoids the `Device UUID` line some Mesa drivers emit (a stable
# per-GPU identifier that users may not want published).
uname -srvmpio > env.txt
( lsb_release -ds 2>/dev/null || cat /etc/os-release ) >> env.txt
glxinfo -B 2>/dev/null | grep -v -iE 'uuid|serial' | head -20 >> env.txt
echo "XDG_SESSION_TYPE=$XDG_SESSION_TYPE" >> env.txt  # wayland vs x11
```

Before uploading `env.txt` in §5, re-read it. If it contains a
hostname, a GPU UUID, or anything else you'd rather not publish,
scrub it with a text editor. The redaction above catches the common
cases but isn't exhaustive on every Mesa version / distro.

Fetch the four aarch64 artifacts from the candidate release. Replace
`v0.2.0-rc.1` with the real tag:

```bash
TAG=v0.2.0-rc.1
base=https://github.com/ArthurDEV44/paneflow/releases/download/${TAG}
for f in "paneflow-${TAG}-aarch64.tar.gz" \
         "paneflow-${TAG}-aarch64.deb" \
         "paneflow-${TAG}-aarch64.rpm" \
         "paneflow-${TAG}-aarch64.AppImage" \
         "paneflow-${TAG}-aarch64.AppImage.zsync" \
         "paneflow-${TAG}-aarch64.tar.gz.sha256" ; do
    curl --fail --location --remote-name "${base}/${f}"
done
sha256sum --check "paneflow-${TAG}-aarch64.tar.gz.sha256"
```

The `sha256sum --check` line must exit `0`. This verifies only the
tar.gz - the release workflow does not emit a `.sha256` sidecar for
`.deb`, `.rpm`, or the AppImage. Per-format authenticity lives
elsewhere: `.deb` and `.rpm` carry embedded GPG signatures (verify
with `dpkg-sig --verify` and `rpm --checksig`, following the
maintainer steps in [`docs/release-signing.md`](./release-signing.md)),
and the AppImage's zsync metadata is validated by
`appimageupdatetool` at update time.

**Authenticity caveat** (US-020 H1): `sha256sum --check` above
detects *integrity* failures (transit corruption, partial downloads)
but NOT *authenticity* failures. The `.sha256` file is fetched from
the same GitHub release page as the artifact - an attacker who
replaces the `.tar.gz` can trivially replace the `.sha256` to match.
For a motivated-attacker threat model, cross-check the artifact
hashes against the GitHub Actions workflow run log for the release
tag (`gh run view <run-id> --log | grep -A2 'paneflow-.*aarch64.*sha256'`)
 -  the log is a separate channel from the release page and a
compromise of one does not automatically compromise the other.

If the mismatch persists after a re-download from the workflow log's
cross-checked hashes, re-cut the release workflow.

Install `asciinema` if you don't already have it - it's the recording
tool §5 uses for the evidence artifact.

```bash
sudo apt install -y asciinema    # Debian/Ubuntu
sudo dnf install -y asciinema    # Fedora/RHEL
```

---

## 3. Install each format

Test each format on a **clean user session**. On the primary device
(RPi 5) the simplest clean state is a fresh user: `sudo adduser
paneflow-test && su - paneflow-test`. After each format's smoke test
(§5), uninstall before moving to the next to avoid cross-format
contamination of `$XDG_DATA_HOME`.

### 3.1 tar.gz (Zed-style user-local install)

```bash
cd ~/paneflow-validation
tar -xzf "paneflow-${TAG}-aarch64.tar.gz"
./paneflow.app/install.sh          # installs to ~/.local/paneflow.app and symlinks ~/.local/bin/paneflow
~/.local/bin/paneflow --version    # expect: paneflow 0.2.0 (or matching tag sans `v`)
```

Uninstall between test runs:

```bash
rm -rf ~/.local/paneflow.app ~/.local/bin/paneflow \
       ~/.local/share/applications/paneflow.desktop \
       ~/.local/share/icons/hicolor/*/apps/paneflow.png
```

### 3.2 .deb (Ubuntu 24.04 on RPi 5)

```bash
sudo apt install -y ./paneflow-${TAG}-aarch64.deb
paneflow --version
```

Uninstall:

```bash
sudo apt remove -y paneflow
```

### 3.3 .rpm (Asahi Linux / Fedora 40 arm64)

```bash
sudo dnf install -y ./paneflow-${TAG}-aarch64.rpm
paneflow --version
```

Uninstall:

```bash
sudo dnf remove -y paneflow
```

### 3.4 AppImage

```bash
chmod +x paneflow-${TAG}-aarch64.AppImage
./paneflow-${TAG}-aarch64.AppImage --version
```

If FUSE 2 is missing (Ubuntu 24.04 default - `libfuse2` is NOT
pre-installed on arm64 either), the runner exits with "dlopen():
error loading libfuse.so.2". Verify the fallback path works:

```bash
./paneflow-${TAG}-aarch64.AppImage --appimage-extract-and-run --version
```

`paneflow` does NOT write to `/usr`, `/opt`, or `/etc` - the AppImage
self-mounts under `/tmp/.mount_*` and is untraced on disk after exit.
No uninstall step required.

---

## 4. Graphical smoke actions

After each of the four installs above, launch `paneflow` in a
**graphical session** (NOT an SSH session - GPUI requires a real
Wayland or X11 display). On the RPi 5 that's the Ubuntu desktop
(Wayland by default on 24.04). On Asahi it's the KDE Plasma session.

Run every step in the checklist below while recording with `asciinema`.

> **Heads-up:** `asciinema` records the **terminal** that launched
> `paneflow`, not the GPUI window. The `.cast` is therefore only
> useful as a stdout/stderr timeline - it proves `paneflow` started
> cleanly but does NOT show the pane you're actually smoke-testing.
> You also need a phone photo or a dedicated screen-capture tool
> (OBS, `wf-recorder` on Wayland, `ffmpeg -f x11grab` on X11)
> pointed at the `paneflow` window to capture visual evidence.

```bash
asciinema rec aarch64-${TAG}-${format}-smoke.cast
```

Smoke checklist - every item is a hard pass/fail:

- [ ] **Launch:** `paneflow` opens a window within 2 s. No Vulkan
  validation errors in stderr. No immediate crash.
- [ ] **Initial pane renders:** the default terminal pane shows a
  prompt. Cursor blinks. Text is readable (glyph atlas works on the
  aarch64 GPU).
- [ ] **Keyboard input:** `echo hello aarch64` → pressing Enter → the
  shell echoes `hello aarch64`. Rules out keymap-layer regressions.
- [ ] **Horizontal split:** `Ctrl+Shift+D` → the pane splits into two
  stacked terminals. Focus lands on the new pane (visible border /
  cursor).
- [ ] **Vertical split:** `Ctrl+Shift+E` → one of the two panes splits
  into side-by-side halves. Three total panes visible.
- [ ] **Type in each pane:** focus each of the three panes via
  `Alt+Arrow`, type a distinct shell command (`date`, `ls`, `uname`).
  Each pane must receive keystrokes independently.
- [ ] **Drag-to-resize:** grab the horizontal divider with the mouse,
  drag it up and down. Panes resize smoothly. Release - layout
  persists.
- [ ] **Close pane:** `Ctrl+Shift+W` twice → back to a single pane.
  Layout collapses cleanly, no orphaned borders.
- [ ] **Exit:** click the close button in the title bar. Window
  destroys. `paneflow` process terminates cleanly (check with
  `pgrep paneflow` - should return nothing).

Stop `asciinema` with `Ctrl+D`. The `.cast` file plus a screenshot
(phone photo is fine) are the evidence artifacts for AC2.

Any failure on any checkbox is a RUNBOOK-FAIL. See §6.

---

## 5. Evidence capture for release notes

Once all four formats pass §4 on the primary device, assemble the
evidence bundle:

1. One `asciinema` `.cast` per format (or a combined cast covering all
   four - maintainer's call).
2. Two to four screenshots of the window showing different smoke
   states (initial pane, 2-way split, 3-way split, resize in
   progress).
3. The `env.txt` recorded in §2.

Upload the evidence as extra attachments on the release page. Edit the
GitHub Release body to add a section:

```markdown
### aarch64 validation (US-020)

Validated on:
- Raspberry Pi 5 (8 GB) - Ubuntu 24.04 Desktop, kernel 6.8, Mesa 24.2,
  Wayland - 2026-MM-DD.

Evidence attached: `aarch64-<tag>-*.cast`, `aarch64-screenshot-*.png`,
`aarch64-env.txt`.

Formats passed: tar.gz, .deb, .rpm, AppImage.
```

If Asahi Linux was also tested, add a second validation line. Clearly
list ANY format that was NOT tested on a given device - users reading
the release notes need to know whether, say, `.rpm` was exercised on
Asahi or only Fedora.

---

## 6. Unhappy path - validation failure (AC4)

If **any** smoke check fails at runtime - even if `paneflow --version`
succeeds - the aarch64 assets for that release are considered
**unvalidated** and MUST NOT be attached to the public release.

The release workflow already keeps an `-rc.N` or `-alpha.N` tag as a
`prerelease: true` GitHub Release (see
[`release.yml`](../.github/workflows/release.yml) - `prerelease: ${{
contains(github.ref_name, 'alpha') || contains(github.ref_name,
'beta') || contains(github.ref_name, 'rc') }}`). The validation
procedure in this runbook is designed to run against a pre-release
tag **before** the maintainer retags to the final `vX.Y.Z`. That
gives three escape hatches:

### 6.1 Remove the broken aarch64 assets from the pre-release

Use this when the failure is aarch64-specific and x86_64 is known good.
The x86_64 assets can ship as planned; the aarch64 set is withheld.

```bash
# Authenticated via `gh auth login`
TAG=v0.2.0-rc.1
# `gh release delete-asset --yes` is irreversible. Print the asset
# list for the resolved tag FIRST so you can visually confirm you
# are pointed at the right release - a `TAG` typo here permanently
# removes assets from a different release.
gh release view "${TAG}" --json assets --jq '.assets[].name'
for suffix in tar.gz tar.gz.sha256 deb rpm AppImage AppImage.zsync ; do
    gh release delete-asset "${TAG}" "paneflow-${TAG}-aarch64.${suffix}" --yes
done
# Confirm only x86_64 assets remain
gh release view "${TAG}" --json assets --jq '.assets[].name'
```

Then proceed to cut the final `vX.Y.Z` tag with **x86_64 only** - the
maintainer must manually remove the aarch64 assets from that release
as well (or re-run the workflow with a patched matrix that skips the
aarch64 leg).

### 6.2 Keep the pre-release as a pre-release

Use this when you want aarch64 users to still get the pre-release bits
with a clear "this is under test" label but don't want to promote it
to `latest`. Just don't retag - the pre-release stays as-is. Users who
click "latest" see the previous stable `vX.Y.Z`; users who navigate to
the specific `-rc.N` page see the new artifacts with GitHub's
"Pre-release" banner.

File an issue with the §7 template so the bug is tracked; mention the
pre-release tag in the issue so users searching for the bug can find
the workaround (or the known-broken build).

### 6.3 Re-cut as a draft

Use this when validation reveals a systemic issue on both arches (in
which case the whole release is suspect). Delete the pre-release
entirely via `gh release delete`, open a draft release manually via
`gh release create --draft`, upload the assets for internal review
only, and hold until the bug is fixed and a new candidate tag is cut.

---

## 7. Reporting bugs found during validation

If a smoke check fails, open a GitHub issue using the
[`aarch64-bug-report.md`](../.github/ISSUE_TEMPLATE/aarch64-bug-report.md)
template. The template prompts for distro, kernel, GPU, install
format, and reproduction steps - everything needed to triage from a
different machine.

Label the issue `aarch64` and (if it blocks the release) `release-blocker`.
Link the issue from the pre-release's GitHub Release body so users
who hit the same symptom find the triage thread.

---

## 8. Promote to stable

Only after §5 is complete and no §6 path was taken:

```bash
# Assuming rc.1 validated cleanly; retag as the final release
git tag -a vX.Y.Z -m "Release vX.Y.Z"
git push origin vX.Y.Z
```

The release workflow runs again, producing 12 assets (two arches ×
six per-arch files: `.tar.gz`, `.tar.gz.sha256`, `.deb`, `.rpm`,
`.AppImage`, `.AppImage.zsync`). Because the tag no longer contains
`rc`/`beta`/`alpha`, the workflow publishes it as the new `latest`
release instead of a pre-release.

One final check after publish: `gh release view vX.Y.Z --json assets
--jq '.assets[].name'` must list both `aarch64` and `x86_64` asset
sets. If either is missing, the release is incomplete and the
post-merge assertion fails - investigate the workflow run before
announcing the release.

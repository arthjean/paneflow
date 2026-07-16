# PaneFlow release runbook

Step-by-step checklist for cutting a new PaneFlow release. Written for
the maintainer coming back cold after a month away - every step has a
time budget, a clear pass/fail signal, and a "what to do if it
breaks" box. Referenced by US-021.

**Target total time: ≤ 15 minutes for a happy-path release.** If any
step pushes you past that, check the step's troubleshooting box before
plowing on - the runbook has probably already anticipated the failure.

**Last validated on:** _unknown - update this after the next release dry run._
(Update this line every release so a returning maintainer knows the
runbook has actually been exercised recently. See the "Dry-run
validation" section at the bottom for the contract.)

Related runbooks:

- [`docs/pkg-repo-runbook.md`](./pkg-repo-runbook.md) - one-time R2 +
  repo bootstrap, GPG key creation.
- [`docs/release-signing.md`](./release-signing.md) - deep-dive on the
  signing pipeline, key rotation.
- [`docs/validation-aarch64.md`](./validation-aarch64.md) - on-device
  aarch64 validation (execute for releases that aarch64 users depend
  on).

Prerequisites (one-time, not part of the per-release cadence):

- `gh` CLI authenticated with `repo` scope (`gh auth status`).
- GitHub secrets populated: `GPG_PRIVATE_KEY`, `GPG_PASSPHRASE`,
  `GPG_KEY_ID`, `R2_ACCESS_KEY_ID`, `R2_SECRET_ACCESS_KEY`,
  `R2_ENDPOINT`, `R2_BUCKET`. See `docs/pkg-repo-runbook.md` §4.
- Docker installed locally (used in Step 6 smoke tests).

---

## Supported release targets (US-008)

Authoritative status of every platform target PaneFlow releases can
ship to. Cross-reference `.github/workflows/release.yml`'s matrix.

| Target | Status | Ships | Gate level | Restore requires |
|---|---|---|---|---|
| `x86_64-unknown-linux-gnu` | **Active** | .deb, .rpm, AppImage, .tar.gz | Hard-required (gates the whole release) | - |
| `aarch64-unknown-linux-gnu` | **Active** | .deb, .rpm, AppImage, .tar.gz | Hard-required (gates the whole release) | - |
| `aarch64-apple-darwin` | **Active** | .dmg | Hard-required (gates the whole release) | - |
| `x86_64-apple-darwin` | **Closed** | - | Removed from matrix entirely | Reopen only when Intel DMG signing is provisioned and the Homebrew cask is widened back to Intel |
| `x86_64-pc-windows-msvc` | **Active** | signed .msi | Hard-required (gates the whole release) | - |
| `aarch64-pc-windows-msvc` | **Closed** | - | Not in matrix | Reopen only when GPUI DX11 ARM64 reliability, signing, and runner coverage are committed |

**Interpretation:**
- *Hard-required*: a failure blocks the release. The `Publish GitHub
  Release` job's `needs:` waits for a green result.
- *Best-effort*: in the matrix with `continue-on-error: true`.
  Catches Rust-level cross-compile regressions at PR time but a
  failure does NOT prevent the release from being published.
- *Closed*: deliberately not in the matrix. Do not silently re-add a
  closed target - adding back requires the listed prerequisites AND a
  status update to this table.

**Closed target rule:** do not re-add `x86_64-apple-darwin` or
`aarch64-pc-windows-msvc` as best-effort release legs. Reopening either
target requires a committed artifact path, signing path, docs update, and
release-gate decision in the same change.

---

## Step 1 - Pre-flight (≈ 3 min, manual judgement required)

Work on `main`. All changes for this release must already be merged.

```bash
git switch main
git pull --ff-only
git status                       # working tree clean? if not, stop.
cargo fmt --check
cargo clippy --workspace -- -D warnings
cargo test --workspace
```

**Manual judgement:** read the test output, not just the exit code.
`cargo test` exits 0 even when a test is marked `#[ignore]` and quietly
skipped - scan the summary for unexpected ignores, new warnings, or
flaky tests that passed this run but failed the previous one.

Bump versions. Single source of truth for the Rust version is the
workspace `Cargo.toml`:

```bash
# Run this block from the repository root. The `sed` and changelog write use
# relative paths and silently misbehave if CWD is a subdirectory.
cd "$(git rev-parse --show-toplevel)"

# Set the new version ONCE, then reuse via the shell var. Pasting the
# block verbatim without setting VERSION will fail loudly - the guard
# below enforces a valid semver.
VERSION="X.Y.Z"   # <-- EDIT THIS before running. No `v` prefix.
[[ "$VERSION" =~ ^[0-9]+\.[0-9]+\.[0-9]+$ ]] \
  || { echo "invalid VERSION='$VERSION' (expected N.N.N)"; return 1 2>/dev/null || exit 1; }

# Only the workspace root Cargo.toml carries a literal `version = "..."`;
# src-app/Cargo.toml and crates/*/Cargo.toml use `version.workspace = true`
# and will inherit automatically. This sed targets workspace root only.
sed -i "s/^version = \".*\"$/version = \"$VERSION\"/" Cargo.toml

# Debian changelog needs a new stanza - format matters (dpkg-parsechangelog is strict).
# Force C locale on `date` so the trailer is RFC-2822 English even on
# a host with a non-English LANG setting (dpkg-parsechangelog rejects
# localized date strings).
cat > /tmp/paneflow-changelog-entry.txt <<EOF
paneflow ($VERSION-1) stable; urgency=medium

  * Release v$VERSION. See GitHub release notes for the full diff.

 -- Arthur Jean <arthur.jean@strivex.fr>  $(LC_TIME=C date -R)

EOF
cat /tmp/paneflow-changelog-entry.txt debian/changelog > debian/changelog.new
mv debian/changelog.new debian/changelog

# Let cargo rewrite Cargo.lock for the new version
cargo build -p paneflow-app --release 2>/dev/null || true
cargo check --workspace

# Commit the bump
git add Cargo.toml Cargo.lock debian/changelog
git commit -m "chore: bump version to v$VERSION"
git push origin main
```

Pass signal: the commit lands on `main` via a green push (pre-commit
hooks or required checks don't block).

### Troubleshooting - Step 1

| Symptom | Top 3 recoveries |
|---|---|
| `cargo test` fails on a flaky test | 1. Re-run the specific test with `cargo test <name> -- --nocapture`. 2. If genuinely flaky, file an issue and mark `#[ignore]` in a separate commit BEFORE tagging - don't tag a known-broken release. 3. If the failure is real, fix it and restart Step 1. |
| Working tree not clean (leftover unstaged changes) | 1. `git stash` to park the noise. 2. `git diff` to audit each change - uncommitted work from a different branch should be committed or stashed, never force-discarded. 3. Only after `git status` is clean do you proceed. |
| `dpkg-parsechangelog: error: no version` when validating `debian/changelog` | 1. The changelog stanza format is strict - `name (VERSION-REVISION) DISTRIBUTION; urgency=LEVEL`. Copy the previous stanza and edit only the version/date. 2. The `-- <name> <email>  <date>` trailer needs **two** spaces before the date. 3. Validate locally with `dpkg-parsechangelog -l debian/changelog` before pushing. |

---

## Step 2 - Tag and push (≈ 1 min)

```bash
# Tag the bump commit with an annotated tag
git tag -a vX.Y.Z -m "Release vX.Y.Z"
git push origin vX.Y.Z
```

**Pre-release convention:** if you need on-device aarch64 validation
before promoting to `latest`, tag as `vX.Y.Z-rc.1` first. The
`release.yml` workflow matches `rc`/`beta`/`alpha` in the tag name and
publishes as `prerelease: true`. After validation (the Pre-announce section or
`docs/validation-aarch64.md`), retag as `vX.Y.Z` to cut the final
release.

### Troubleshooting - Step 2

| Symptom | Top 3 recoveries |
|---|---|
| Tag already exists on the remote | 1. Do NOT `git push --force` - that overwrites an immutable release marker and breaks anyone's `git fetch`. 2. Decide if the existing tag should be consumed as-is (rare) or if you need a new version number (usual). 3. If a broken tag was pushed and the release workflow produced bad artifacts, bump to the next patch and re-tag - the dud release stays on GitHub as a historical record. |
| Push rejected because `main` moved after you committed | 1. `git pull --rebase origin main`, re-inspect `git log` to confirm your bump commit is still the tip. 2. Re-run `cargo test` locally - a racing merge may have introduced conflicts that Git resolved cleanly but the test suite doesn't. 3. Only then `git push` and re-`git push origin vX.Y.Z`. |
| Pushed the tag but forgot to commit the version bump | 1. **IRREVERSIBLE for anyone who already `git fetch`-ed** - even a few seconds of exposure means downstream clones may carry the bad tag indefinitely. `git tag -d vX.Y.Z` locally and `git push --delete origin vX.Y.Z` to remove the remote tag. 2. If the release workflow already created a GitHub Release object for the tag, delete that separately: `gh release delete vX.Y.Z --cleanup-tag --yes`. 3. Make the version-bump commit, re-tag on the new commit, push. Announce in release notes that the original tag was retracted so downstream forks update. |

---

## Step 3 - Monitor release.yml (≈ 15 min, manual judgement required)

```bash
# `gh run list --branch=` does NOT match tag refs - for tag-triggered
# workflows we filter by event=push and take the most recent run.
# The tag name is informational only (to sanity-check that the run
# you're watching is actually the one you just pushed).
RUN_ID=$(gh run list --workflow=release.yml --event=push --limit=1 \
          --json databaseId,headBranch --jq '.[0].databaseId')
gh run watch --exit-status "$RUN_ID"
```

Budget from US-019 AC4: **total matrix wall-clock < 25 min** for both
arches combined (`fail-fast: true` means both legs must finish for the
release job to run). First aarch64 run after an `ubuntu-22.04-arm`
runner cold start may burn 2-3 extra minutes on queueing - acceptable.

**Manual judgement:** if the workflow is green but a step emitted a
`::warning::` annotation (`lintian`, `dpkg-sig` verify-tail output, or
GPG fingerprint mismatch warning), stop here and read the annotation
before proceeding - a warning on the signing leg is often a silent
"unsigned release would have shipped if we hadn't tripped the
fingerprint guard" near-miss.

### Troubleshooting - Step 3

| Symptom | Top 3 recoveries |
|---|---|
| aarch64 leg queues for > 5 min on `ubuntu-22.04-arm` | 1. Wait - GitHub's free-tier ARM queue is bursty and usually clears within 10 min. 2. If genuinely stuck > 20 min, cancel via `gh run cancel <id>` and re-run the workflow from the Actions UI. 3. Persistent queueing may mean the runner label is wrong post-GitHub-maintenance; verify the runner matrix in [`.github/workflows/release.yml`](../.github/workflows/release.yml) and follow [`docs/validation-aarch64.md`](validation-aarch64.md). |
| GPG signing step exits with `GPG signing failed: key not loaded` | 1. A required secret (`GPG_PRIVATE_KEY`, `GPG_KEY_ID`, `GPG_PASSPHRASE`) is missing or malformed. Check the run log for the exact secret referenced. 2. Re-populate the secret from the password manager (see `docs/pkg-repo-runbook.md` §4). 3. Re-run the failed job - don't re-tag. |
| `fail-fast: true` cancelled the x86_64 leg after aarch64 failed | 1. Click through to the aarch64 log and find the real failure - `fail-fast` makes the reported failure cascade-prone. 2. If aarch64 is the blocker and you need an x86_64-only release, patch the matrix in a hotfix commit to skip aarch64, re-tag to a patch number, and note the aarch64 gap in release notes. 3. If aarch64 is a first-time failure since a GPUI bump, follow [`docs/validation-aarch64.md`](validation-aarch64.md) and check for an upstream GPUI regression. |

---

## Step 4 - Verify artifacts attached (≈ 2 min)

```bash
gh release view vX.Y.Z --json assets --jq '.assets[].name' | sort
```

Expected primary artifacts:

```
paneflow-X.Y.Z-aarch64.AppImage
paneflow-X.Y.Z-aarch64.deb
paneflow-X.Y.Z-aarch64.rpm
paneflow-X.Y.Z-aarch64.tar.gz
paneflow-X.Y.Z-x86_64.AppImage
paneflow-X.Y.Z-x86_64.deb
paneflow-X.Y.Z-x86_64.rpm
paneflow-X.Y.Z-x86_64.tar.gz
paneflow-X.Y.Z-aarch64-apple-darwin.dmg
paneflow-X.Y.Z-x86_64-pc-windows-msvc.msi
```

Each primary artifact must have a `.sha256` sidecar and a `.minisig`
signature. Each AppImage must also have an `.AppImage.zsync` sidecar. A
missing or renamed asset breaks the in-app updater's asset matcher (it
looks up by `-<arch>.<format>` suffix, see `src-app/src/update/checker.rs`).

### Troubleshooting - Step 4

| Symptom | Top 3 recoveries |
|---|---|
| Asset count is 6 (one arch missing) | 1. The `fail-fast: true` matrix should have prevented this. Check whether the `release` job in `release.yml` ran - if one matrix leg silently skipped artifact upload, that's a release.yml bug. 2. Re-run the missing leg via `gh run rerun --failed`, which re-uploads without re-tagging. 3. If neither arch's asset set is complete, delete the broken release (`gh release delete vX.Y.Z --cleanup-tag`) and restart at Step 1 with a patch bump. |
| Asset names have the wrong arch suffix (e.g., `_amd64.deb` instead of `-x86_64.deb`) | 1. The staging step in `release.yml` renames to the canonical form; a missing rename is a workflow regression. 2. Do NOT publish - the updater will not find assets and user upgrades break. 3. Patch the staging step, cut a new patch tag. |
| Pre-release ended up on `latest` | 1. The tag contains `rc`/`beta`/`alpha` but the workflow's prerelease boolean is false - double-check the `contains(...)` expression in `release.yml`. 2. Manually flip the release to pre-release: `gh release edit vX.Y.Z --prerelease`. 3. Fix the workflow expression in a follow-up commit. |

---

## Step 5 - Verify repo-publish.yml (≈ 10 min, auto-chained from release.yml)

`repo-publish.yml` auto-chains off `release.yml` completion via GitHub
Actions' `workflow_run` trigger (US-003). A successful tag-push
`release.yml` run fires `repo-publish.yml` within ~30 s of the parent
job finishing - no manual `gh workflow run` is needed on the happy path.
Prerelease tags (`-rc.N`, `-alpha.N`, `-beta.N`) are filtered out on the
auto-chain path, so they do NOT push to the stable repo (intentional:
stable is stable-only).

```bash
gh run watch --exit-status $(gh run list --workflow=repo-publish.yml --limit=1 --json databaseId --jq '.[0].databaseId')
```

Then verify the repo metadata is fresh:

```bash
# APT - InRelease file must include today's date
curl --fail --silent https://pkg.paneflow.dev/apt/dists/stable/InRelease \
  | grep -E '^Date:'
# RPM - repomd.xml should list a paneflow-<version>.rpm whose version
# matches what you just tagged. We use a semver regex so the grep
# works even if the maintainer forgot to substitute X.Y.Z.
curl --fail --silent https://pkg.paneflow.dev/rpm/repodata/repomd.xml \
  | grep -oE 'paneflow-v[0-9]+\.[0-9]+\.[0-9]+[^"]*' | head -1
```

### Troubleshooting - Step 5

| Symptom | Top 3 recoveries |
|---|---|
| `repo-publish.yml` didn't fire | 1. Check whether the tag is a pre-release (`-rc.N`/`-alpha.N`/`-beta.N`) - the auto-chain filters those out by design; manually re-publish via `gh workflow run repo-publish.yml -f tag=vX.Y.Z` if you really want a prerelease on the stable repo. 2. Check the `release.yml` parent run succeeded - the `workflow_run` guard requires `conclusion == 'success'` AND `event == 'push'` (a `gh run rerun` of release.yml will NOT re-fire repo-publish). 3. If release.yml succeeded on a final tag but repo-publish still didn't fire, confirm `release.yml`'s `name:` field is still `Release` (verbatim match required by `workflow_run`), then fall back to `gh workflow run repo-publish.yml -f tag=vX.Y.Z`. |
| `rclone sync` step fails with 403 | 1. R2 credentials rotated. Refresh `R2_ACCESS_KEY_ID` / `R2_SECRET_ACCESS_KEY` from Cloudflare (see `docs/pkg-repo-runbook.md` §2) and update the GitHub secrets. 2. Re-run the workflow. 3. If the bucket itself was deleted or renamed, restore from the rclone `--dry-run` diff before re-syncing - never blast-write an empty local staging dir to a bucket that still has user-visible content. |
| `InRelease` returns stale date | 1. Cloudflare edge cache - invalidate via the dashboard (or wait 60s for the default TTL). 2. Check whether `reprepro` actually ran by inspecting the workflow log for the "signing Release file" step. 3. If reprepro ran but the InRelease has wrong dists/version, the repo DB on R2 may be corrupted; rebuild from scratch per `docs/pkg-repo-runbook.md` §Bootstrap. |

---

## Step 6 - Smoke-test install in containers (≈ 4 min)

CI already runs `smoke-test-ubuntu` and `smoke-test-fedora` jobs
against the built artifacts. This step exercises the **published**
release from a user's perspective - "does a fresh `apt install` off
`pkg.paneflow.dev` actually work?"

> **Trust-on-first-use on the Docker base images.** The `ubuntu:22.04`
> and `fedora:40` tags below are mutable - `docker run` pulls whatever
> Docker Hub currently serves under those names. If you need
> reproducibility across releases (e.g., when diagnosing a failure that
> only reproduces against a specific base-image build), pin by digest:
> `docker run ubuntu@sha256:<hash> ...`. For routine smoke tests, the
> mutable tag is fine - we're validating our packages, not Ubuntu.

```bash
# Ubuntu 22.04 - apt-repo path
docker run --rm -it ubuntu:22.04 bash -c '
  set -euo pipefail
  apt-get update -qq
  apt-get install -y --no-install-recommends ca-certificates curl
  curl -fsSL https://pkg.paneflow.dev/gpg \
    > /usr/share/keyrings/paneflow-archive.asc
  echo "deb [signed-by=/usr/share/keyrings/paneflow-archive.asc] https://pkg.paneflow.dev/apt stable main" \
    > /etc/apt/sources.list.d/paneflow.list
  apt-get update
  apt-get install -y paneflow
  paneflow --version
'

# Fedora 40 - dnf-repo path
docker run --rm -it fedora:40 bash -c '
  set -euo pipefail
  dnf install -y dnf-plugins-core
  dnf config-manager --add-repo https://pkg.paneflow.dev/rpm/paneflow.repo
  rpm --import https://pkg.paneflow.dev/gpg
  dnf install -y paneflow
  paneflow --version
'
```

Both commands must exit 0 with the correct version string. A failure
here is the "Cursor regression" scenario - the release is live but
users can't install it.

### Troubleshooting - Step 6

| Symptom | Top 3 recoveries |
|---|---|
| `apt-get install` fails with `Hash Sum mismatch` | 1. Cloudflare edge still serving a stale InRelease or Packages file. Wait 60 s for TTL or purge cache manually via the Cloudflare dashboard. 2. Inside the container, force a no-cache refresh: `apt-get update -o Acquire::Check-Valid-Until=false`. 3. If the hash mismatch persists after 5 min, the reprepro build is inconsistent - re-run Step 5. |
| `dnf install` fails with `Errors during downloading metadata` | 1. `.repo` file points at the wrong base URL - cross-check against `.github/workflows/repo-publish.yml`'s published path. 2. `gpgkey=` URL 404s - confirm `https://pkg.paneflow.dev/gpg` is reachable from outside the container (`curl -I`). 3. RPM repo metadata in R2 lacks a matching signature - fix is the same as InRelease staleness above. |
| `paneflow --version` prints an unexpected version string | 1. The apt/dnf cache returned an older package. Purge and re-install: `apt remove paneflow && apt update && apt install paneflow`. 2. Two concurrent release workflows overwrote each other - check the repo-publish.yml concurrency group and make sure the most recent one actually ran. 3. Release was cut from the wrong branch/commit; abandon and re-cut at a patch bump. |

---

## Pre-announce - aarch64 on-device validation (conditional)

Only execute for releases where aarch64 users are expected (i.e.,
most public releases). Follow
[`docs/validation-aarch64.md`](./validation-aarch64.md) end-to-end on
at least one aarch64 device (RPi 5 or Asahi Linux). Attach evidence
(asciinema cast + screenshot) to the release notes per that runbook's
§5.

If validation fails, take the escape hatch documented in
`docs/validation-aarch64.md` §6 (remove the aarch64 assets from the
release, or hold as a pre-release) BEFORE announcing in Step 7 - an
announcement that links to broken aarch64 bits is the worst
failure mode possible for ARM users.

---

## Step 7 - Announce (≈ 2 min, manual judgement required)

Write the release notes on the GitHub Release page. `release.yml` sets
`generate_release_notes: true`, so GitHub has pre-filled the changelog
from merged PRs since the previous tag - your job is to polish that
default, not write it from scratch.

Suggested structure:

```markdown
## Highlights

- One-sentence summary of the biggest user-visible change.
- Second highlight.

## Install

See the [Installation section in the README](https://github.com/ArthurDEV44/paneflow#install).

## Validation

- x86_64 smoke tests: ✅ (CI: link to workflow run)
- aarch64 on-device: ✅ <distro / date / maintainer> OR ⏳ pending
  (see pre-release policy in `docs/validation-aarch64.md`)

## Full changelog

{the auto-generated list GitHub prepended}
```

**Manual judgement:** read the auto-generated changelog end-to-end.
Re-order items so user-visible changes come first (refactors and
chores go last), and drop anything that's pure-noise (dependabot bumps
that users don't need to know about).

Post-announce:

- Share the release link in whatever channels matter (GitHub
  Discussions, Discord, Mastodon, Reddit). Keep the post short - "new
  release, highlights, install link" - not a wall of text.
- Update the memory note
  `~/.claude/projects/-home-arthur-dev-paneflow/memory/research_linux_packaging.md`
  if the release changed the current-release-state lines (version,
  tag list).

### Troubleshooting - Step 7

| Symptom | Top 3 recoveries |
|---|---|
| Auto-generated release notes are empty | 1. No PRs merged since the previous tag - the notes generator has nothing to list. Write a manual entry explaining the release (direct-commit patch release, dependency-only bump, etc.). 2. The previous tag used a different naming scheme (e.g., missing `v` prefix) and the generator didn't find it - manually supply `--generate-notes --notes-start-tag=<previous-tag>` via `gh release edit`. 3. GitHub outage on the notes generator - retry in 10 min or fall back to `git log --oneline <prev-tag>..vX.Y.Z`. |
| Announced, then discovered a critical bug | 1. Flip the release to pre-release: `gh release edit vX.Y.Z --prerelease`. Users on `latest` fall back to the previous stable. 2. Pin a known-issue note at the top of the release notes and link a tracking issue. 3. Cut a patch release as quickly as feasible - the repo-publish workflow updates the apt/dnf streams automatically, so `apt upgrade` fixes most users without manual intervention. |
| Forgot to promote an `-rc.N` tag to the final release | 1. Run through Steps 0-5 with the non-rc tag - the workflow will produce a fresh set of artifacts. 2. Don't delete the `-rc.N` release; it stays as a historical pre-release record. 3. Update the "latest" link consumers by making sure the new final release is marked as `latest` (`gh release edit vX.Y.Z --latest`). |

---

## Self-update on rpm/deb (v0.2.3+)

Users on the signed `pkg.paneflow.dev` rpm or deb repo update PaneFlow
by clicking the "Update available" pill in the title bar. The dispatcher
in `src-app/src/app/self_update_flow.rs` routes the click through a
`pkexec`-elevated `dnf install paneflow-<ver>` or `apt-get install
paneflow=<ver>-1` subprocess. This section documents what the user sees,
the full fallback matrix, and the pre-release acceptance checklist
(run only for releases that touch the self-update flow).

### User-visible sequence (happy path)

1. User clicks the "Update available" pill.
2. ~200 ms later, the system polkit agent shows its native
   authentication dialog ("Authentication is required to install
   software packages"). The dialog is not branded PaneFlow - it is the
   system-supplied UI (GNOME / KDE / whichever is installed).
3. User types their password (or uses `fprintd` fingerprint).
4. Pill transitions `Downloading…` → `Installing…` while `dnf` / `apt`
   runs. The session is persisted before the restart.
5. PaneFlow re-execs `/usr/bin/paneflow` (now the new version via
   GPUI's launcher pattern - detached bash script that waits for our
   PID to exit before exec'ing the new binary).
6. Workspaces, layouts, and CWDs are restored from `session.json`.

### Fallback matrix

| Condition | Behaviour |
|---|---|
| `pkexec` missing from `$PATH` | Clipboard-copy toast fallback ("Copied: sudo dnf upgrade paneflow-X.Y.Z"). No retry-counter bump. (Note: the spawn uses the hardcoded `/usr/bin/pkexec`, not `which::which`, so a PATH-shadowed pkexec in `~/bin/` cannot redirect the exec - US-001 security fix.) |
| Polkit auth cancelled (pkexec exit 126) | Neutral toast "Update cancelled". Pill returns to idle. No retry-counter bump (user intent, not a failure). |
| Package manager non-zero exit (mirror error, conflict, network loss, etc.) | `record_update_failure("dnf" / "apt", err, cx)` - increments the 3-strike counter, shows retry toast. After 3 failures the 4th click routes the user to the releases page. |
| `dnf` lock held (`/run/dnf/rpmtransaction.lock` present - e.g. `dnf-automatic.timer` firing) | Toast "Package manager is busy - try again in a moment." No retry-counter bump. Lock-owner PID (when available) in the anyhow diagnostic context, never in the toast. |
| `apt`/`dpkg` lock held (matching `/proc/{pid}/comm` among `apt`, `apt-get`, `apt.systemd.da`, `dpkg`, `unattended-upgr`) | Same backpressure toast as dnf. |
| `/run/ostree-booted` present (Silverblue / Kinoite / Bazzite) | Informational toast + clipboard copy of `rpm-ostree upgrade`. No subprocess spawn, no restart (rpm-ostree stages for next reboot). |
| `PackageManager::Other` (Solus, Void, NixOS, arbitrary distros) | Clipboard-copy fallback with a generic "Update PaneFlow via your system's package manager" message. |
| Version string from `UpdateStatus::Available` fails `^v?\d+\.\d+\.\d+$` | Dispatcher refuses up-front with "Update unavailable - invalid release tag". Defence against a compromised GitHub tag. |

Stderr from the pkg-mgr is captured and emitted at `log::debug!` only
on a non-zero exit that is *not* 126. The 1 MB stderr buffer cap
prevents a pathological transaction from exhausting heap.

### Pre-release acceptance checklist

Run this matrix pre-release whenever the self-update dispatcher or
runner (`src-app/src/app/self_update_flow.rs`,
`src-app/src/update/linux/system_package.rs`) changed since the
previous release. Skip for pure non-self-update releases.

Target VMs: **Fedora 41** (dnf) + **Ubuntu 24.04** (apt). Each
scenario must produce the expected outcome.

| # | Scenario | Fedora (dnf) | Ubuntu (apt) | Expected |
|---|---|---|---|---|
| 1 | Happy path - newer version in signed repo, fast mirror | ✓ | ✓ | Click → polkit prompt → 15-30 s → restart with session intact |
| 2 | Cancel polkit (click "Cancel" on auth dialog) | ✓ | ✓ | "Update cancelled" toast, pill idle, no retry bump |
| 3 | Wrong password 3× in polkit dialog | ✓ | ✓ | polkit dismisses, pkexec returns 127 → clipboard-copy fallback toast |
| 4 | `pkexec` removed from `$PATH` (`sudo mv /usr/bin/pkexec /tmp/`) | ✓ | ✓ | Clipboard-copy fallback toast, no retry bump. Restore with `sudo mv /tmp/pkexec /usr/bin/`. |
| 5 | Mirror returns 404 for the pinned version (simulate by `sudo dnf config-manager --disable paneflow` then click) | ✓ | ✓ | `InstallFailed` toast, retry counter bumps |
| 6 | `dnf-automatic` holds lock (`sudo systemctl start dnf-automatic-install.timer` then click within the install window) | ✓ | n/a | "Package manager is busy - try again in a moment." No retry bump. |
| 7 | Concurrent `sudo apt install -y htop` in another terminal | n/a | ✓ | Same backpressure toast for apt. No retry bump. |
| 8 | Offline mid-install (disconnect network during `Installing…`) | ✓ | ✓ | `InstallFailed` toast, retry counter bumps |
| 9 | Silverblue (`touch /run/ostree-booted` in a throwaway container that looks like a rpm install) | ✓ (simulated) | n/a | Informational toast with `rpm-ostree upgrade` in clipboard. No subprocess spawn. |
| 10 | Workspace with 6 panes + running shells survives the restart | ✓ | ✓ | All 6 panes restored with correct CWDs, prompts intact |

Scenarios 11-13 below only apply when the release includes EP-002
changes (install-method hygiene - stale user-local icon cleanup +
dual-install coexistence advisory). Skip them for EP-001-only
releases.

| # | Scenario | Fedora (dnf) | Ubuntu (apt) | Expected |
|---|---|---|---|---|
| 11 | EP-002: host has stale `~/.local/share/icons/hicolor/*/apps/paneflow.png` from a prior tar.gz install, then user switches to the fresh rpm install | ✓ | n/a | Single login shows the new icon everywhere (dock, Activities, Alt+Tab); stale user-local files and orphaned icon cache entries removed; migration marker file written. |
| 12 | EP-002: host has BOTH `/usr/bin/paneflow` (system) AND `~/.local/paneflow.app/bin/paneflow` (tar.gz) concurrently | ✓ | ✓ | One advisory toast on first launch after detection; marker prevents the toast recurring; deleting the marker re-shows the toast on next launch. |
| 13 | EP-002: fresh rpm install on a clean host with no tar.gz history and no user-local icons | ✓ | ✓ | Migration runs as a no-op; marker written; zero log noise above `info!`; zero filesystem writes beyond the marker. |

Mark each scenario pass/fail with a one-line note if it fails. A
single failure among scenarios that apply is a release blocker - do
not tag `vX.Y.Z` until the matrix is fully green for the relevant
epic(s).

### Troubleshooting - Self-update

| Symptom | Top 3 recoveries |
|---|---|
| Polkit dialog never appears after clicking pill | 1. Check that a polkit agent is running in the session: `ps -ef \| grep -E 'polkit-(gnome\|mate\|kde)-authentication-agent' \| grep -v grep`. On bare Sway/Hyprland without an agent, pkexec returns 127 and the clipboard fallback fires - this is correct behaviour. 2. Start an agent for the session: `/usr/libexec/polkit-gnome-authentication-agent-1 &`. 3. If `gdm`/`sddm` is running, the agent should auto-start with the session - check `journalctl --user -u polkit` for errors. |
| "Update cancelled" toast fires without the user cancelling | 1. Pkexec exit 126 can also mean "authorization refused by policy" - inspect `journalctl -u polkit` for a rule that denies the action. 2. The `org.freedesktop.policykit.exec` action should be unrestricted for admin users by default; packaged polkit rules in `/etc/polkit-1/rules.d/` can override that. 3. If the user's gnome-keyring or KDE wallet is locked, polkit may reject non-interactive retries - log out and back in. |
| Update succeeds but restart never happens | 1. `session.save` may have failed - check `~/.cache/paneflow/` for a fresh `session.json`. 2. GPUI's launcher script may have lost track of our PID - this surfaces as "restarting into …" in the log but no new process. File a bug with the `~/.local/share/paneflow/logs/` attachment. 3. On Wayland, the compositor may reject the restart's window-open request if the original window was closed before the restart fired - reproduce with DRI_PRIME=0 to rule out GPU-handshake issues. |
| Backpressure toast fires but no `dnf-automatic` / `apt-get` is running | 1. Check the probe manually: `ls -la /run/dnf/rpmtransaction.lock` (dnf) or `pgrep -a -f 'dpkg\|apt-get\|unattended-upgr'` (apt). 2. A crashed libdnf5 may leave the lock file on disk; on Fedora 41+, `/run` is a tmpfs by default so it clears on reboot. On non-default layouts, delete the stale file manually (`sudo rm /run/dnf/rpmtransaction.lock`) after confirming no dnf process holds it. 3. On apt, a process with a spoofed `comm` name (`exec -a dpkg sleep 3600`) would false-positive - this is an accepted limitation (self-DoS, not privilege escalation). |

---

## Troubleshooting - two PaneFlow installs detected (US-009)

PaneFlow v0.2.3+ surfaces a one-time advisory toast at startup when it
detects that both flavours of the app are installed on the same host -
typically the rpm/deb system package at `/usr/bin/paneflow` AND a
left-over tar.gz install at `~/.local/paneflow.app/bin/paneflow`. The
toast names the running binary and the other install, then writes a
marker at `~/.cache/paneflow/migration-v0.2.3-coexistence-warned` so it
never fires twice.

### Cleaning up the unused install

- **Keeping the rpm/deb (recommended)** - remove the tar.gz tree:
  ```bash
  rm -rf ~/.local/paneflow.app
  # If a ~/.local/bin/paneflow symlink survives, drop it too:
  rm -f ~/.local/bin/paneflow
  ```

- **Keeping the tar.gz** - remove the system package. On
  Fedora / RHEL / Rocky: `sudo dnf remove paneflow`. On openSUSE:
  `sudo zypper remove paneflow`. On Ubuntu / Debian / Mint:
  `sudo apt remove paneflow`. These commands also remove
  `/usr/share/applications/paneflow.desktop` and the hicolor icons
  under `/usr/share/icons/hicolor/*/apps/`, so the tar.gz install
  becomes the only registered app again after the next
  `gtk-update-icon-cache` run.

### Keeping both installs intentionally

Users running a dev build alongside a stable system install
(common for contributors) should keep
`~/.cache/paneflow/migration-v0.2.3-coexistence-warned` in place -
its presence silences the toast indefinitely. Delete the marker
only if you cleaned up and want PaneFlow to re-check on next
launch.

### Symptom reference

| Symptom | Likely cause / next step |
|---|---|
| Toast fires every session after cleanup | Marker write failed on the session the toast first appeared (permissions on `~/.cache/paneflow/` or read-only tmpfs). Verify the directory is writable; `touch ~/.cache/paneflow/migration-v0.2.3-coexistence-warned` to silence it manually. |
| Toast never fires even though two installs exist | Check `~/.cache/paneflow/migration-v0.2.3-coexistence-warned`; delete it to re-trigger. Also confirm `$HOME` is set in the launching environment - the detection bails when it's missing. |
| Toast fires on a fresh host with only one install | Symlink farms (e.g., `~/.local/paneflow.app/bin/paneflow` pointing at `/usr/bin/paneflow`) satisfy `Path::exists()` and trip the detection. Remove the symlink or keep the marker. |

---

## Dry-run validation (AC6 - once per runbook revision)

This runbook is considered validated when a maintainer has executed
it end-to-end at least once for a real release AND the three Phase 1
formats (plus the Phase 2 `.rpm`) installed cleanly in fresh
containers during Step 6. The first execution should treat any
friction as a bug in the runbook, not in the maintainer - open a PR
to fix the step that went wrong.

Track dry-run validation at the top of the runbook ("Last validated
on: vX.Y.Z, YYYY-MM-DD") - keep that line updated every release so a
maintainer returning after a long break knows the runbook still
works.

Last validated on: _unknown - update after the next dry run with tag,
date, workflow run, and smoke-test evidence_.

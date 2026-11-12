# Paneflow release runbook

The mechanical shell blocks behind a release. `.claude/skills/release/SKILL.md`
sequences them, owns the notes and the site, and keeps every recovery move in
`.claude/skills/release/references/red.md`. This file holds only what is
pasted into a shell. The per-platform signing and engine runbooks sit beside
it in this directory.

Prerequisites, once: `gh auth status` reports `repo` scope, the GitHub
secrets named in `keys/README.md` are populated, and a container runtime
(`podman` or `docker`) is installed for Step 6.

## Supported release targets

Cross-reference `.github/workflows/release-linux.yml`,
`release-macos.yml` and `release-windows.yml`, one per platform.

| Target | Status | Ships | Gate |
|---|---|---|---|
| `x86_64-unknown-linux-gnu` | Active | .deb, .rpm, AppImage, .tar.gz | Hard-required |
| `aarch64-unknown-linux-gnu` | Active | .deb, .rpm, AppImage, .tar.gz | Hard-required |
| `aarch64-apple-darwin` | Active | .dmg | Hard-required |
| `x86_64-pc-windows-msvc` | Active | signed .msi | Hard-required |
| `x86_64-apple-darwin` | Closed | | Reopen only with Intel DMG signing and a widened Homebrew cask |
| `aarch64-pc-windows-msvc` | Closed | | Reopen only with GPUI DX11 ARM64 reliability, signing, and runner coverage |

Hard-required means a failure blocks the release: the
`Publish GitHub Release` job waits on every leg. Do not re-add a closed
target as a best-effort leg; reopening one needs an artifact path, a signing
path, a docs update, and a release-gate decision in the same change.

## The `release` environment

Every signing and publishing secret (`APPLE_*`, `AZURE_*`, `GPG_*`, `R2_*`,
`CLOUDFLARE_*`, `HOMEBREW_TAP_DEPLOY_KEY`, and `MINISIGN_SECRET_KEY`) belongs
to the `release` environment, which only the branch `main` and the tags `v*`
may deploy to. The jobs that consume them declare the environment: `build`
in each of `release-linux.yml`, `release-macos.yml` and
`release-windows.yml`, `auto-update-e2e` in `release-linux.yml`, `release`
in `release.yml`, `publish` in `repo_publish.yml`, `update-cask` in
`update_cask.yml`. `build` declares it
only for a `v*` tag push, or for a dispatch from `main` whose `target_ref` is
`main`. The deployment policy checks the dispatch ref, not the `target_ref`,
so this second condition is what stops a dispatch from `main` from signing a
branch. Any other `release.yml` run (a dispatch from another branch, from a
tag, or with another `target_ref`) has no environment, gets no signing secret,
builds unsigned dry-run artifacts, skips the Windows MSI smokes and the
auto-update e2e, and never reaches publication.

`scripts/configure-release-environment.sh` holds the exact `gh api` calls.
Apply the move once, in this order:

```bash
bash scripts/configure-release-environment.sh status
bash scripts/configure-release-environment.sh policy
```

`status` is read-only: it prints the deployment policy and, for each secret,
whether it lives in the repository, the environment, or both, followed by the
remaining commands. Recreate each listed secret in the environment with the
printed `gh secret set NAME --repo ... --env release`, which reads the value
from stdin (`< file` for the P12 and the armored GPG key). Then:

1. Dispatch `release.yml` from `main` and check that every leg signs with the
   environment copies.
2. Delete the repository copies with the `gh secret delete` commands `status`
   prints. Until they are gone, a job with no environment still receives them,
   so a branch dry-run would still sign.
3. Dispatch `release.yml` from a scratch branch, for example
   `gh workflow run release.yml --ref <branch> -f version=0.0.0-dryrun.1`, and
   check that the build legs warn `Signing secrets unavailable on <ref>` and
   that no job fails on the deployment policy.
4. Dispatch `repo_publish.yml` from that branch and check that GitHub refuses
   the `release` deployment. Run it only after `policy`: without the policy,
   this dispatch publishes.

The secrets only the publishing jobs read (`R2_*`, `CLOUDFLARE_*`,
`HOMEBREW_TAP_DEPLOY_KEY`, `MINISIGN_SECRET_KEY`) are first exercised by the
next real release, so watch that release's `repo_publish` and `update_cask`
runs.

## Draft, digests, attestation, and immutable releases

`Publish GitHub Release` never exposes a release before it is complete:

1. `scripts/require-unpublished-release.sh` fails when the tag's release is
   already published, so a re-run never replaces a published asset. A draft
   left by an earlier attempt is replaced.
2. Every asset is signed with minisign, then uploaded to a draft release.
3. `scripts/verify-release-digests.sh` compares the `digest` GitHub computed
   for each uploaded asset with the SHA-256 of the file the run built, and the
   two asset sets. On a mismatch the job fails and the draft stays for
   inspection: `gh release view "v$VERSION" --json assets`, then
   `gh release delete "v$VERSION" --yes`, which keeps the tag, and re-run the
   failed jobs.
4. `actions/attest-build-provenance` attests every asset. Users verify a
   download with `gh attestation verify <file> --repo arthjean/paneflow
   --signer-workflow arthjean/paneflow/.github/workflows/release.yml`.
5. Only then is the draft published.

Immutable releases make step 5 final: once published, a release's assets
and its tag can no longer be changed or deleted. Arthur enables it once:

```bash
gh api -X PUT repos/arthjean/paneflow/immutable-releases
gh api repos/arthjean/paneflow/immutable-releases
```

The second command prints `{"enabled":true,...}`. The title and the notes of
a published release stay editable, so the curated release note is still
edited after publication. From then on, a tag is
deleted and re-created at a fix commit only while its release is still a
draft or absent. A problem found after publication ships as a patch version.

## Step 1 - Bump and commit

Work on `main`, clean and current, with every change for the release
already merged. Run from the repository root: the `sed` and the changelog
writes use relative paths.

```bash
cd "$(git rev-parse --show-toplevel)"
VERSION="X.Y.Z"
[[ "$VERSION" =~ ^[0-9]+\.[0-9]+\.[0-9]+(-(rc|alpha|beta)\.[0-9]+)?$ ]] \
  || { echo "invalid VERSION='$VERSION'"; return 1 2>/dev/null || exit 1; }
TODAY="$(date -u +%F)"

sed -i "s/^version = \".*\"$/version = \"$VERSION\"/" Cargo.toml

cat > /tmp/paneflow-changelog-entry.txt <<EOF
paneflow ($VERSION-1) stable; urgency=medium

  * Release v$VERSION. See GitHub release notes for the full diff.

 -- Arthur Jean <arthur.jean@strivex.fr>  $(LC_TIME=C date -R)

EOF
cat /tmp/paneflow-changelog-entry.txt debian/changelog > debian/changelog.new
mv debian/changelog.new debian/changelog

cargo check --workspace --offline
```

Only the workspace root `Cargo.toml` carries a literal `version`; every
crate inherits it through `version.workspace = true`. The Debian stanza
format is strict: `name (VERSION-REVISION) DISTRIBUTION; urgency=LEVEL`, and
the trailer needs two spaces before an RFC 2822 date in the C locale.

Three edits the block does not make:

- `CHANGELOG.md`: rename `## [Unreleased]` to `## [X.Y.Z] - YYYY-MM-DD` and
  open a fresh empty `## [Unreleased]` above it.
- `assets/io.github.arthurdev44.paneflow.metainfo.xml`: prepend a
  `<release version="X.Y.Z" date="YYYY-MM-DD">` entry under `<releases>`,
  with a one-paragraph summary and a short `<ul>`. The
  `Validate AppStream metainfo` step of `release-linux.yml` runs
  `appstreamcli validate` on every leg, so validate it locally first when
  `appstreamcli` is installed.
- The GitHub release notes, written by the skill outside the repository.

Then gate, commit, and push:

```bash
cargo fmt --check
cargo clippy --workspace --all-targets --locked -- -D warnings
cargo test --workspace --locked
git add Cargo.toml Cargo.lock debian/changelog CHANGELOG.md \
  assets/io.github.arthurdev44.paneflow.metainfo.xml
git commit -m "chore: bump version to v$VERSION"
git push origin main
```

Read the test output, not just the exit code: `cargo test` exits 0 on an
ignored test.

## Step 2 - Tag and push

`release.yml` publishes only a commit on which every required `run_tests`
job is `success`: Style, Tests (Linux x86_64), Linux aarch64, macOS aarch64
smoke build, Windows x86_64 libghostty check, Performance gates and the
cargo-deny audit (the `ci-verdict` job, `scripts/require-ci-verdict.sh`). The
push of Step 1 runs them, so wait for `run_tests` on the commit you are about
to tag before tagging it:

```bash
gh run watch --exit-status "$(gh run list --workflow run_tests.yml --commit "$(git rev-parse HEAD)" \
  --limit 1 --json databaseId --jq '.[0].databaseId')"
```

Before tagging a minor version (`X.Y.0`), when the version changes rendering
(GPUI elements, animations, the sidebar, tabs, the terminal painter), the real
hardware protocol of
[bench/README.md](../../bench/README.md#real-hardware-protocol) must also have
been run on this commit under Linux against the previous minor version, with
its results and summary committed. No CI gate covers it.

```bash
git tag -a "v$VERSION" -m "Release v$VERSION"
git push origin "v$VERSION"
```

A tag with `-rc.N`, `-alpha.N`, or `-beta.N` publishes as a prerelease and
skips `repo_publish` and `update_cask` by design.

## Step 3 - Watch the chain

`gh run list --branch` does not match tag refs, so filter the tag-triggered
run by event.

The release resolves the tagged commit once, in `preflight`, and fails there
within a minute when the tag is not `v` followed by the `Cargo.toml` version.
`ci-verdict` runs beside the builds and waits, polling every 60 s for up to
60 min, while `run_tests` is still running on that commit. It fails with the
job name, its URL and the command to run when a required job is red, canceled
or skipped, which happens when a path filter skipped the Rust jobs:

```bash
gh workflow run run_tests.yml --ref "v$VERSION"
```

Once that run is green, re-run the failed jobs of the release run. The
publish job also refuses to run if the tag was moved to another commit while
the release was in flight.

```bash
gh run watch --exit-status "$(gh run list --workflow=release.yml --event=push --limit=1 --json databaseId --jq '.[0].databaseId')"
gh run watch --exit-status "$(gh run list --workflow=repo_publish.yml --limit=1 --json databaseId --jq '.[0].databaseId')"
gh run watch --exit-status "$(gh run list --workflow=update_cask.yml --limit=1 --json databaseId --jq '.[0].databaseId')"
```

Both chained workflows gate their only job behind an `if:` guard, and a
skipped job still reports the run as successful. Confirm the job ran:

```bash
gh run view "$RUN_ID" --json jobs --jq '.jobs[] | "\(.name): \(.conclusion)"'
```

`Dry-run artifact summary` is skipped on a tag push; that is expected.
Read the annotations of a green `release` run: a `::warning::` on signing,
`lintian`, or a GPG fingerprint is not noise, and neither is a `ci-verdict`
failure, which no re-run of the build legs fixes. The release declares
`cache-mode: none`, so no cache step runs and none can annotate the run.

## Step 4 - Verify the asset set

```bash
gh release view "v$VERSION" --json isDraft,isPrerelease,assets \
  --jq '{draft:.isDraft, pre:.isPrerelease, assets:(.assets|length)}'
gh api repos/arthjean/paneflow/releases/latest --jq .tag_name
gh release view "v$VERSION" --json assets --jq '.assets[].name' | sort
```

Primary artifacts, each with a `.sha256` sidecar and a `.minisig`
signature, each AppImage also with an `.AppImage.zsync`:

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

The in-app updater matches assets by their `-<arch>.<format>` suffix
(`src-app/src/update/checker.rs`), so a renamed or missing asset breaks
every user's upgrade. The workflow's own verification step blocks
publication on a mismatch; the count here is the second check.

## Step 5 - Verify the published streams

```bash
bash scripts/verify-published-streams.sh "$VERSION"
```

This is the script that `repo_publish.yml` (apt and rpm) and
`update_cask.yml` (cask) run after they publish. It reads every file with a
cache-busting query, so the Cloudflare edge cache cannot hide a stale stream,
and checks:

- apt: `InRelease` verifies against `packaging/paneflow-release.asc`, each
  `binary-<arch>/Packages.gz` matches the SHA-256 that the signed `InRelease`
  lists, and both architectures list `paneflow X.Y.Z-1`;
- rpm: `repomd.xml.asc` verifies, the primary metadata matches the SHA-256 of
  the signed `repomd.xml`, and it lists `paneflow X.Y.Z` for `x86_64` and
  `aarch64`;
- cask: `Casks/paneflow.rb` in `arthjean/homebrew-paneflow` declares
  `version "X.Y.Z"` and a real `sha256`, read through the GitHub API with
  `GH_TOKEN` or the `gh` login.

A stream still stale after 5 attempts 60 s apart fails with its name. Pass
`--stream apt`, `--stream rpm` or `--stream cask` to check a single stream;
`PANEFLOW_VERIFY_ATTEMPTS` and `PANEFLOW_VERIFY_INTERVAL_SECONDS` change the
retry budget. The script refuses a prerelease version: the chained runs never
publish one, and a prerelease dispatched to `repo_publish.yml` by hand is
checked by hand.

## Step 6 - Install from a user's position

CI already smoke-tests the built artifacts. This step installs the
published packages off `pkg.paneflow.dev`. The `ubuntu:22.04` and
`fedora:40` tags are mutable; pin by digest only when reproducing a
failure against a specific base image.

```bash
R="$(command -v podman || command -v docker)"

"$R" run --rm ubuntu:22.04 bash -c '
  set -euo pipefail
  apt-get update -qq
  apt-get install -y --no-install-recommends ca-certificates curl
  curl -fsSL https://pkg.paneflow.dev/gpg > /usr/share/keyrings/paneflow-archive.asc
  echo "deb [signed-by=/usr/share/keyrings/paneflow-archive.asc] https://pkg.paneflow.dev/apt stable main" \
    > /etc/apt/sources.list.d/paneflow.list
  apt-get update
  apt-get install -y paneflow
  paneflow --version
'

"$R" run --rm fedora:40 bash -c '
  set -euo pipefail
  cat > /etc/yum.repos.d/paneflow.repo <<EOF
[paneflow]
name=Paneflow
baseurl=https://pkg.paneflow.dev/rpm
enabled=1
gpgcheck=1
repo_gpgcheck=1
gpgkey=https://pkg.paneflow.dev/gpg
EOF
  rpm --import https://pkg.paneflow.dev/gpg
  dnf install -y paneflow
  paneflow --version
'
```

There is no hosted `paneflow.repo` descriptor: the RPM's own `%post`
(`packaging/rpm/postinst.sh`) writes `/etc/yum.repos.d/paneflow.repo` after
a first install from a GitHub Release, which is why the Fedora block writes
it by hand. Both commands must exit 0 and print `paneflow X.Y.Z`.

## Workflow layout

`release.yml` is the orchestrator and the only file that triggers. It holds
`preflight`, `ci-verdict`, one call per platform, the Linux package smoke
matrix, the dry-run summary and the publish job. The builds live in three
reusable workflows, which nothing else calls:

| File | Jobs | Secrets it receives |
|---|---|---|
| `release-linux.yml` | `build` (x86_64 and aarch64), `auto-update-e2e` | `POSTHOG_API_KEY`, `GPG_PRIVATE_KEY`, `GPG_PASSPHRASE`, `GPG_KEY_ID`, `MINISIGN_SECRET_KEY` |
| `release-macos.yml` | `build` | `POSTHOG_API_KEY`, the five `APPLE_*` secrets |
| `release-windows.yml` | `build`, `smoke-test-windows-msi`, `smoke-test-windows-msi-live-host` | `POSTHOG_API_KEY`, the six `AZURE_*` secrets |

The orchestrator maps each secret by name and never passes `secrets: inherit`,
so a platform never sees the keys of another. The mapping only names the
secret: the value a signing job reads is the one of the `release` environment
it declares. A called workflow gets the tag, the mode, the commit and the
`signed` flag as inputs, because the `env` of the caller does not reach it,
and it may not ask for more permissions than the orchestrator grants: GitHub
refuses to start such a run.

In the run view a called job reads `Windows / Build x86_64-pc-windows-msvc`.
When one platform fails, "Re-run failed jobs" re-runs the failed jobs of that
platform and the jobs that depend on them, and keeps the artifacts of the two
others. `scripts/check-workflows.sh` holds the structure: `release.yml` stays
at 400 lines or fewer, no call inherits secrets, no platform workflow tests
`runner.os`, and all four files declare `cache-mode: none`.

## Release scripts

The release workflows hold no logic of their own: each step calls a script under
`scripts/`, so a step can be run and debugged from a checkout. Run every
script from the repository root. The bash scripts print their usage when
called with the wrong number of arguments; the PowerShell scripts need
PowerShell 7 and take named parameters. A secret is always read from the
environment, never from an argument. `RELEASE_MODE` is `release` unless it is
set to `dry-run`.

`scripts/test-ci-scripts.sh` runs the scripts marked "fixtures" against
fixtures, on any machine.

### Linux legs (`release-linux.yml`)

The two Linux legs build in the `ubuntu:22.04` image on an `ubuntu-24.04`
host, so every binary and every library the AppImage bundles comes from
Ubuntu 22.04, the floor `packaging/debian/postinst` promises.

The container costs nothing measurable. `cargo build --release` took 13 min
22 s (x86_64) and 11 min 21 s (aarch64) on the `ubuntu-22.04` hosts of run
43070193687, and between 10 min 58 s and 13 min 40 s (x86_64) and between
10 min 48 s and 11 min 29 s (aarch64) in the container, over runs
43173633556, 43177326821 and 43182204818: at most 2 % slower. Preparing the
container adds about 10 s. In run 43182204818 `check-glibc-floor.sh` read
2.35 as the highest glibc symbol version of `paneflow` on both targets, and
2.34 for `paneflow-host` and the three helpers.

| Script | Arguments | Environment | What it does |
|---|---|---|---|
| `check-appstream-version.sh` | none | | Fails when the top `<release>` of the AppStream metainfo is not the workspace version |
| `check-glibc-floor.sh` (fixtures) | `<floor> <elf>...`, for example `2.35 target/<triple>/release/paneflow` | `OBJDUMP` to use another objdump | Fails and names each symbol that needs a glibc newer than the floor |
| `check-helper-sizes.sh` (fixtures) | `[helper-dir]` | | Enforces the size caps of the three embedded helpers |
| `sign-linux-packages.sh` (fixtures, unsigned paths) | `<triple>` | `GPG_PRIVATE_KEY`, `GPG_KEY_ID`, `GPG_PASSPHRASE`, `RELEASE_MODE` | Signs the `.deb` with `dpkg-sig` and the `.rpm` with `rpmsign`, then verifies both |
| `stage-linux-assets.sh` (fixtures) | `<version> <arch> <triple>` | | Copies the tarball, AppImage, `.deb` and `.rpm` into `release-assets/` under their release names, with a `.sha256` each |
| `smoke-linux-package.sh` | `<ubuntu\|debian\|fedora\|opensuse\|arch> <version> <asset-dir>` | | Installs the x86_64 package as the distribution does and checks what it wrote. It runs as root and edits `/etc`: use a throwaway container |
| `resolve-previous-release.sh` (fixtures) | `<current-tag>` | `GH_TOKEN` | Writes `old_tag` and `old_version`, the newest stable release other than the current tag, to `$GITHUB_OUTPUT` |
| `sign-update-e2e-tarball.sh` | `<new-asset-dir>` | `MINISIGN_SECRET_KEY` | Signs the new x86_64 tarball the way the publish job will |
| `run-update-e2e.sh` | `<new-asset-dir> <old-asset-dir>` | `OLD_VERSION` | Runs `test-update-e2e.sh` from the old tarball to the new one |

`sign-linux-packages.sh` keeps three constraints that cost a release each:
`GPG_KEY_ID` must be the 40-character fingerprint, because a short key id can
be forged; `rpmsign` reads `%__gpg /usr/bin/gpg` from `~/.rpmmacros`, because
Ubuntu's rpm otherwise calls a `gpg2` that does not exist; and the signature
is checked against a private rpm database (`rpm --dbpath`), because Ubuntu
ships no initialized system one. A release without the key fails with
"GPG signing failed: key not loaded"; only a dry-run skips the signature.

The package smoke on Ubuntu 22.04 runs `paneflow --version`: that is the run
that proves the binary starts on glibc 2.35.

### macOS leg (`release-macos.yml`)

| Script | Arguments | Environment | What it does |
|---|---|---|---|
| `check-xcode-metal.sh` | none | | Fails when the Command Line Tools, the SDK or the Metal compiler is missing |
| `detect-macos-signing.sh` (fixtures) | none | the five `APPLE_*` secrets, `RELEASE_MODE` | Writes `signing_available` to `$GITHUB_OUTPUT`; a release without the secrets fails |
| `summarize-unsigned-macos.sh` | `<arch> <tag>` | | Writes the unsigned-build notice to the job summary |
| `stage-macos-assets.sh` | `<version> <arch>` | | Copies the `.dmg` into `release-assets/` with its `.sha256` |

### Windows leg (`release-windows.yml`)

Load the MSVC environment first (`scripts/msvc-dev-env.ps1 -Arch x64`). The
MSI chain runs only when the six `AZURE_*` secrets are present: Paneflow
never ships an unsigned installer.

| Script | Arguments | Environment | What it does |
|---|---|---|---|
| `check-windows-build-tools.ps1` | none | | Fails when `cl.exe`, `link.exe` or `signtool.exe` cannot be found |
| `detect-windows-signing.sh` (fixtures) | none | the six `AZURE_*` secrets | Writes `signing_available` to `$GITHUB_OUTPUT` |
| `build-windows-helpers.ps1` | `-Target <triple>` | | Builds the three embedded helpers under `release-min`, with no signing secret in the environment |
| `stage-windows-helpers.ps1` | `-Target <triple>` | `AZURE_*` | Signs the helpers, copies them where `rust-embed` and WiX read them, writes `helpers_prepared` |
| `check-wix-helper-set.ps1` | `-Target <triple>` | | Fails when the WiX `HelperBinaries` list and the prepared helpers differ |
| `preflight-wix.ps1` | none | | Finds WiX v3 (`cargo-wix` 0.3.9 has no WiX v4 backend) and adds it to `$GITHUB_PATH` |
| `check-wix-per-machine.ps1` | none | | Fails when a per-machine MSI uses an HKCU key path outside a shortcut-only component |
| `sign-windows-executables.ps1` | none | `AZURE_*` | Signs `paneflow.exe` and `paneflow-host.exe` |
| `verify-windows-release-payload.ps1` | `-Target <triple>` | | Checks the static libghostty link and writes the provenance report |
| `build-msi.ps1` | `-Version <semver>` | | Runs `cargo wix --no-build --include packaging/wix/main.wxs`; `cargo wix` has no `build` subcommand |
| `sign-msi.ps1` | `-Tag <tag>` | `AZURE_*` | Signs the single MSI under `target/wix` |
| `verify-msi-signature.ps1` | `-Tag <tag>` | | Requires `signtool verify /pa` to print "Successfully verified" |
| `verify-windows-authenticode.ps1` | `-Target <triple>` | | Requires a valid Authenticode signature on every executable the MSI installs and on the MSI |
| `stage-windows-assets.sh` (fixtures) | `<version>` | | Copies the MSI and the provenance report into `release-assets/` |
| `summarize-skipped-msi.sh` | `<arch> <tag>` | | Writes the skipped-MSI notice to the job summary |
| `download-release-msi.ps1` | `-Tag <vX.Y.Z\|latest> -Directory <dir>` | `GH_TOKEN`, `GITHUB_REPOSITORY` | Downloads a published MSI and its `.sha256` |
| `smoke-msi.ps1` | `-Tag -MsiVersion -SmokeDir -BaselineDir` | `RELEASE_MODE` | Fresh install, upgrade from v0.8.0, update relay and uninstall of the MSI in `release-assets/` |
| `smoke-msi-live-host.ps1` | `-SmokeDir -BaselineDir` | | Upgrades the latest published release while its host runs |

The two MSI smokes install and remove Paneflow machine-wide. Run them on a
throwaway Windows machine, never on a workstation that has Paneflow
installed.

### Every leg, summary and publication

| Script | Arguments | Environment | What it does |
|---|---|---|---|
| `check-staged-assets.sh` (fixtures) | none | `RELEASE_MODE` | Fails when `release-assets/` is empty, except in a dry-run, where an unsigned leg stages nothing |
| `release-dry-run-summary.sh` (fixtures) | `<tag> <built-ref> <asset-dir>` | | Writes the table of assets, sizes and SHA-256 to the job summary |
| `require-tag-at-commit.sh` (fixtures) | `<tag> <sha>` | `GITHUB_REPOSITORY` | Fails when the tag no longer points at the commit the run built |
| `sign-release-assets.sh` | `<asset-dir>` | `MINISIGN_SECRET_KEY` | Writes a `.minisig` next to every asset |
| `install-minisign.sh` | `<install-dir>` | `GITHUB_PATH`, optional | Installs the pinned minisign as `<install-dir>/minisign`, checked against a pinned SHA-256, on Linux and macOS. `sign-release-assets.sh` and `sign-update-e2e-tarball.sh` call it themselves |

## Downstream packagers

A distribution package that must not self-update sets
`PANEFLOW_UPDATE_EXPLANATION` at build time. The in-app updater then treats
the install as externally managed: the title bar shows the update as
system-managed, and the update action copies this text to the clipboard and
shows it in a toast instead of downloading anything. The same variable set at
runtime overrides the built-in text for one launch.

```bash
PANEFLOW_UPDATE_EXPLANATION="Update Paneflow with your package manager: sudo pacman -Syu paneflow" \
  cargo build --release --locked -p paneflow-app
```

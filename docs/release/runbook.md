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

Cross-reference the matrix in `.github/workflows/release.yml`.

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
may deploy to. The jobs that consume them declare the environment: `build`,
`auto-update-e2e` and `release` in `release.yml`, `publish` in
`repo_publish.yml`, `update-cask` in `update_cask.yml`. `build` declares it
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
  `Validate AppStream metainfo` step of `release.yml` runs
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
Read the annotations of a green `release` run: `ENOENT` from the
`rust-cache` post step is noise, a `::warning::` on signing, `lintian`, or
a GPG fingerprint is not, and neither is a `ci-verdict` failure, which no
re-run of the build legs fixes.

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
curl --fail --silent https://pkg.paneflow.dev/apt/dists/stable/InRelease | grep -E '^Date:'
curl --fail --silent https://pkg.paneflow.dev/apt/dists/stable/main/binary-amd64/Packages | grep -m1 '^Version:'
curl --fail --silent https://pkg.paneflow.dev/rpm/repodata/repomd.xml | grep -m1 '<revision>'
gh api repos/arthurdev44/homebrew-paneflow/contents/Casks/paneflow.rb --jq .content \
  | base64 -d | grep -E '^\s*version'
```

The apt date is today's, the apt version is `X.Y.Z-1`, and the cask
version is `X.Y.Z`. A stale `InRelease` within the first minute is the
Cloudflare edge TTL, not a failed publish.

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

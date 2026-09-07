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

```bash
git tag -a "v$VERSION" -m "Release v$VERSION"
git push origin "v$VERSION"
```

A tag with `-rc.N`, `-alpha.N`, or `-beta.N` publishes as a prerelease and
skips `repo_publish` and `update_cask` by design.

## Step 3 - Watch the chain

`gh run list --branch` does not match tag refs, so filter the tag-triggered
run by event.

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
a GPG fingerprint is not.

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

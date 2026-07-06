# Linux signature verification (US-025)

End-user runbook for verifying that a downloaded Paneflow Linux artifact
(`.deb` / `.rpm` / `.tar.gz` / `.AppImage`) was genuinely produced by
the Paneflow release pipeline and has not been tampered with in
transit.

This document is **user-facing**. The maintainer runbook for generating,
rotating, and storing the signing key lives at `docs/release-signing.md`.

---

## 1. Trust model (read this first)

The Paneflow release key (`paneflow-release@paneflow.dev`) is published
in **two places**, by design:

1. **Repo-committed copy** - [`keys/paneflow-release.asc`](../../keys/paneflow-release.asc)
   inside this repository. Because it is part of the git history, it is
   covered by the maintainer's signed commit chain and cannot be
   silently swapped without a force-push to `main`.
2. **Server-served copy** - `https://pkg.paneflow.dev/gpg`, served from
   the Cloudflare R2 bucket alongside the apt and dnf repositories.

> **The repo-committed copy is the authoritative source.** Prefer it
> for first import. Then cross-check the fingerprint against the
> server-served copy - if they differ, **stop**: the package repository
> is compromised and any artifact downloaded from `pkg.paneflow.dev` is
> suspect until the discrepancy is resolved.

### Expected fingerprint (the trust anchor)

The Paneflow release key fingerprint is:

```
9809 948F 4433 CF93 DD13  2944 9A25 2F0C 183F 2711
```

`pub rsa4096/9A252F0C183F2711` - `PaneFlow Release
<paneflow-release@paneflow.dev>` - expires `2028-04-19`.

This 40-char hex value MUST match the fingerprint printed by both
`curl … | gpg --with-fingerprint` invocations below. If you only
trust a single source for the fingerprint check (just the README, just
the runbook), a coordinated MITM that controls every URL in the chain
could feed you a wrong-but-consistent fingerprint and you would pass
the cross-check. Resist that scenario by also reading the same hex
value off a third independent surface - the GitHub web UI commit
history of `keys/paneflow-release.asc`, the maintainer's social-media
profile, or the upcoming `paneflow.dev` security page - before pasting
any import command from §2.

```bash
# Fingerprint of the repo-committed copy
curl -fsSL https://raw.githubusercontent.com/ArthurDEV44/paneflow/main/keys/paneflow-release.asc \
  | gpg --with-fingerprint --with-colons \
  | awk -F: '/^fpr:/ {print $10; exit}'
# Expected: 9809948F4433CF93DD1329449A252F0C183F2711

# Fingerprint of the server-served copy
curl -fsSL https://pkg.paneflow.dev/gpg \
  | gpg --with-fingerprint --with-colons \
  | awk -F: '/^fpr:/ {print $10; exit}'
# Expected: 9809948F4433CF93DD1329449A252F0C183F2711

# Both lines MUST print the same 40-char hex fingerprint AND that value
# MUST match the literal value above. If any of the three diverge,
# STOP - do not run any §2 import command until the discrepancy is
# explained.
```

> **Note on key locations.** A second copy of the public key lives at
> `packaging/paneflow-release.asc` for historical reasons - it is the
> path the maintainer runbook (`docs/release-signing.md`) and the
> `pkg.paneflow.dev` repo-publish workflow operate against. Both files
> are byte-identical and must stay in sync; users should prefer
> `keys/paneflow-release.asc` for clarity, but either works.

## 2. Import the public key (one-time per workstation)

> **Stop.** Do not run any command in this section until you have
> completed §1 and verified the fingerprint
> `9809948F4433CF93DD1329449A252F0C183F2711` matches what each `curl
> … | gpg --with-fingerprint` invocation prints. Importing first and
> verifying second is too late - once the key is in your trust store,
> later artifact verifications will report `Good signature` against
> the malicious key.

Pick the path matching your distro's package manager. All three are
equivalent (same key, same fingerprint).

### APT (Debian / Ubuntu)

```bash
sudo apt-get install -y curl
curl -fsSL https://raw.githubusercontent.com/ArthurDEV44/paneflow/main/keys/paneflow-release.asc \
  | sudo tee /usr/share/keyrings/paneflow-archive.asc >/dev/null
```

The ASCII-armored keyring at `/usr/share/keyrings/paneflow-archive.asc` is
what the apt source-list line references via `signed-by=`. APT pins
trust to that single keyring, so installing untrusted third-party keys
into the global trust store is unnecessary and a known foot-gun (the
deprecated `apt-key add` pattern).

### DNF / RPM (Fedora / RHEL / openSUSE)

```bash
sudo rpm --import \
  https://raw.githubusercontent.com/ArthurDEV44/paneflow/main/keys/paneflow-release.asc
```

`rpm --import` deduplicates, so re-running it across releases is safe.

### Bare GPG (for `.tar.gz` / `.AppImage` verification)

```bash
gpg --import keys/paneflow-release.asc
# Optional sanity: print the imported key
gpg --list-keys paneflow-release@paneflow.dev
```

## 3. Verify a `.deb` signature

The release pipeline signs every `.deb` with `dpkg-sig` (US-017). The
matching verifier is the same tool:

```bash
sudo apt-get install -y dpkg-sig
dpkg-sig --verify paneflow-vX.Y.Z-x86_64.deb
```

> **Ubuntu 24.04 (Noble) and later.** `dpkg-sig` is unmaintained
> upstream (last upstream release 2014) and was dropped from the
> Ubuntu archive entirely - `apt-cache policy dpkg-sig` resolves to
> nothing on 24.04 even with universe enabled (verified on a fresh
> noble container, 2026-06). Use the bare-GPG fallback: extract the
> `_gpgbuilder` member with `ar x` and verify it directly:
>
> ```bash
> ar x paneflow-vX.Y.Z-x86_64.deb _gpgbuilder
> gpg --verify _gpgbuilder
> # Expected: gpg: Good signature from "PaneFlow Release …"
> ```
>
> Both verifiers consume the same signature blob; the bare-GPG path
> is what `dpkg-sig --verify` does internally.

Expected output:

```
GOODSIG _gpgbuilder <40-char-fingerprint> <UNIX-timestamp>
```

Any other status (`BADSIG`, `NOSIG`, missing fingerprint) means the
file was tampered with or is unsigned. Do **not** install it - discard
and re-download.

`apt install ./paneflow-*.deb` will additionally check the signature
against the system trust store as part of normal install validation
once the keyring has been imported per §2. The `dpkg-sig` invocation
above is the explicit pre-install gate.

## 4. Verify a `.rpm` signature

`rpm --checksig` validates against any key imported via §2 above:

```bash
rpm --checksig -v paneflow-vX.Y.Z-x86_64.rpm
```

Expected output (modern RPM ≥ 4.14, Fedora 39+ / RHEL 9+):

```
Header V4 RSA/SHA256 Signature, key ID <8-char-shortid>: OK
Header SHA256 digest: OK
Payload SHA256 digest: OK
V4 RSA/SHA256 Signature, key ID <8-char-shortid>: OK
```

Older RPM 4.13 and earlier emits a `pgp md5 OK` line and a `MD5
digest: OK` line in addition to the above - both also valid. Modern
`rpmbuild` defaults omit MD5, so its absence on packages built
post-2024 is expected, not a failure.

If any line reports `NOTOK` or `MISSING KEY`, stop and re-run the §2
import; do **not** install the package.

## 5. Verify a `.tar.gz` artifact

The canonical end-user verification recipe for a Paneflow `.tar.gz` is a
detached GPG signature against the same release key used for `.deb` and
`.rpm`:

```bash
# Download the artifact and its detached signature sidecar from the
# release page, then verify with the public key imported in §2.
gpg --verify paneflow-vX.Y.Z-x86_64.tar.gz.sig \
            paneflow-vX.Y.Z-x86_64.tar.gz
# Expected:
#   gpg: Good signature from "PaneFlow Release <paneflow-release@paneflow.dev>"
#   Primary key fingerprint: 9809 948F 4433 CF93 DD13  2944 9A25 2F0C 183F 2711
```

`gpg: Good signature` plus a fingerprint that matches the value in §1
is the trust gate. Any other status (`BAD signature`, `gpg: WARNING:
This key is not certified with a trusted signature!` without a
preceding good signature line) means the artifact was tampered with or
the key was never imported correctly - discard and re-download.

> **Current state (US-025).** The release workflow ships `.tar.gz`
> artifacts with a SHA-256 sidecar (`paneflow-*.tar.gz.sha256`) but
> does **not** yet emit the detached `.tar.gz.sig` referenced above.
> Until the deferred follow-up lands, `.tar.gz` integrity is verified
> only via:
>
> ```bash
> sha256sum --check paneflow-vX.Y.Z-x86_64.tar.gz.sha256
> # Expected: paneflow-vX.Y.Z-x86_64.tar.gz: OK
> ```
>
> The `.sha256` sidecar is also what the in-app updater
> (`update_checker.rs`, US-011) checks against the downloaded binary
> before swapping `~/.local/paneflow.app/`. Producing the detached
> `.tar.gz.sig` closes the residual gap (an attacker with R2-bucket
> write access can regenerate the SHA-256 sidecar to match a
> tampered `.tar.gz` - the GPG signature is what defeats that
> scenario) and is tracked as the next hardening story after US-025.

## 6. AppImage signatures

**Decision (US-025):** the project will use **detached GPG signatures**
for `.AppImage` artifacts when the in-AppImage signature pipeline is
added, mirroring the existing `.deb`/`.rpm` paths and using the same
release key. Sigstore (cosign + Fulcio + Rekor) was evaluated and
rejected for two reasons:

1. **Operational asymmetry.** Adding cosign would introduce a second
   signing toolchain, two trust stores for end users (GPG for
   deb/rpm/tar.gz + cosign for AppImage), and a runtime dependency on
   Sigstore's Fulcio CA + Rekor transparency-log uptime for verify-time
   lookups. The release-day blast radius would grow.
2. **No reputation gain.** Unlike SmartScreen on Windows or Gatekeeper
   on macOS, no Linux end-user trust system today gives cosign-signed
   AppImages an automatic green-check experience. Both GPG and cosign
   are explicit-verify only.

Today, integrity for `.AppImage` is **SHA-256 only**, identical to the
`.tar.gz` path:

```bash
sha256sum --check paneflow-vX.Y.Z-x86_64.AppImage.sha256
```

The `.zsync` sidecar (`paneflow-*.AppImage.zsync`) provides delta-
update integrity for `appimageupdatetool` but is **not** a signature -
it only catches transport corruption between an old and new AppImage,
not malicious tampering.

> **Hardening backlog.** Producing `paneflow-*.AppImage.sig` (detached
> GPG armor) in the release workflow is the deferred follow-up. The
> verification recipe will be:
> ```bash
> gpg --verify paneflow-vX.Y.Z-x86_64.AppImage.sig \
>             paneflow-vX.Y.Z-x86_64.AppImage
> ```
> Tracked outside this PRD.

## 7. Verifying the verification - end-to-end smoke test

A clean-room smoke test that exercises §2 + §3 on Ubuntu 22.04:

```bash
# 1. Spin up a clean container
docker run --rm -it ubuntu:22.04 bash

# Inside the container:
apt-get update && apt-get install -y curl gnupg dpkg-sig

# 2. Fetch the key, print its fingerprint, and verify against the §1
#    expected value BEFORE importing into the trust store.
curl -fsSL https://raw.githubusercontent.com/ArthurDEV44/paneflow/main/keys/paneflow-release.asc \
  -o /tmp/paneflow-release.asc
gpg --with-fingerprint --with-colons /tmp/paneflow-release.asc \
  | awk -F: '/^fpr:/ {print $10; exit}'
# Expected output: 9809948F4433CF93DD1329449A252F0C183F2711
# If it does not match, STOP - do NOT run gpg --import below.
gpg --import /tmp/paneflow-release.asc

# 3. Download a release artifact
curl -fsSLO https://github.com/ArthurDEV44/paneflow/releases/download/vX.Y.Z/paneflow-vX.Y.Z-x86_64.deb

# 4. Verify
dpkg-sig --verify paneflow-vX.Y.Z-x86_64.deb
# Expected: GOODSIG _gpgbuilder <fingerprint> <timestamp>
```

If the GOODSIG line appears, the artifact is genuine and unmodified.
If it does not, file an issue at
<https://github.com/ArthurDEV44/paneflow/issues> with the full output -
do not install the package.

## 8. Failure modes

| Symptom | Likely cause | Recovery |
|---|---|---|
| `dpkg-sig --verify` reports `BADSIG` | File modified in transit, or attacker substituted artifact | Discard, re-download from the GitHub Release (not from a mirror), re-verify. |
| `rpm --checksig` reports `NOKEY` | Public key not imported on this host | Re-run §2 RPM import. |
| `gpg --verify` reports `BAD signature` on `.AppImage.sig` | (Future: when AppImage signing lands) tampered AppImage or wrong matching `.sig` file | Discard, re-download both files from the same release. |
| Repo-committed and server-served fingerprints disagree | Package repo is compromised | **Stop.** File a security issue and do not install any artifact from `pkg.paneflow.dev` until the maintainer confirms the rotation. |
| `apt update` complains "key is stored in legacy trusted.gpg keyring" | Old keyring imported via deprecated `apt-key add` | Remove the legacy entry: `sudo apt-key del <8-char-shortid>`, then re-import per §2 (`signed-by=` keyring path). |

## 9. References

- [`docs/release-signing.md`](../release-signing.md) - maintainer-side
  key generation, secret rotation, R2 publishing.
- [`docs/release/macos-signing.md`](macos-signing.md) - equivalent
  runbook for macOS signing & notarization.
- [`docs/release/windows-signing.md`](windows-signing.md) - equivalent
  runbook for Windows Azure Trusted Signing.
- [`tasks/prd-cmux-port-2026-q2.md`](../../tasks/prd-cmux-port-2026-q2.md)
  US-025 - acceptance criteria for this document.
- `dpkg-sig(1)` - `man dpkg-sig` for the underlying signing tool.
- `rpm(8)` `--checksig` - `man rpm` and Fedora docs at
  <https://docs.fedoraproject.org/en-US/quick-docs/installing-plugins-for-playing-movies-and-music/#proc_signing>.

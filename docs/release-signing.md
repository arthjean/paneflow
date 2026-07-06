# Release signing runbook (US-017)

One-time and periodic procedures for the PaneFlow release signing key.
The `release.yml` workflow signs every `.deb` and `.rpm` artifact with
the key described here; the repo-publishing workflow
(`repo-publish.yml`, US-015) signs repository metadata with the same
key. Without the key populated as GitHub Actions secrets, the release
workflow hard-fails with `GPG signing failed: key not loaded` - by
design (AC7). No silent unsigned release is possible.

Everything after the first section is operator-only. The code in the
repo assumes the key exists and is populated as secrets.

---

## 1. Generating the key (one time)

Do this on a **trusted workstation** - never on a shared machine, never
on a CI runner, never via SSH to a multi-user box. The private key
material must never exist on disk outside the password manager.

```bash
# Write the unattended generation batch. The `%no-protection` directive
# emits a key with NO passphrase. See the trade-off discussion below.
cat > /tmp/paneflow-keygen.batch <<'EOF'
%echo Generating PaneFlow release signing key (US-017)
Key-Type: RSA
Key-Length: 4096
Key-Usage: sign
Name-Real: PaneFlow Release
Name-Email: paneflow-release@paneflow.dev
Name-Comment: PaneFlow package + repository signing
Expire-Date: 2y
%no-protection
%commit
%echo Done
EOF

# Use a temporary GNUPGHOME so the key doesn't land in the operator's
# main keyring (and can't leak via personal git commits, gpg-preset-
# passphrase cache, etc.). Remove the directory after export.
export GNUPGHOME=$(mktemp -d)
gpg --batch --generate-key /tmp/paneflow-keygen.batch

# Grab the 40-char fingerprint - this is the GPG_KEY_ID secret value.
FPR=$(gpg --list-keys --with-colons paneflow-release@paneflow.dev \
      | awk -F: '/^fpr:/ {print $10; exit}')
echo "Fingerprint: $FPR"

# Generate a revocation certificate NOW, while we have the private key
# material on hand. This file is what you publish (and append to `/gpg`)
# if the private key is ever compromised - see §5e Emergency Rotation.
# Store it in the password manager alongside the private key. Without a
# revocation cert prepared in advance, an emergency rotation has no
# signal to tell users "don't trust this key anymore".
gpg --batch --yes --armor \
    --output /tmp/paneflow-release-revoke.asc \
    --gen-revoke "$FPR" <<EOF
y
0
key retired as part of rotation - see docs/release-signing.md §5e
y
EOF

# Export the private key (ASCII-armored). THIS IS THE GPG_PRIVATE_KEY
# secret - put it in the password manager immediately and paste into
# GitHub Actions secrets from there.
gpg --armor --export-secret-keys "$FPR" > /tmp/paneflow-release-private.asc

# Export the public key. Commit this to the repo as
# packaging/paneflow-release.asc for the audit trail (AC2).
gpg --armor --export "$FPR" > packaging/paneflow-release.asc

# Remove the temporary GNUPGHOME - everything secret is now either in
# the password manager or destroyed.
rm -rf "$GNUPGHOME"
unset GNUPGHOME
```

### Passphrase vs `%no-protection`

The batch above uses `%no-protection`, which emits a key with no
passphrase. That is the simplest and most secure choice for a key that
lives exclusively inside GitHub Actions secrets:

- GitHub secrets are already encrypted at rest with AES-256 and
  decrypted only during workflow execution. A passphrase adds a second
  encryption layer that must itself be stored as a second secret
  (`GPG_PASSPHRASE`), giving an attacker who has read access to
  secrets both halves anyway.
- Passphrase-protected keys require `gpg-agent` + loopback pinentry
  plumbing in CI; the moving parts have a history of breaking in
  subtle ways across runner image upgrades.

The alternative - a passphrase-protected key - is still supported by
the release workflow:

```gpg
# Replace `%no-protection` with:
Passphrase: strong-random-string
```

Store `strong-random-string` as a `GPG_PASSPHRASE` secret alongside
`GPG_PRIVATE_KEY`. The workflow's `~/.rpmmacros` and reprepro invocations
already pass the passphrase via `--passphrase-file <(…)`.

If `GPG_PASSPHRASE` is empty (or the secret doesn't exist), the workflow
treats the key as passphrase-less. No code change required.

---

## 2. Populating GitHub Actions secrets

Under the repository's *Settings* → *Secrets and variables* →
*Actions*:

| Secret | Value | Notes |
|---|---|---|
| `GPG_PRIVATE_KEY` | Full content of `paneflow-release-private.asc` | Starts with `-----BEGIN PGP PRIVATE KEY BLOCK-----` |
| `GPG_PASSPHRASE` | Key passphrase (or leave unset if `%no-protection`) | An empty string is also acceptable |
| `GPG_KEY_ID` | The 40-char fingerprint from step 1 | Hex only, no spaces |

The `repo-publish.yml` workflow (US-015) additionally needs
`R2_ACCESS_KEY_ID`, `R2_SECRET_ACCESS_KEY`, `R2_ENDPOINT`, `R2_BUCKET`.
See `docs/pkg-repo-runbook.md` for those.

### Setting GPG secrets on GitHub (US-005)

The three secrets above are best populated via the `gh` CLI rather
than pasted through the GitHub UI (the UI silently line-wraps long
PEM blocks at field boundaries). But `gh secret set` has ONE
non-obvious quirk for multi-line values - **always feed them via a
stdin-redirect from a file, never via a pipe from `gpg`**:

```bash
# ❌ DO NOT - the pipe form has been observed to truncate the exported
# key at a buffer boundary, storing a malformed blob. A 4096-bit RSA
# ASCII-armored private key block is long enough to hit this.
gpg --armor --export-secret-keys "$FPR" | gh secret set GPG_PRIVATE_KEY

# ✅ DO - export to a file, set permissions, upload via stdin-redirect,
# then securely erase the local copy. `gh secret set < file` reads the
# full file contents in one go:
gpg --armor --export-secret-keys "$FPR" > /tmp/paneflow-release-private.asc
chmod 600 /tmp/paneflow-release-private.asc
gh secret set GPG_PRIVATE_KEY -R <owner>/<repo> < /tmp/paneflow-release-private.asc
shred -u /tmp/paneflow-release-private.asc
```

Substitute `<owner>/<repo>` for the repository slug (e.g.,
`ArthurDEV44/paneflow`). If you run `gh secret set` from inside a
checked-out clone, the `-R` flag is optional - `gh` auto-detects the
repo from `.git` config.

Single-line secrets (`GPG_KEY_ID`, `GPG_PASSPHRASE`) do NOT hit the
truncation risk and can use the `--body` form:

```bash
gh secret set GPG_KEY_ID     -R <owner>/<repo> --body "$FPR"
gh secret set GPG_PASSPHRASE -R <owner>/<repo> --body "$PASSPHRASE"
```

**Historical note (v0.2.0 retry #5, 2026-04-21):** using the pipe form
for `GPG_PRIVATE_KEY` during that release cycle stored a truncated blob,
and the next CI run hard-failed at the GPG-import step with:

> GPG_PRIVATE_KEY failed to import. Secret may be truncated or missing the ASCII-armor header.

If that error surfaces on a future rotation, the fix is always the same
four lines above - re-export to a file, `chmod 600`, `gh secret set ... < /tmp/file`, `shred`. This recipe is robust regardless of the `gh` CLI version, the terminal used, or whether the original pipe-truncation root cause still exists in the current `gh` release.

---

## 3. Committing the public key

Replace the placeholder `packaging/paneflow-release.asc` (committed to
the repo as a stub in US-017) with the real ASCII-armored public key
exported in step 1.

```bash
# From the exported public key in step 1:
gpg --armor --export "$FPR" > packaging/paneflow-release.asc

# Sanity check: no private material
grep -q 'PRIVATE KEY' packaging/paneflow-release.asc && {
    echo "FATAL: private key in public file - aborting commit"
    exit 1
}

git add packaging/paneflow-release.asc
git commit -m "chore(packaging): publish PaneFlow release key public half"
```

The file is never read by CI - `repo-publish.yml` exports the public
key fresh from the imported secret at runtime. The committed copy
exists solely for offline audit and third-party verification that the
key advertised at `https://pkg.paneflow.dev/gpg` matches what maintainers
intended.

---

## 4. Verifying signatures locally

After a signed release is produced (either by the workflow or by a
manual `cargo deb && dpkg-sig` run), consumers can verify the
signatures with the public key.

```bash
# Import the public key (one-time per workstation)
gpg --import packaging/paneflow-release.asc

# Verify a .deb
dpkg-sig --verify paneflow-*.deb
# Expect: "GOODSIG _gpgbuilder <fingerprint> <timestamp>"

# Verify a .rpm
rpm --import packaging/paneflow-release.asc
rpm --checksig -v paneflow-*.rpm
# Expect: "digests signatures OK" (modern rpm) or "pgp md5 OK" (legacy)
```

---

## 5. Rotating the key (every 2 years or on compromise)

The expiry date in step 1 is 2 years - gpg will refuse to sign new
artifacts after that point, which forces a rotation. The rotation
procedure must preserve signature validation for users who already
have the old public key pinned on their system (APT `signed-by=` or
RPM `gpgkey=`).

### 5a. Generate the new key

Same procedure as step 1, but use a different batch file (e.g., with
`Name-Comment: PaneFlow package + repository signing (2028)`). Export
both halves; populate a **new** pair of secrets - `GPG_PRIVATE_KEY_NEW`,
`GPG_PASSPHRASE_NEW`, `GPG_KEY_ID_NEW` - without disturbing the live
secrets. Keep both key pairs available during the transition window.

### 5b. APT - publish new key via a keyring update

The `signed-by=/usr/share/keyrings/paneflow-archive.asc` file pins the
repo to a specific keyring. Any key **in that file** is trusted, so
the transition is a matter of appending the new key to the keyring
without removing the old one.

Append both keys to `packaging/paneflow-release.asc`:

```bash
gpg --armor --export OLD_FPR NEW_FPR > packaging/paneflow-release.asc
git commit -am "chore(packaging): add 2028 release key to transition keyring"
```

Next release: `repo-publish.yml` publishes the updated multi-key
ASCII-armor at `/gpg`. Users who refresh the packaged keyring manually or
via future package upgrade pick up both keys; users with the old
single-key keyring continue to trust the old key until they refresh.

For the cleanest client migration, ship a dedicated
`paneflow-archive-keyring` package (future story) that owns
`/usr/share/keyrings/paneflow-archive.asc` - an `apt upgrade` then
atomically delivers the multi-key keyring. This is the pattern Debian
(`debian-archive-keyring`), GitHub CLI (`gh-cli-keyring`), and
DataDog (`datadog-keyring`) use.

### 5c. RPM - list both keys in the `.repo` file

The RPM equivalent lives in the `[paneflow]` block in
`/etc/yum.repos.d/paneflow.repo`. Edit `packaging/rpm/postinst.sh` to
emit both URLs (space-separated or newline-continued):

```ini
gpgkey=https://pkg.paneflow.dev/gpg
       https://pkg.paneflow.dev/gpg-2028
```

Publish the new key at `/gpg-2028` (as a second object in R2; update
`repo-publish.yml` to upload both). Clients that run `dnf install`
during the transition see both keys and accept packages signed by
either.

### 5d. Switch signing (after N weeks)

Schedule the switchover - conventional window is 3-6 months. At the
end of it:

1. Swap the CI secrets: `GPG_PRIVATE_KEY`/`GPG_KEY_ID` now hold the
   NEW key; delete the `_NEW` aliases.
2. `repo-publish.yml` re-signs repomd.xml and the APT Release files
   with the new key on the next publish.
3. Packages signed by the old key keep verifying (old key still in
   the keyring).
4. After 2 years of transition, drop the old key from
   `packaging/paneflow-release.asc` and from the RPM `.repo` file.

### 5e. Emergency rotation (key compromise)

If the private key is known-compromised:

1. **Immediately** revoke the old key publicly. Generate a revocation
   certificate during key generation (step 1) - `gpg --output
   revoke-OLDFPR.asc --gen-revoke OLD_FPR` - and keep it in the
   password manager.
2. Publish the revocation cert at
   `https://pkg.paneflow.dev/gpg-OLDFPR.revoke` and announce on the
   project's release-notes channel.
3. Execute steps 5a-5d as a **compressed** timeline (no 3-month
   transition - hours to days).
4. Consider whether to pull all prior releases from the download page
   to prevent downgrade attacks. For PaneFlow this probably isn't
   worth it; announce that users must re-verify their installed
   packages and reinstall if the signature doesn't match a
   non-compromised key.

---

## 6. Troubleshooting

### `GPG signing failed: key not loaded`

The workflow's fail-fast check. One of:

- `GPG_PRIVATE_KEY` secret is unset or empty. Populate per step 2.
- The secret value is truncated - often because a `gpg ... | gh secret
  set` pipe cut the key at a buffer boundary. Re-upload via the
  stdin-redirect recipe in §2 "Setting GPG secrets on GitHub". The
  literal upstream error is:
  `GPG_PRIVATE_KEY failed to import. Secret may be truncated or missing the ASCII-armor header.`
- `GPG_KEY_ID` doesn't match the key inside `GPG_PRIVATE_KEY`. The
  workflow's imported-fingerprint check catches this. Re-copy both
  secrets from the same source.

### `dpkg-sig --verify` says `BADSIG`

The signature doesn't match the .deb's contents. Usually means the
.deb was modified AFTER signing (e.g., rebuilding and not re-signing).
Fix: re-run the workflow - the sign step runs on every build.

### `rpm --checksig` says `NOKEY`

The local RPM DB doesn't have the public key. Run `rpm --import
packaging/paneflow-release.asc` on the verifier's machine.

### rpmsign hangs in CI

`gpg-agent` is waiting for an interactive pinentry. Check that
`~/.rpmmacros` was written correctly with `--pinentry-mode loopback`
and that the gpg-agent config has `allow-loopback-pinentry` (the
workflow does both - if you see this, something upstream in a runner
image update is the likely culprit).

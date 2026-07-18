# pkg.paneflow.dev - APT + RPM repository runbook

One-time setup and operator procedures for the hosted package repository.
Referenced by US-015 (hosting) and US-017 (GPG signing).

The CI workflow `.github/workflows/repo-publish.yml` automates every
publish after a GitHub release is cut - this document covers only the
steps that cannot be automated (cloud console + secret provisioning) and
the troubleshooting hooks for when things go wrong.

---

## 1. Cloudflare R2 bucket

1. In the Cloudflare dashboard, create an R2 bucket named
   **`paneflow-pkg`** (eu-west region recommended for latency; the
   bucket-level region is ignored by rclone but affects replication).
2. **Public read access:** under the bucket's *Settings* → *Public
   Access*, enable "Allow public access via custom domain". Do NOT
   enable the `r2.dev` subdomain - that route doesn't support caching
   rules and would let anyone scrape egress.
3. **Custom domain:** bind **`pkg.paneflow.dev`** to the bucket. This
   requires the domain to be in the same Cloudflare account; if the
   DNS is elsewhere, create a CNAME pointing at the R2 endpoint and
   enable Cloudflare proxy.
4. **Egress budget:** the PRD targets < 10 GB/month. Set an email alert
   at 8 GB in *Cloudflare* → *Notifications* → *Billing Alerts*.

## 2. R2 API credentials

Cloudflare uses an S3-compatible API for R2. Create a dedicated token
 -  do NOT reuse the account-wide token.

1. R2 → *Manage API tokens* → *Create API token*.
2. Scope: **Object Read & Write**, bucket filter: **`paneflow-pkg`**.
   (Not admin; not account-wide - scope to the single bucket.)
3. TTL: **no expiration** (rotate annually via calendar reminder).
4. Copy the Access Key ID, Secret Access Key, and endpoint URL.

## 3. GPG signing key (US-017)

Generate once, store encrypted. Do NOT check the private key into git.

```bash
# Generate a 4096-bit RSA key, 2-year expiry, paneflow-release@paneflow.dev
gpg --batch --full-generate-key <<EOF
%no-protection
Key-Type: RSA
Key-Length: 4096
Key-Usage: sign
Name-Real: PaneFlow Release
Name-Email: paneflow-release@paneflow.dev
Expire-Date: 2y
%commit
EOF

# Grab the fingerprint for reprepro's SignWith config
gpg --list-secret-keys --keyid-format=long paneflow-release@paneflow.dev

# Export the private key (ASCII-armored - what the CI secret expects).
# IMPORTANT: write to /tmp (ephemeral) - NEVER to the repo tree. An
# accidental `git add .` with the private key at the repo root would
# leak the signing material permanently. `docs/release-signing.md:68`
# uses the same convention; keep them aligned.
gpg --armor --export-secret-keys paneflow-release@paneflow.dev > /tmp/paneflow-release-private.asc

# Export the public key (ASCII-armored - committed to repo for audit).
# This one is safe to write inside the tree.
gpg --armor --export paneflow-release@paneflow.dev > packaging/paneflow-release.asc
```

Paste the content of `/tmp/paneflow-release-private.asc` into the
`GPG_PRIVATE_KEY` GitHub Actions secret (step 4 below), then delete
the file: `shred -u /tmp/paneflow-release-private.asc`. Commit
`packaging/paneflow-release.asc` (the **public** key). Back up the
private key to a password manager. A key rotation procedure is
documented in `docs/release-signing.md`.

## 4. GitHub Actions secrets

Under the repository's *Settings* → *Secrets and variables* →
*Actions*, create these secrets. The `repo-publish.yml` workflow has a
"Verify required secrets are present" step that fails fast with a
human-readable list if any are missing.

| Secret | Value | Notes |
|---|---|---|
| `GPG_PRIVATE_KEY` | Full content of `paneflow-release.asc` (private) | Starts with `-----BEGIN PGP PRIVATE KEY BLOCK-----` |
| `GPG_PASSPHRASE` | Passphrase for the private key | Only used if you set one (our `%no-protection` default above leaves it empty - then use a single space) |
| `GPG_KEY_ID` | 40-character fingerprint | e.g. `ABCD1234EF56...` - no spaces |
| `R2_ACCESS_KEY_ID` | From step 2 | S3-compatible access key |
| `R2_SECRET_ACCESS_KEY` | From step 2 | S3-compatible secret |
| `R2_ENDPOINT` | `https://<account-id>.r2.cloudflarestorage.com` | From R2 dashboard |
| `R2_BUCKET` | `paneflow-pkg` | Bucket name only (no `r2://` prefix) |

## 5. First publish - bootstrap

The workflow assumes an empty bucket is a valid state. The first tag
push after secrets are populated will:

1. `rclone sync` returns 0 against the empty bucket (no files).
2. The workflow falls back to its baked-in `packaging/apt/conf/` defaults.
3. After reprepro + createrepo_c run, `rclone sync` uploads the fresh repo.
4. The public key is uploaded to `/gpg`.

There is no separate "initialise bucket" step.

## 6. Verifying after a publish

From a clean machine:

```bash
# APT
curl -fsSL https://pkg.paneflow.dev/gpg \
  | sudo tee /usr/share/keyrings/paneflow-archive.asc >/dev/null
echo "deb [signed-by=/usr/share/keyrings/paneflow-archive.asc] https://pkg.paneflow.dev/apt stable main" \
  | sudo tee /etc/apt/sources.list.d/paneflow.list
sudo apt update
sudo apt-cache policy paneflow   # should show pkg.paneflow.dev as a source
sudo apt install paneflow

# RPM
# Bootstrap: download any released .rpm once, install it locally - its
# %post scriptlet drops /etc/yum.repos.d/paneflow.repo pointing at
# pkg.paneflow.dev (US-016). Subsequent upgrades are automatic.
curl -fsSLO https://github.com/arthjean/paneflow/releases/latest/download/paneflow-vX.Y.Z-x86_64.rpm
sudo dnf install -y ./paneflow-vX.Y.Z-x86_64.rpm
sudo dnf check-update paneflow   # should show pkg.paneflow.dev as a source
sudo dnf upgrade paneflow
```

## 7. Troubleshooting

### `apt update` fails with "Signature by key X uses weak digest"

The key was generated with SHA-1 as the fallback hash. Regenerate with
explicit `Digest-Algo: SHA256` (default in GnuPG 2.4+). Rotation
procedure: publish both keys at `/gpg` for one release cycle, then
remove the old one.

### `rclone sync` fails with "403 Forbidden" on HEAD

The workflow sets `RCLONE_CONFIG_R2_NO_CHECK_BUCKET=true` precisely to
avoid this - R2 returns 403 on the bucket-level HEAD even with correct
credentials. If you see it anyway, verify the env var was set.

### `reprepro` hangs on key signing

GPG is prompting for passphrase. The workflow pre-caches the passphrase
via `gpg --quick-set-expire` as a side-effect, which won't help if the
key has no passphrase (`%no-protection`) - in that case just remove the
`GPG_PASSPHRASE` wiring. If the key is passphrase-protected, make sure
`allow-loopback-pinentry` is in `~/.gnupg/gpg-agent.conf` and agent was
killed with `gpgconf --kill gpg-agent`.

### A release was corrupted on R2

The workflow is idempotent - re-running `workflow_dispatch` with the
same tag rebuilds metadata from the existing pool files and re-signs.
If the pool itself is corrupted, delete the affected `*.deb`/`*.rpm`
from R2 via the Cloudflare UI (or `rclone deletefile R2:paneflow-pkg/...`)
and re-dispatch.

### Disaster recovery: "I lost the private GPG key"

There is no recovery path for signed-repo integrity without the private
key. The user-facing fix is to:

1. Generate a new key (step 3 above).
2. Publish both the old public key and the new one at `/gpg` (concatenate
   the armored exports).
3. Push a tag - the next publish re-signs everything with the new key.
4. After one release cycle, drop the old public key from `/gpg`.

Clients who imported the old key will need to re-import from `/gpg`.
Document in release notes.

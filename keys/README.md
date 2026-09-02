# Paneflow signing keys

This directory holds public-key material that end users need to verify
the authenticity of Paneflow release artifacts.

## `paneflow-release.asc`

ASCII-armored OpenPGP public key used to sign every `.deb` and `.rpm`
release artifact, the apt/dnf package-repository metadata, and (when
the workflow lands the deferred signing step) the `.tar.gz` and
`.AppImage` artifacts.

- **Fingerprint:** `9809 948F 4433 CF93 DD13  2944 9A25 2F0C 183F 2711`
- **User ID:** `PaneFlow Release <paneflow-release@paneflow.dev>`
- **Algorithm / size:** RSA 4096
- **Expires:** 2028-04-19

**For verification instructions, read
[`docs/release/linux-signing.md`](../docs/release/linux-signing.md)
first.** Do not import this file into your trust store before
verifying the fingerprint per the runbook §1 - pre-emptive imports
defeat the trust-bootstrap step.

## `paneflow-release.asc` ↔ `packaging/paneflow-release.asc`

This file is byte-identical to
[`packaging/paneflow-release.asc`](../packaging/paneflow-release.asc).
The duplication exists for two distinct audiences:

- `keys/` - end-user-discoverable path referenced by
  `docs/release/linux-signing.md` and `README.md`.
- `packaging/` - maintainer-canonical path referenced by the
  `pkg.paneflow.dev` repo-publish workflow.

**Both files MUST stay in sync.** A maintainer rotating the key under
the operator runbook should update both paths in the same commit.

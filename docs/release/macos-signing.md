# macOS signing & notarization runbook (US-023)

One-time setup, secret rotation, and expiration tracking for the PaneFlow
macOS release pipeline. The `release.yml` workflow signs and notarizes
every `aarch64-apple-darwin` `.app` produced by `scripts/bundle-macos.sh`
when the five `APPLE_*` GitHub Secrets are populated. If any secret is
missing the leg degrades to an **unsigned** build with a banner in the
job summary - by design (US-023 AC7).

This document is operator-only. Application code never reads from any of
the files described here.

---

## 1. What gets signed

`scripts/bundle-macos.sh` produces a flat bundle:

```
dist/PaneFlow.app/Contents/
  MacOS/paneflow
  Info.plist
  Resources/PaneFlow.icns
```

`scripts/sign-macos.sh` runs in two passes:

1. **Inside-out walk** - every Mach-O / `.dylib` / `.framework` / `.xpc`
   under `Contents/Frameworks`, `Contents/Helpers`, `Contents/PlugIns`,
   `Contents/XPCServices`. Today the bundle has none of those, so this
   pass is a no-op; it stays in place so adding a helper bundle later
   needs zero signing-script changes.
2. **Top-level sign** - `Contents/MacOS/paneflow` and the parent `.app`
   wrapper, with `--entitlements` bound and the hardened runtime enabled.

`scripts/notarize-macos.sh` then submits the bundle to `notarytool`,
waits, staples the ticket, and runs `spctl --assess --type exec --verbose`
as a Gatekeeper smoke test.

## 2. Entitlements files

Three entitlements plists live under `packaging/macos/`. Each is a
distinct file so future variant-specific tweaks can land in isolation.
**The release and nightly variants ship to users; the dev variant is
local-only and must never be sent to notarytool.**

| File | When to use | Notable keys |
|---|---|---|
| `paneflow.entitlements` | Tagged `v*` releases (default for `sign-macos.sh`). | `app-sandbox=false`, `automation.apple-events`, `cs.allow-jit`, `cs.allow-unsigned-executable-memory`. |
| `paneflow.nightly.entitlements` | Nightly builds shipped under `io.github.arthurdev44.paneflow.nightly`. | Same strict key set as release; forked file so nightly-only entitlements can be added without touching the production file. |
| `paneflow.dev.entitlements` | **Local only.** Use when you need to attach `lldb` to a signed build on your own machine. | Adds `com.apple.security.get-task-allow=true`. **Notarization rejects any bundle carrying this entitlement** - never use for distribution. |

The `cs.*` block is required for any GPUI / wgpu app under the hardened
runtime: GPUI compiles `MTLComputePipelineState` objects at first use,
which Apple classifies as JIT.

## 3. One-time onboarding

1. **Apple Developer account.** Decided 2026-04-18: **Individual / Sole
   Proprietor**, owner email `arthur.jean@strivex.fr`. Membership active,
   D-U-N-S not required for this account type.
2. **Generate a Developer ID Application certificate.**
   - Xcode → Settings → Accounts → Manage Certificates → `+` → "Developer
     ID Application".
   - Common name will be `Developer ID Application: <your name> (TEAMID)`.
   - Note the **Created** and **Expires** dates. Set a calendar reminder
     for 60 days before expiry - Gatekeeper rejects releases signed with
     an expired certificate.
3. **Export to `.p12`.** Keychain Access → Login → expand the cert → right-
   click the private key (the disclosure-triangle child) → Export. Choose
   `.p12`, set a strong password, save to a temp file. Memorize the
   password into your password manager **before** you upload anything.
4. **Encode for GitHub Secrets.** Prefer `gh` CLI to avoid putting the
   encoded `.p12` in the system clipboard (Universal Clipboard syncs to
   every signed-in Apple device, and most clipboard managers retain
   history):
   ```bash
   gh secret set APPLE_DEVELOPER_CERT_P12 < <(base64 -i developer-id.p12)
   ```
   If `gh` is unavailable, write to a temp file, paste it into the
   GitHub Secrets UI manually, and shred:
   ```bash
   base64 -i developer-id.p12 > /tmp/cert.b64
   # Open https://github.com/<owner>/<repo>/settings/secrets/actions
   # and paste the contents into APPLE_DEVELOPER_CERT_P12.
   shred -u /tmp/cert.b64   # or `rm -P /tmp/cert.b64` on macOS
   ```
5. **Generate an app-specific password** at
   <https://account.apple.com/account/manage> → Sign-In and Security →
   App-Specific Passwords. Label it "PaneFlow Notarization". Save the
   16-character output to your password manager.
6. **Locate your Team ID** at <https://developer.apple.com/account>
   under Membership Details → Team ID. 10 alphanumeric characters.
7. **Populate the five GitHub Secrets** under repo Settings → Secrets and
   Variables → Actions:

   | Secret | Source | Notes |
   |---|---|---|
   | `APPLE_DEVELOPER_CERT_P12` | `pbpaste` from step 4 | Base64. Whitespace-tolerant. |
   | `APPLE_DEVELOPER_CERT_PASSWORD` | The password set during `.p12` export | |
   | `APPLE_ID` | `arthur.jean@strivex.fr` | The account that owns the membership. |
   | `APPLE_APP_SPECIFIC_PASSWORD` | App-specific password from step 5 | NOT the Apple ID login password. |
   | `APPLE_TEAM_ID` | From step 6 | Plain text, no quotes, no spaces. |
8. **Re-run the release workflow** (Actions → Release → "Re-run failed
   jobs"). The `Detect macOS signing secrets` step should now print
   `signing_available=true` and the sign + notarize steps should fire.
9. **First-time verification.** Download the published `.dmg` on a clean
   macOS machine, open it, drag to `/Applications`, double-click. Expected:
   no Gatekeeper prompt, app launches. If you see a prompt, the
   notarization ticket is missing - look at the `Notarize + staple macOS
   .app bundle` step's `notarytool log` output for the cause.

## 4. Local dev signing (no CI)

If you need to sign a build on your own machine - typically when
attaching `lldb` to a hardened-runtime binary - use the `.dev`
entitlements:

```bash
cargo build --release --target aarch64-apple-darwin -p paneflow-app
bash scripts/bundle-macos.sh --version 0.0.0-dev --arch aarch64

# Provide your secrets via env (replace with real values):
export APPLE_DEVELOPER_CERT_P12="$(base64 -i ~/secrets/dev-id.p12)"
export APPLE_DEVELOPER_CERT_PASSWORD='...'
export APPLE_TEAM_ID='ABCDE12345'

bash scripts/sign-macos.sh \
    --entitlements packaging/macos/paneflow.dev.entitlements \
    dist/PaneFlow.app

# DO NOT notarize a dev build - get-task-allow guarantees rejection.
```

Once signed with the dev entitlements you can `lldb -- dist/PaneFlow.app/Contents/MacOS/paneflow`.

## 5. Periodic maintenance

| Cadence | Task |
|---|---|
| **Every 12 months** | Rotate `APPLE_APP_SPECIFIC_PASSWORD`. App-specific passwords have no hard expiry but a yearly rotation matches the cert renewal cycle and limits credential blast radius. |
| **Per cert expiry (typically 1-3 years)** | Re-run §3 steps 2-4 to mint a new `.p12`, then update `APPLE_DEVELOPER_CERT_P12` and `APPLE_DEVELOPER_CERT_PASSWORD`. **Releases already in the wild keep working** - notarization tickets are timestamped and remain valid past cert expiry. |
| **On Apple ID password change** | Old app-specific passwords are NOT auto-revoked by an Apple ID password change (Apple's design), but rotate to be safe: revoke at appleid.apple.com → App-Specific Passwords → trash icon, generate a new one, update `APPLE_APP_SPECIFIC_PASSWORD`. |
| **On Team ID change** | Should never happen for an individual account, but if you migrate to a Company account later, regenerate the cert under the new team and update both `APPLE_TEAM_ID` and `APPLE_DEVELOPER_CERT_P12`. |

## 6. Troubleshooting

- **`notarytool submit` returns `Invalid` with `The binary is not signed`.**
  The bundle reached Apple but a nested binary was unsigned. Run
  `codesign --verify --deep --strict --verbose=2 dist/PaneFlow.app` on the
  pre-submitted bundle to find the offender. If a new helper was added,
  confirm it lives under `Contents/Frameworks` / `Contents/Helpers` /
  `Contents/PlugIns` / `Contents/XPCServices` so the inside-out walk in
  `sign-macos.sh` picks it up automatically.
- **`The signature does not include a secure timestamp`.** The signing
  step ran but `--timestamp` was missing or Apple's TSA was unreachable.
  Re-run the leg; transient TSA failures usually clear within minutes.
- **`The executable requests the com.apple.security.get-task-allow
  entitlement`.** A dev build was sent to notarytool by accident - the
  `.dev` entitlements made it into CI. Verify
  `release.yml` passes `paneflow.entitlements` (release), not
  `paneflow.dev.entitlements`.
- **`error reading entitlements`** during codesign. The `.entitlements`
  file is malformed XML. `plutil -lint packaging/macos/*.entitlements`
  on the affected file. The sign script `plutil`-lints before signing,
  so a clean local run will surface the same error before CI.
- **`spctl --assess` returns `rejected`** after a successful staple. The
  ticket attached but Apple's revocation feed considers the cert
  invalid. Check certificate validity at developer.apple.com and your
  local clock (a desynchronized runner clock can spuriously reject
  valid tickets).

## 7. Related files

- `scripts/sign-macos.sh` - codesign driver (inside-out walk + parent sign).
- `scripts/notarize-macos.sh` - notarytool + staple + `spctl --assess`.
- `scripts/bundle-macos.sh` - produces the `.app` consumed by the two scripts above.
- `.github/workflows/release.yml` - `Detect macOS signing secrets` /
  `Sign macOS .app bundle` / `Notarize + staple macOS .app bundle` /
  `Record unsigned macOS build in job summary` steps.
- `packaging/macos/paneflow.entitlements`, `paneflow.dev.entitlements`,
  `paneflow.nightly.entitlements` - the three entitlements variants.
- `assets/Info.plist` - release bundle ID `io.github.arthurdev44.paneflow`.

#!/usr/bin/env bash
set -euo pipefail

if [ "$#" -ne 1 ]; then
  echo "usage: sign-linux-packages.sh <rust-target-triple>" >&2
  echo "  signs and verifies target/<triple>/debian/*.deb and target/<triple>/generate-rpm/*.rpm" >&2
  echo "  environment: GPG_PRIVATE_KEY, GPG_KEY_ID (40 hex), GPG_PASSPHRASE (may be empty), RELEASE_MODE (release or dry-run)" >&2
  exit 2
fi
TARGET="$1"
RELEASE_MODE="${RELEASE_MODE:-release}"
GITHUB_REF="${GITHUB_REF:-this ref}"

if [ -z "${GPG_PRIVATE_KEY:+set}" ] || [ -z "${GPG_KEY_ID:+set}" ]; then
  if [ "${RELEASE_MODE}" = "dry-run" ]; then
    echo "::warning title=GPG signing skipped::Signing secrets unavailable on ${GITHUB_REF}: building unsigned dry-run artifacts"
    exit 0
  fi
  echo "::error title=GPG signing::GPG signing failed: key not loaded"
  echo "::error::GPG_PRIVATE_KEY and/or GPG_KEY_ID secrets are not populated." >&2
  exit 1
fi
if ! [[ "$GPG_KEY_ID" =~ ^[0-9A-Fa-f]{40}$ ]]; then
  echo "::error title=GPG signing::GPG signing failed: key not loaded"
  echo "::error::GPG_KEY_ID must be the 40-char hex fingerprint (got ${#GPG_KEY_ID} chars). Short key IDs are rejected because they are collision-vulnerable." >&2
  exit 1
fi

mkdir -p ~/.gnupg
chmod 700 ~/.gnupg
echo "allow-loopback-pinentry" >> ~/.gnupg/gpg-agent.conf
gpgconf --kill gpg-agent || true
if ! gpg --batch --import <<< "$GPG_PRIVATE_KEY" 2>&1; then
  echo "::error title=GPG signing::GPG signing failed: key not loaded"
  echo "::error::GPG_PRIVATE_KEY failed to import. Secret may be truncated or missing the ASCII-armor header." >&2
  exit 1
fi
if ! gpg --list-secret-keys --with-colons \
     | awk -F: '/^fpr:/ {print $10}' \
     | grep -qi "${GPG_KEY_ID}$"; then
  echo "::error title=GPG signing::GPG signing failed: key not loaded"
  echo "::error::GPG_PRIVATE_KEY does not match GPG_KEY_ID fingerprint. Secrets likely mismatched." >&2
  exit 1
fi

if [ -n "${GPG_PASSPHRASE:+set}" ]; then
  printf 'prime' | gpg --batch --yes \
    --passphrase-file <(printf '%s' "$GPG_PASSPHRASE") \
    --pinentry-mode loopback \
    --local-user "$GPG_KEY_ID" \
    --armor --clearsign \
    --output /dev/null
fi

gpg --list-secret-keys >/dev/null

PASS_FILE=$(mktemp)
chmod 600 "$PASS_FILE"
trap 'rm -f "$PASS_FILE" "$HOME/.rpmmacros"' EXIT
printf '%s' "${GPG_PASSPHRASE:-}" > "$PASS_FILE"

cat > "$HOME/.rpmmacros" <<RPMMACROS
%_signature gpg
%_gpg_name  $GPG_KEY_ID
%__gpg      /usr/bin/gpg
%__gpg_sign_cmd %{__gpg} --batch --yes \
    --pinentry-mode loopback \
    --passphrase-file $PASS_FILE \
    --no-verbose \
    --no-armor \
    --no-secmem-warning \
    -u "%{_gpg_name}" \
    -sbo %{__signature_filename} \
    %{__plaintext_filename}
RPMMACROS

for deb in target/"${TARGET}"/debian/paneflow_*.deb; do
  [ -f "$deb" ] || continue
  echo "→ dpkg-sig --sign builder $deb"
  dpkg-sig -k "$GPG_KEY_ID" --sign builder "$deb"
done

for rpm in target/"${TARGET}"/generate-rpm/paneflow-*.rpm; do
  [ -f "$rpm" ] || continue
  echo "→ rpmsign --addsign $rpm"
  rpmsign --addsign "$rpm"
done

PUBKEY_FILE=$(mktemp --suffix=.asc)
chmod 600 "$PUBKEY_FILE"
RPM_DB=$(mktemp -d)
trap 'rm -f "$PASS_FILE" "$HOME/.rpmmacros" "$PUBKEY_FILE"; rm -rf "$RPM_DB"' EXIT
gpg --armor --export "$GPG_KEY_ID" > "$PUBKEY_FILE"
rpm --dbpath "$RPM_DB" --initdb
rpm --dbpath "$RPM_DB" --import "$PUBKEY_FILE"
KEY_SHORT=$(echo "$GPG_KEY_ID" | tail -c9 | tr 'A-F' 'a-f')
if ! rpm --dbpath "$RPM_DB" -qa 'gpg-pubkey*' | grep -qi "${KEY_SHORT}"; then
  echo "::error::Public key ${KEY_SHORT} not imported into rpm DB at ${RPM_DB}. Dumping keyring for diagnostics:"
  rpm --dbpath "$RPM_DB" -qa 'gpg-pubkey*' >&2 || true
  exit 1
fi
echo "→ Imported gpg-pubkey-${KEY_SHORT} into isolated rpm DB at ${RPM_DB}"
for deb in target/"${TARGET}"/debian/paneflow_*.deb; do
  [ -f "$deb" ] || continue
  echo "→ dpkg-sig --verify $deb"
  dpkg-sig --verify "$deb" | tee /tmp/dpkg-sig-verify.log
  if [ "${PIPESTATUS[0]}" -ne 0 ]; then
    echo "::error::dpkg-sig --verify failed on $deb"
    exit 1
  fi
done
for rpm_path in target/"${TARGET}"/generate-rpm/paneflow-*.rpm; do
  [ -f "$rpm_path" ] || continue
  echo "→ rpm --dbpath $RPM_DB --checksig -v $rpm_path"
  rpm --dbpath "$RPM_DB" --checksig -v "$rpm_path" | tee /tmp/rpm-checksig-verify.log
  if [ "${PIPESTATUS[0]}" -ne 0 ]; then
    echo "::error::rpm --checksig failed on $rpm_path (NOKEY or BAD signature)"
    exit 1
  fi
done

#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd -P)"
PKG_BASE_URL="${PANEFLOW_PKG_BASE_URL:-https://pkg.paneflow.dev}"
CASK_URL="${PANEFLOW_CASK_URL:-https://api.github.com/repos/arthjean/homebrew-paneflow/contents/Casks/paneflow.rb?ref=main}"
RELEASE_KEY="${PANEFLOW_RELEASE_KEY:-$ROOT/packaging/paneflow-release.asc}"
ATTEMPTS="${PANEFLOW_VERIFY_ATTEMPTS:-5}"
INTERVAL="${PANEFLOW_VERIFY_INTERVAL_SECONDS:-60}"
APT_ARCHES=(amd64 arm64)
RPM_ARCHES=(x86_64 aarch64)

usage() {
  echo "usage: $0 [--stream apt|rpm|cask]... <version>" >&2
  echo "  checks that the apt and rpm repositories and the Homebrew cask serve paneflow <version>," >&2
  echo "  with valid repository signatures, bypassing the CDN cache; every stream by default" >&2
  exit 2
}

streams=()
version=""
while (($# > 0)); do
  case "$1" in
    --stream)
      (($# >= 2)) || usage
      case "$2" in
        apt | rpm | cask) streams+=("$2") ;;
        *) echo "unknown stream '$2'" >&2; usage ;;
      esac
      shift 2
      ;;
    -h | --help) usage ;;
    -*) echo "unknown option '$1'" >&2; usage ;;
    *)
      [ -z "$version" ] || usage
      version="${1#v}"
      shift
      ;;
  esac
done
[ -n "$version" ] || usage
if ! [[ "$version" =~ ^[0-9]+\.[0-9]+\.[0-9]+$ ]]; then
  echo "::error::'$version' is not a stable MAJOR.MINOR.PATCH version; prereleases are not published to the streams" >&2
  exit 2
fi
((${#streams[@]} > 0)) || streams=(apt rpm cask)
[[ "$ATTEMPTS" =~ ^[1-9][0-9]*$ ]] || { echo "PANEFLOW_VERIFY_ATTEMPTS must be a positive integer" >&2; exit 2; }
[[ "$INTERVAL" =~ ^[0-9]+$ ]] || { echo "PANEFLOW_VERIFY_INTERVAL_SECONDS must be a non-negative integer" >&2; exit 2; }

work="$(mktemp -d)"
trap 'rm -rf "$work"' EXIT

sha256_of() {
  if command -v sha256sum >/dev/null; then
    sha256sum "$1" | awk '{print $1}'
  else
    shasum -a 256 "$1" | awk '{print $1}'
  fi
}

keyring="$work/release-keyring.gpg"
prepare_keyring() {
  [ -f "$RELEASE_KEY" ] || { echo "::error::repository signing key $RELEASE_KEY is missing"; exit 1; }
  mkdir -p "$work/gnupg"
  chmod 700 "$work/gnupg"
  gpg --batch --quiet --homedir "$work/gnupg" --dearmor --output "$keyring" "$RELEASE_KEY"
}

gpg_verify() {
  gpgv --keyring "$keyring" "$@" >/dev/null 2>&1
}

github_token="${GH_TOKEN:-${GITHUB_TOKEN:-}}"
if [ -z "$github_token" ] && command -v gh >/dev/null; then
  github_token="$(gh auth token 2>/dev/null || true)"
fi

fetch_count=0
fetch() {
  local url="$1" output="$2" separator="?"
  [[ "$url" != *\?* ]] || separator="&"
  fetch_count=$((fetch_count + 1))
  local busted now
  now="$(date +%s)"
  busted="${url}${separator}cb=${now}-$$-$fetch_count"
  local headers=(-H "Cache-Control: no-cache" -H "Pragma: no-cache")
  if [[ "$url" == https://api.github.com/* ]]; then
    headers+=(-H "Accept: application/vnd.github.raw")
    if [ -n "$github_token" ]; then
      headers+=(-H "Authorization: Bearer $github_token")
    fi
  fi
  curl -sSfL --max-time 30 "${headers[@]}" -o "$output" "$busted" 2>"$output.err"
}

problem=""
signature_broken=0

check_apt() {
  local dir="$work/apt" arch listed expected_sha actual_sha found
  rm -rf "$dir"
  mkdir -p "$dir"
  fetch "$PKG_BASE_URL/apt/dists/stable/InRelease" "$dir/InRelease" || { problem="InRelease could not be fetched: $(cat "$dir/InRelease.err")"; return 1; }
  if ! gpg_verify "$dir/InRelease"; then
    problem="InRelease does not carry a valid signature from $(basename "$RELEASE_KEY")"
    signature_broken=1
    return 1
  fi
  local summary=()
  for arch in "${APT_ARCHES[@]}"; do
    listed="main/binary-$arch/Packages.gz"
    expected_sha="$(awk -v file="$listed" '
      /^-----BEGIN PGP SIGNATURE-----/ { exit }
      /^[A-Za-z0-9]+:/ { section = $1; next }
      section == "SHA256:" && $3 == file { print $1; exit }
    ' "$dir/InRelease")"
    if [ -z "$expected_sha" ]; then
      problem="InRelease lists no SHA256 for $listed"
      return 1
    fi
    fetch "$PKG_BASE_URL/apt/dists/stable/$listed" "$dir/Packages-$arch.gz" || { problem="$listed could not be fetched: $(cat "$dir/Packages-$arch.gz.err")"; return 1; }
    actual_sha="$(sha256_of "$dir/Packages-$arch.gz")"
    if [ "$actual_sha" != "$expected_sha" ]; then
      problem="$listed has SHA256 $actual_sha, the signed InRelease expects $expected_sha"
      return 1
    fi
    found="$(gunzip -c "$dir/Packages-$arch.gz" | awk '
      /^Package:/ { package = $2 }
      /^Version:/ && package == "paneflow" { print $2 }
    ')"
    if [ -z "$found" ]; then
      problem="$listed lists no paneflow package"
      return 1
    fi
    if ! grep -qxE "${version//./\\.}(-[0-9]+)?" <<<"$found"; then
      problem="$listed still lists paneflow $(tr '\n' ' ' <<<"$found" | sed 's/ $//'), expected $version"
      return 1
    fi
    summary+=("$arch $(grep -m1 -xE "${version//./\\.}(-[0-9]+)?" <<<"$found")")
  done
  echo "apt: InRelease signature valid; paneflow ${summary[*]}"
}

check_rpm() {
  local dir="$work/rpm" href expected_sha actual_sha arch triples
  rm -rf "$dir"
  mkdir -p "$dir"
  fetch "$PKG_BASE_URL/rpm/repodata/repomd.xml" "$dir/repomd.xml" || { problem="repomd.xml could not be fetched: $(cat "$dir/repomd.xml.err")"; return 1; }
  fetch "$PKG_BASE_URL/rpm/repodata/repomd.xml.asc" "$dir/repomd.xml.asc" || { problem="repomd.xml.asc could not be fetched: $(cat "$dir/repomd.xml.asc.err")"; return 1; }
  if ! gpg_verify "$dir/repomd.xml.asc" "$dir/repomd.xml"; then
    problem="repomd.xml does not carry a valid signature from $(basename "$RELEASE_KEY")"
    signature_broken=1
    return 1
  fi
  read -r expected_sha href < <(awk '
    /<data type="primary">/ { inside = 1; next }
    inside && /<\/data>/ { exit }
    inside && /<checksum type="sha256">/ { sub(/.*<checksum type="sha256">/, ""); sub(/<\/checksum>.*/, ""); sha = $0 }
    inside && /<location href="/ { sub(/.*<location href="/, ""); sub(/".*/, ""); href = $0 }
    END { if (sha != "" && href != "") print sha, href }
  ' "$dir/repomd.xml") || true
  if [ -z "${href:-}" ]; then
    problem="repomd.xml names no primary metadata with a SHA-256"
    return 1
  fi
  fetch "$PKG_BASE_URL/rpm/$href" "$dir/primary.xml.gz" || { problem="$href could not be fetched: $(cat "$dir/primary.xml.gz.err")"; return 1; }
  actual_sha="$(sha256_of "$dir/primary.xml.gz")"
  if [ "$actual_sha" != "$expected_sha" ]; then
    problem="$href has SHA256 $actual_sha, the signed repomd.xml expects $expected_sha"
    return 1
  fi
  triples="$(gunzip -c "$dir/primary.xml.gz" | awk '
    /<name>/ { name = $0; sub(/.*<name>/, "", name); sub(/<\/name>.*/, "", name) }
    /<arch>/ { arch = $0; sub(/.*<arch>/, "", arch); sub(/<\/arch>.*/, "", arch) }
    /<version / { ver = $0; sub(/.* ver="/, "", ver); sub(/".*/, "", ver); print name, arch, ver }
  ')"
  for arch in "${RPM_ARCHES[@]}"; do
    if ! grep -qxF "paneflow $arch $version" <<<"$triples"; then
      problem="repodata lists no paneflow $version for $arch (newest: $(awk -v a="$arch" '$1 == "paneflow" && $2 == a { print $3 }' <<<"$triples" | sort -V | tail -1))"
      return 1
    fi
  done
  echo "rpm: repomd.xml signature valid; paneflow $version on ${RPM_ARCHES[*]}"
}

check_cask() {
  local dir="$work/cask" found sha
  rm -rf "$dir"
  mkdir -p "$dir"
  fetch "$CASK_URL" "$dir/paneflow.rb" || { problem="the cask could not be fetched: $(cat "$dir/paneflow.rb.err")"; return 1; }
  found="$(sed -nE 's/^[[:space:]]*version "([^"]*)".*/\1/p' "$dir/paneflow.rb" | head -1)"
  sha="$(sed -nE 's/^[[:space:]]*sha256 "([^"]*)".*/\1/p' "$dir/paneflow.rb" | head -1)"
  if [ "$found" != "$version" ]; then
    problem="the cask still declares version ${found:-none}, expected $version"
    return 1
  fi
  if ! [[ "$sha" =~ ^[0-9a-f]{64}$ ]] || [[ "$sha" =~ ^0+$ ]]; then
    problem="the cask declares no real sha256 ('$sha')"
    return 1
  fi
  echo "cask: version $version, sha256 $sha"
}

for stream in "${streams[@]}"; do
  if [ "$stream" != cask ]; then
    prepare_keyring
    break
  fi
done

status=0
for stream in "${streams[@]}"; do
  for ((attempt = 1; attempt <= ATTEMPTS; attempt++)); do
    problem=""
    signature_broken=0
    if "check_$stream"; then
      break
    fi
    if ((signature_broken == 1)); then
      echo "::error::$stream stream: $problem"
      status=1
      break
    fi
    if ((attempt == ATTEMPTS)); then
      echo "::error::$stream stream: $problem, after $ATTEMPTS attempt(s) ${INTERVAL} s apart"
      status=1
      break
    fi
    echo "$stream stream, attempt $attempt/$ATTEMPTS: $problem; retrying in ${INTERVAL} s"
    sleep "$INTERVAL"
  done
done
exit "$status"

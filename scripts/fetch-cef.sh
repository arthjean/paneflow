#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd -P)"
MANIFEST="$ROOT/native/cef/manifest.toml"
TARGETS=()
ALL_TARGETS=0

usage() {
  cat <<'USAGE'
usage: fetch-cef.sh [--target <triple>]... [--all] [--manifest <path>]

Places the CEF archive pinned by native/cef/manifest.toml under
native/cef/prebuilt/<target>/ after its SHA-256 matches the manifest.
Without --target or --all, fetches the archive of the host target.

examples:
  scripts/fetch-cef.sh
  scripts/fetch-cef.sh --target aarch64-apple-darwin
  scripts/fetch-cef.sh --all
USAGE
}

manifest_string() {
  sed -n "s/^$1 = \"\(.*\)\"$/\1/p" "$MANIFEST"
}

target_manifest_value() {
  local target="$1"
  local key="$2"
  awk -v target="$target" -v key="$key" '
    $0 == "[targets.\"" target "\"]" { in_target = 1; next }
    in_target && /^\[/ { exit }
    in_target {
      prefix = key " = "
      if (index($0, prefix) == 1) {
        value = substr($0, length(prefix) + 1)
        gsub(/^"|"$/, "", value)
        print value
        found = 1
        exit
      }
    }
    END { if (!found) exit 1 }
  ' "$MANIFEST"
}

manifest_targets() {
  sed -n 's/^\[targets\."\([^"]*\)"\]$/\1/p' "$MANIFEST"
}

host_target() {
  if command -v rustc >/dev/null; then
    rustc -vV | sed -n 's/^host: //p'
    return
  fi
  case "$(uname -s)-$(uname -m)" in
    Linux-x86_64) echo x86_64-unknown-linux-gnu ;;
    Linux-aarch64 | Linux-arm64) echo aarch64-unknown-linux-gnu ;;
    Darwin-arm64) echo aarch64-apple-darwin ;;
    MINGW*-x86_64 | MSYS*-x86_64 | CYGWIN*-x86_64) echo x86_64-pc-windows-msvc ;;
    *) echo "cannot infer the host target from $(uname -s) $(uname -m); pass --target" >&2; return 1 ;;
  esac
}

sha256_of() {
  if command -v sha256sum >/dev/null; then
    sha256sum "$1" | awk '{print $1}'
  else
    shasum -a 256 "$1" | awk '{print $1}'
  fi
}

while (($#)); do
  case "$1" in
    --target)
      [[ $# -ge 2 ]] || { echo "--target requires a Rust target triple" >&2; exit 2; }
      TARGETS+=("$2")
      shift 2
      ;;
    --all)
      ALL_TARGETS=1
      shift
      ;;
    --manifest)
      [[ $# -ge 2 ]] || { echo "--manifest requires a path" >&2; exit 2; }
      MANIFEST="$(cd "$(dirname "$2")" && pwd -P)/$(basename "$2")"
      shift 2
      ;;
    -h | --help)
      usage
      exit 0
      ;;
    *)
      echo "unknown argument: $1" >&2
      usage >&2
      exit 2
      ;;
  esac
done

[[ -f "$MANIFEST" ]] || { echo "manifest not found: $MANIFEST" >&2; exit 1; }
PREBUILT="$(dirname "$MANIFEST")/prebuilt"

if ((ALL_TARGETS)); then
  while IFS= read -r target; do
    TARGETS+=("$target")
  done < <(manifest_targets)
elif ((${#TARGETS[@]} == 0)); then
  TARGETS+=("$(host_target)")
fi

CEF_VERSION="$(manifest_string cef_version)"
[[ -n "$CEF_VERSION" ]] || { echo "manifest must name cef_version" >&2; exit 1; }
command -v curl >/dev/null || { echo "curl is required" >&2; exit 1; }

curl_flags=(
  --fail --silent --show-error --location
  --connect-timeout 30 --max-time 1800
  --retry 5 --retry-delay 5 --retry-all-errors
)
case "$(curl --version)" in
  *Schannel*) curl_flags+=(--ssl-revoke-best-effort) ;;
esac

for target in "${TARGETS[@]}"; do
  url="$(target_manifest_value "$target" archive_url)" || {
    echo "manifest declares no target $target" >&2
    exit 1
  }
  name="$(target_manifest_value "$target" archive_name)"
  expected="$(target_manifest_value "$target" archive_sha256)"
  [[ "$expected" =~ ^[0-9a-f]{64}$ ]] || { echo "$target: archive_sha256 is not a SHA-256" >&2; exit 1; }
  destination="$PREBUILT/$target/$name"
  if [[ -f "$destination" && "$(sha256_of "$destination")" == "$expected" ]]; then
    echo "$target: CEF $CEF_VERSION already in place ($expected)"
    continue
  fi

  partial="$destination.part"
  mkdir -p "$(dirname "$destination")"
  rm -f "$partial" "$destination"
  echo "$target: downloading $url"
  if ! curl "${curl_flags[@]}" --output "$partial" "$url"; then
    rm -f "$partial"
    echo "$target: download failed" >&2
    exit 1
  fi
  actual="$(sha256_of "$partial")"
  if [[ "$actual" != "$expected" ]]; then
    rm -f "$partial"
    echo "$target: downloaded archive hash $actual does not match manifest archive_sha256 $expected" >&2
    exit 1
  fi
  mv -f "$partial" "$destination"
  echo "$target: placed $destination ($expected)"
done

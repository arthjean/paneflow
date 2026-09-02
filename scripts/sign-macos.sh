#!/usr/bin/env bash
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd -P)"
REPO_ROOT="$(cd "$SCRIPT_DIR/.." && pwd -P)"

ENTITLEMENTS="$REPO_ROOT/packaging/macos/paneflow.entitlements"
APP=""

while [ "$#" -gt 0 ]; do
    case "$1" in
        --entitlements)
            [ "$#" -ge 2 ] || { echo "error: --entitlements requires a path" >&2; exit 1; }
            ENTITLEMENTS="$2"
            shift 2
            ;;
        -h|--help)
            sed -n '1,30p' "$0" >&2
            exit 0
            ;;
        --*)
            echo "error: unknown flag: $1" >&2
            exit 1
            ;;
        *)
            [ -z "$APP" ] || { echo "error: unexpected positional arg: $1" >&2; exit 1; }
            APP="$1"
            shift
            ;;
    esac
done

APP="${APP:-dist/PaneFlow.app}"

[ -d "$APP" ] || { echo "error: bundle not found: $APP" >&2; exit 1; }
[ -f "$ENTITLEMENTS" ] || { echo "error: entitlements file not found: $ENTITLEMENTS" >&2; exit 1; }

plutil -lint "$ENTITLEMENTS" >/dev/null

: "${APPLE_DEVELOPER_CERT_P12:?APPLE_DEVELOPER_CERT_P12 env var is required}"
: "${APPLE_DEVELOPER_CERT_PASSWORD:?APPLE_DEVELOPER_CERT_PASSWORD env var is required}"
: "${APPLE_TEAM_ID:?APPLE_TEAM_ID env var is required}"

KEYCHAIN_PASSWORD="$(openssl rand -hex 32)"
KEYCHAIN="build.keychain"
CERT_P12="$(mktemp -t paneflow-cert.XXXXXX).p12"

cleanup() {
    security delete-keychain "$KEYCHAIN" 2>/dev/null || true
    rm -f "$CERT_P12"
}
trap cleanup EXIT

base64 -D > "$CERT_P12" <<< "$APPLE_DEVELOPER_CERT_P12"

security create-keychain -p "$KEYCHAIN_PASSWORD" "$KEYCHAIN"
security set-keychain-settings -lut 3600 "$KEYCHAIN"
security unlock-keychain -p "$KEYCHAIN_PASSWORD" "$KEYCHAIN"

# shellcheck disable=SC2046
security list-keychains -d user -s "$KEYCHAIN" $(security list-keychains -d user | tr -d '"')

security import "$CERT_P12" \
    -k "$KEYCHAIN" \
    -P "$APPLE_DEVELOPER_CERT_PASSWORD" \
    -T /usr/bin/codesign \
    -T /usr/bin/productbuild

security set-key-partition-list \
    -S apple-tool:,apple:,codesign: \
    -s -k "$KEYCHAIN_PASSWORD" \
    "$KEYCHAIN" > /dev/null

IDENTITY="$(security find-identity -v -p codesigning "$KEYCHAIN" \
    | awk -F'"' '/Developer ID Application/ { print $2; exit }')"

if [ -z "$IDENTITY" ]; then
    echo "error: no 'Developer ID Application' identity in $KEYCHAIN" >&2
    echo "  --- keychain contents ---" >&2
    security find-identity -v "$KEYCHAIN" >&2 || true
    exit 1
fi

if [[ "$IDENTITY" != *"($APPLE_TEAM_ID)"* ]]; then
    echo "error: signing identity team ID does not match APPLE_TEAM_ID" >&2
    echo "  identity: $IDENTITY" >&2
    echo "  expected: ...($APPLE_TEAM_ID)" >&2
    exit 1
fi

NESTED_PATTERNS=(
    "Contents/Frameworks"
    "Contents/Helpers"
    "Contents/PlugIns"
    "Contents/XPCServices"
)

for sub in "${NESTED_PATTERNS[@]}"; do
    dir="$APP/$sub"
    [ -d "$dir" ] || continue

    while IFS= read -r -d '' nested; do
        codesign \
            --force \
            --options runtime \
            --timestamp \
            --sign "$IDENTITY" \
            "$nested"
    done < <(find -d "$dir" -name '*.dylib' -print0)

    while IFS= read -r -d '' nested; do
        codesign \
            --force \
            --options runtime \
            --timestamp \
            --entitlements "$ENTITLEMENTS" \
            --sign "$IDENTITY" \
            "$nested"
    done < <(find -d "$dir" \
                \( \
                    \( -name '*.framework' -o -name '*.xpc' \) \
                    -o \
                    \( -type f -perm -u+x ! -name '*.dylib' \) \
                \) -print0)
done

codesign \
    --force \
    --options runtime \
    --timestamp \
    --entitlements "$ENTITLEMENTS" \
    --sign "$IDENTITY" \
    "$APP"

codesign --verify --deep --strict --verbose=2 "$APP"

echo "Signed: $APP ($IDENTITY)"
echo "Entitlements: $ENTITLEMENTS"

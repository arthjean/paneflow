#!/usr/bin/env bash
set -euo pipefail
mode=sync
while (($#)); do
    case "$1" in
        --check) mode=check; shift ;;
        *) printf 'Unknown argument: %s\n' "$1" >&2; exit 2 ;;
    esac
done
crate="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/../crates/paneflow-highlights" && pwd)"
manifest="$crate/c/MANIFEST.toml"
[[ -f "$manifest" ]] || { printf 'Missing %s\n' "$manifest" >&2; exit 1; }

pin() {
    local value
    value="$(awk -v section="$1" -v key="$2" '
        /^\[\[/ { current = "#files"; next }
        /^\[/ { current = substr($0, 2, length($0) - 2); next }
        current == section && $1 == key && $2 == "=" {
            line = $0
            sub(/^[^=]*= *"/, "", line)
            sub(/" *$/, "", line)
            print line
            exit
        }' "$manifest")"
    [[ -n "$value" ]] || { printf 'MANIFEST.toml has no %s.%s\n' "${1:-<root>}" "$2" >&2; exit 1; }
    printf '%s' "$value"
}

sha256_of() {
    local hash
    if command -v sha256sum >/dev/null 2>&1; then
        hash="$(sha256sum "$1")"
    else
        hash="$(shasum -a 256 "$1")"
    fi
    printf '%s' "${hash%% *}"
}

sha512_base64_of() {
    openssl dgst -sha512 -binary "$1" | openssl base64 -A
}

fail_input() {
    printf 'Hash mismatch for %s: expected %s, found %s\n' "$1" "$2" "$3" >&2
    exit 1
}

download() {
    curl --fail --silent --show-error --location --retry 3 --output "$2" "$1" \
        || { printf 'Download failed for %s: %s\n' "$3" "$1" >&2; exit 1; }
}

expect_sha256() {
    local found
    found="$(sha256_of "$2")"
    [[ "$found" == "$3" ]] || fail_input "$1" "$3" "$found"
}

name="$(pin '' name)"
version="$(pin '' version)"
package_url="$(pin package url)"
integrity="$(pin package integrity)"
wasm_member="$(pin package wasm)"
wasm_sha256="$(pin package wasm_sha256)"
wabt_version="$(pin wabt version)"
wabt_url="$(pin wabt url)"
wabt_license_url="$(pin wabt license_url)"
wabt_license_sha256="$(pin wabt license_sha256)"
simde_commit="$(pin simde commit)"
simde_url="$(pin simde url)"
simde_sha256="$(pin simde sha256)"
simde_header="$(pin simde header)"
simde_header_sha256="$(pin simde header_sha256)"
simde_license_url="$(pin simde license_url)"
simde_license_sha256="$(pin simde license_sha256)"

work="$(mktemp -d)"
trap 'rm -rf -- "$work"' EXIT
mkdir -- "$work/package" "$work/wabt" "$work/simde" "$work/licenses" "$work/out"

download "$package_url" "$work/package/package.tgz" "$name $version"
found="sha512-$(sha512_base64_of "$work/package/package.tgz")"
[[ "$found" == "$integrity" ]] || fail_input "$name $version tarball" "$integrity" "$found"
tar -xzf "$work/package/package.tgz" -C "$work/package"
package_version="$(sed -n 's/^  "version": "\([^"]*\)",$/\1/p' "$work/package/package/package.json")"
[[ "$package_version" == "$version" ]] || { printf 'Tarball holds %s %s, MANIFEST.toml pins %s\n' "$name" "$package_version" "$version" >&2; exit 1; }
wasm="$work/package/$wasm_member"
expect_sha256 "$name $version highlights.wasm" "$wasm" "$wasm_sha256"

if [[ -n "${WASM2C:-}" ]]; then
    wasm2c="$WASM2C"
    found="$("$wasm2c" --version 2>/dev/null || true)"
    [[ "$found" == "$wabt_version" ]] || { printf 'WASM2C must be wasm2c %s, found %s\n' "$wabt_version" "${found:-no version}" >&2; exit 1; }
    resolved="$(cd -- "$(dirname -- "$wasm2c")" && pwd -P)/$(basename -- "$wasm2c")"
    wabt_root="$(cd -- "$(dirname -- "$resolved")/.." && pwd)"
else
    case "$(uname -s)-$(uname -m)" in
        Linux-x86_64) host=linux-x64 ;;
        Linux-aarch64 | Linux-arm64) host=linux-arm64 ;;
        Darwin-arm64) host=macos-arm64 ;;
        MINGW*-x86_64 | MSYS*-x86_64 | CYGWIN*-x86_64) host=windows-x64 ;;
        *) printf 'No WABT %s binary for %s %s: set WASM2C\n' "$wabt_version" "$(uname -s)" "$(uname -m)" >&2; exit 1 ;;
    esac
    archive="wabt-$wabt_version-$host.tar.gz"
    download "$wabt_url/$archive" "$work/wabt/$archive" "WABT $wabt_version ($host)"
    download "$wabt_url/$archive.sha256" "$work/wabt/$archive.sha256" "WABT $wabt_version ($host) checksum"
    published="$(awk '{ print $1; exit }' "$work/wabt/$archive.sha256")"
    expect_sha256 "WABT $wabt_version ($host), published checksum" "$work/wabt/$archive" "$published"
    expect_sha256 "WABT $wabt_version ($host)" "$work/wabt/$archive" "$(pin wabt "${host//-/_}_sha256")"
    tar -xzf "$work/wabt/$archive" -C "$work/wabt"
    wabt_root="$work/wabt/wabt-$wabt_version"
    wasm2c="$wabt_root/bin/wasm2c"
    [[ "$host" == windows-x64 ]] && wasm2c="$wasm2c.exe"
    found="$("$wasm2c" --version)"
    [[ "$found" == "$wabt_version" ]] || { printf 'The WABT archive holds wasm2c %s, expected %s\n' "$found" "$wabt_version" >&2; exit 1; }
fi

download "$simde_url" "$work/simde/simde.tar.xz" "SIMDe $simde_commit"
expect_sha256 "SIMDe amalgamated archive" "$work/simde/simde.tar.xz" "$simde_sha256"
tar -xJf "$work/simde/simde.tar.xz" -C "$work/simde" "$simde_header"
expect_sha256 "SIMDe wasm/simd128.h" "$work/simde/$simde_header" "$simde_header_sha256"
[[ "$(sed -n 2p "$work/simde/$simde_header")" == "/* $simde_commit */" ]] || { printf 'SIMDe wasm/simd128.h is not from commit %s\n' "$simde_commit" >&2; exit 1; }

download "$wabt_license_url" "$work/licenses/wabt" "WABT license"
expect_sha256 "WABT license" "$work/licenses/wabt" "$wabt_license_sha256"
download "$simde_license_url" "$work/licenses/simde" "SIMDe license"
expect_sha256 "SIMDe license" "$work/licenses/simde" "$simde_license_sha256"

out="$work/out"
mkdir -p -- "$out/c/generated" "$out/c/wasm2c" "$out/c/simde/wasm" "$out/src"
"$wasm2c" "$wasm" -n highlights -o "$work/highlights.c"
banner="/* Generated by scripts/sync-pierre-highlights from $name $version (highlights.wasm sha256 $wasm_sha256). Do not edit. */"
for file in highlights.c highlights.h; do
    { printf '%s\n' "$banner"; cat -- "$work/$file"; } > "$out/c/generated/$file"
done
cp -- "$work/package/package/LICENSE.md" "$out/c/generated/LICENSE"
runtime=(
    "include/wasm-rt.h:wasm-rt.h"
    "share/wabt/wasm2c/wasm-rt-impl.h:wasm-rt-impl.h"
    "share/wabt/wasm2c/wasm-rt-impl.c:wasm-rt-impl.c"
    "share/wabt/wasm2c/wasm-rt-impl-tableops.inc:wasm-rt-impl-tableops.inc"
    "share/wabt/wasm2c/wasm-rt-mem-impl.c:wasm-rt-mem-impl.c"
    "share/wabt/wasm2c/wasm-rt-mem-impl-helper.inc:wasm-rt-mem-impl-helper.inc"
)
for entry in "${runtime[@]}"; do
    cp -- "$wabt_root/${entry%%:*}" "$out/c/wasm2c/${entry##*:}"
done
cp -- "$work/licenses/wabt" "$out/c/wasm2c/LICENSE"
cp -- "$work/simde/$simde_header" "$out/c/simde/wasm/simd128.h"
cp -- "$work/licenses/simde" "$out/c/simde/LICENSE"

{
    printf '%s, the lexers of the npm package %s %s translated to C by wasm2c %s.\n' "Pierre highlights" "$name" "$version" "$wabt_version"
    printf 'Package: %s\n' "$package_url"
    printf 'Source: %s (%s)\n\n' "$(pin '' source_repository)" "$(pin '' source_path)"
    printf 'The generated code under generated/ is licensed under %s, the license of\n' "$(pin '' license)"
    printf 'the package it is generated from; generated/LICENSE holds its text.\n'
    printf 'The wasm2c runtime under wasm2c/ is part of WABT, licensed under Apache-2.0\n'
    printf '(wasm2c/LICENSE). simde/wasm/simd128.h is SIMDe %s at commit %s,\n' "$(pin simde version)" "$simde_commit"
    printf 'licensed under MIT (simde/LICENSE).\n\n'
    printf 'The NOTICE.md of %s %s follows.\n\n' "$name" "$version"
    cat -- "$work/package/package/NOTICE.md"
} > "$out/c/NOTICE"

languages_js="$work/package/package/dist/languages.js"
tokens_js="$work/package/package/dist/token-types.js"
language_rows="$(sed -n 's/^	"\{0,1\}\([^":]*\)"\{0,1\}: \([0-9][0-9]*\),\{0,1\}$/    ("\1", \2),/p' "$languages_js")"
token_rows="$(sed -n 's/^	"\([^"]*\)",\{0,1\}$/    "\1",/p' "$tokens_js")"
language_keys="$(grep -c '^	' "$languages_js" || true)"
token_keys="$(grep -c '^	"' "$tokens_js" || true)"
[[ -n "$language_rows" && "$(printf '%s\n' "$language_rows" | wc -l | tr -d ' ')" == "$language_keys" ]] \
    || { printf 'Could not read every language of %s/dist/languages.js\n' "$name" >&2; exit 1; }
[[ -n "$token_rows" && "$(printf '%s\n' "$token_rows" | wc -l | tr -d ' ')" == "$token_keys" ]] \
    || { printf 'Could not read every token type of %s/dist/token-types.js\n' "$name" >&2; exit 1; }
if printf '%s\n%s\n' "$language_rows" "$token_rows" | grep -q '\\'; then
    printf 'A language key or token type holds a backslash\n' >&2
    exit 1
fi
{
    printf 'pub(crate) const PACKAGE_VERSION: &str = "%s";\n' "$version"
    printf 'pub(crate) const WASM_SHA256: &str = "%s";\n\n' "$wasm_sha256"
    printf 'pub(crate) const LANGUAGE_KEYS: &[(&str, u8)] = &[\n%s\n];\n\n' "$language_rows"
    printf 'pub(crate) const TOKEN_TYPES: &[&str] = &[\n%s\n];\n' "$token_rows"
} > "$out/src/tables.rs"

files=(
    c/generated/highlights.c
    c/generated/highlights.h
    c/generated/LICENSE
    c/NOTICE
    c/wasm2c/wasm-rt.h
    c/wasm2c/wasm-rt-impl.h
    c/wasm2c/wasm-rt-impl.c
    c/wasm2c/wasm-rt-impl-tableops.inc
    c/wasm2c/wasm-rt-mem-impl.c
    c/wasm2c/wasm-rt-mem-impl-helper.inc
    c/wasm2c/LICENSE
    c/simde/wasm/simd128.h
    c/simde/LICENSE
    src/tables.rs
)
awk '/^\[\[files\]\]$/ { exit } /^$/ { blank++; next } { while (blank > 0) { print ""; blank-- } print }' "$manifest" > "$out/c/MANIFEST.toml"
for file in "${files[@]}"; do
    printf '\n[[files]]\npath = "%s"\nsha256 = "%s"\n' "$file" "$(sha256_of "$out/$file")" >> "$out/c/MANIFEST.toml"
done

if [[ "$mode" == check ]]; then
    failed=0
    while IFS= read -r file; do
        if [[ -n "$file" && ! -f "$crate/$file" ]]; then
            printf 'Missing vendored file: %s\n' "$file" >&2
            failed=1
        fi
    done < <(sed -n 's/^path = "\(.*\)"$/\1/p' "$manifest")
    for file in "${files[@]}" c/MANIFEST.toml; do
        if [[ ! -f "$crate/$file" ]]; then
            grep -qxF "path = \"$file\"" "$manifest" || printf 'Missing vendored file: %s\n' "$file" >&2
            failed=1
        elif ! cmp -s "$out/$file" "$crate/$file"; then
            printf 'Divergent vendored file: %s\n' "$file" >&2
            failed=1
        fi
    done
    ((failed == 0)) || exit 1
    printf '%s %s vendored files verified\n' "$name" "$version"
else
    for file in "${files[@]}" c/MANIFEST.toml; do
        mkdir -p -- "$(dirname -- "$crate/$file")"
        cp -- "$out/$file" "$crate/$file"
    done
    printf '%s %s vendored files written\n' "$name" "$version"
fi

#!/usr/bin/env bash
set -euo pipefail

if [ "$#" -ne 1 ]; then
  echo "usage: stage-windows-assets.sh <version>" >&2
  echo "  copies the single target/wix/paneflow-*.msi and the libghostty provenance report into release-assets/ under their release names, with a .sha256 each" >&2
  exit 2
fi
version="${1#v}"

mkdir -p release-assets
shopt -s nullglob
matches=( target/wix/paneflow-*.msi )
shopt -u nullglob
if [ "${#matches[@]}" -ne 1 ]; then
    printf '::error::Expected exactly one MSI under target/wix/, found %s: %s\n' "${#matches[@]}" "${matches[*]:-<none>}" >&2
    exit 1
fi
src="${matches[0]}"
dst="paneflow-${version}-x86_64-pc-windows-msvc.msi"
provenance_src="target/libghostty-windows-release/provenance.json"
provenance_dst="paneflow-${version}-x86_64-pc-windows-msvc.libghostty-provenance.json"
if [ ! -s "${provenance_src}" ]; then
    echo "::error::Missing Windows libghostty provenance report at ${provenance_src}" >&2
    exit 1
fi
cp "${src}" "release-assets/${dst}"
cp "${provenance_src}" "release-assets/${provenance_dst}"
( cd release-assets \
  && sha256sum "${dst}" > "${dst}.sha256" \
  && sha256sum "${provenance_dst}" > "${provenance_dst}.sha256" )
ls -la release-assets/

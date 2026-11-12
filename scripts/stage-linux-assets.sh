#!/usr/bin/env bash
set -euo pipefail

if [ "$#" -ne 3 ]; then
  echo "usage: stage-linux-assets.sh <version> <arch> <rust-target-triple>" >&2
  echo "  copies the tarball, AppImage, .deb and .rpm of one Linux leg into release-assets/ under their release names, with a .sha256 each" >&2
  exit 2
fi
VERSION="${1#v}"
ARCH="$2"
TARGET="$3"

mkdir -p release-assets
cp "target/bundle/paneflow-${VERSION}-${ARCH}.tar.gz"            "release-assets/"
cp "target/appimage/paneflow-${VERSION}-${ARCH}.AppImage"        "release-assets/"
cp "target/appimage/paneflow-${VERSION}-${ARCH}.AppImage.zsync"  "release-assets/"
cp target/"${TARGET}"/debian/paneflow_*.deb \
   "release-assets/paneflow-${VERSION}-${ARCH}.deb"
cp target/"${TARGET}"/generate-rpm/paneflow-*.rpm \
   "release-assets/paneflow-${VERSION}-${ARCH}.rpm"
for art in \
    "paneflow-${VERSION}-${ARCH}.tar.gz" \
    "paneflow-${VERSION}-${ARCH}.AppImage" \
    "paneflow-${VERSION}-${ARCH}.deb" \
    "paneflow-${VERSION}-${ARCH}.rpm"
do
    if [ ! -s "release-assets/${art}" ]; then
        echo "::error::release-assets/${art} is missing or empty, refusing to hash." >&2
        exit 1
    fi
done
( cd release-assets \
  && sha256sum "paneflow-${VERSION}-${ARCH}.tar.gz" \
       > "paneflow-${VERSION}-${ARCH}.tar.gz.sha256" \
  && sha256sum "paneflow-${VERSION}-${ARCH}.AppImage" \
       > "paneflow-${VERSION}-${ARCH}.AppImage.sha256" \
  && sha256sum "paneflow-${VERSION}-${ARCH}.deb" \
       > "paneflow-${VERSION}-${ARCH}.deb.sha256" \
  && sha256sum "paneflow-${VERSION}-${ARCH}.rpm" \
       > "paneflow-${VERSION}-${ARCH}.rpm.sha256" )
ls -la release-assets/

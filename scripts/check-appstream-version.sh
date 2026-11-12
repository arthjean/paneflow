#!/usr/bin/env bash
set -euo pipefail

workspace_version="$(awk -F'"' '/^version = / { print $2; exit }' Cargo.toml)"
appstream_version="$(awk -F'"' '/<release version=/ { print $2; exit }' assets/io.github.arthurdev44.paneflow.metainfo.xml)"
if [ -z "$workspace_version" ] || [ -z "$appstream_version" ]; then
  echo "::error::could not resolve Cargo.toml or AppStream release version"
  exit 1
fi
if [ "$workspace_version" != "$appstream_version" ]; then
  echo "::error::AppStream top release version ($appstream_version) must match workspace version ($workspace_version)"
  exit 1
fi

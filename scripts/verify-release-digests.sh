#!/usr/bin/env bash
set -euo pipefail

if (($# != 2)); then
  echo "usage: $0 <release-id> <asset-dir>" >&2
  echo "  compares the digest GitHub computed for every asset of the release with the SHA-256 of the local file," >&2
  echo "  and the uploaded asset names with the local ones. Reads GITHUB_REPOSITORY" >&2
  exit 2
fi
release_id="$1"
asset_dir="$2"
: "${GITHUB_REPOSITORY:?GITHUB_REPOSITORY is required}"
[[ "$release_id" =~ ^[0-9]+$ ]] || { echo "::error::release id '$release_id' is not numeric"; exit 2; }
[ -d "$asset_dir" ] || { echo "::error::$asset_dir is not a directory"; exit 2; }

sha256_of() {
  if command -v sha256sum >/dev/null; then
    sha256sum "$1" | awk '{print $1}'
  else
    shasum -a 256 "$1" | awk '{print $1}'
  fi
}

remote="$(gh api --paginate "repos/$GITHUB_REPOSITORY/releases/$release_id/assets?per_page=100" --jq '.[] | "\(.name)\t\(.digest // "")"')"
declare -A remote_digest=()
while IFS=$'\t' read -r name digest; do
  [ -n "$name" ] || continue
  remote_digest["$name"]="$digest"
done <<<"$remote"

status=0
local_count=0
declare -A seen=()
while IFS= read -r -d '' file; do
  name="$(basename "$file")"
  local_count=$((local_count + 1))
  seen["$name"]=1
  expected="sha256:$(sha256_of "$file")"
  if [ -z "${remote_digest[$name]+set}" ]; then
    echo "::error::Digest check failed for $name: the asset is missing from release $release_id"
    status=1
  elif [ -z "${remote_digest[$name]}" ]; then
    echo "::error::Digest check failed for $name: GitHub reports no digest for the uploaded asset"
    status=1
  elif [ "${remote_digest[$name]}" != "$expected" ]; then
    echo "::error::Digest check failed for $name: uploaded ${remote_digest[$name]}, built $expected"
    status=1
  fi
done < <(find "$asset_dir" -maxdepth 1 -type f -print0 | sort -z)

for name in "${!remote_digest[@]}"; do
  if [ -z "${seen[$name]+set}" ]; then
    echo "::error::Digest check failed for $name: release $release_id carries an asset this run did not build"
    status=1
  fi
done

if ((local_count == 0)); then
  echo "::error::$asset_dir holds no asset to compare"
  exit 1
fi
if ((status == 0)); then
  echo "every one of the $local_count uploaded assets matches the SHA-256 of the file this run built"
else
  echo "::error::The draft release $release_id stays in place for inspection; nothing was published."
fi
exit "$status"

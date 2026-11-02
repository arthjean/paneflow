#!/usr/bin/env bash
set -euo pipefail

: "${GITHUB_REPOSITORY:?GITHUB_REPOSITORY is required}"
: "${EVENT_NAME:?EVENT_NAME is required (push or workflow_dispatch)}"

SEMVER_RE='^[0-9]+\.[0-9]+\.[0-9]+(-[0-9A-Za-z]+([.-][0-9A-Za-z]+)*)?(\+[0-9A-Za-z]+([.-][0-9A-Za-z]+)*)?$'
MSI_SMOKE_BASELINE=0.8.0

fail() {
  echo "::error::$1"
  exit 1
}

output() {
  echo "$1=$2"
  if [ -n "${GITHUB_OUTPUT:-}" ]; then
    echo "$1=$2" >> "$GITHUB_OUTPUT"
  fi
}

core_version() {
  if ! [[ "$1" =~ ^([0-9]+)\.([0-9]+)\.([0-9]+) ]]; then
    return 1
  fi
  printf '%d %d %d' "${BASH_REMATCH[1]}" "${BASH_REMATCH[2]}" "${BASH_REMATCH[3]}"
}

version_greater() {
  local a b
  read -r -a a <<< "$(core_version "$1")"
  read -r -a b <<< "$(core_version "$2")"
  local i
  for i in 0 1 2; do
    if [ "${a[$i]}" -gt "${b[$i]}" ]; then
      return 0
    fi
    if [ "${a[$i]}" -lt "${b[$i]}" ]; then
      return 1
    fi
  done
  return 1
}

resolve_named_ref() {
  local ref="$1" listing sha
  listing="$(git ls-remote "https://github.com/$GITHUB_REPOSITORY.git" \
    "refs/heads/$ref" "refs/tags/$ref" "refs/tags/$ref^{}")" ||
    fail "could not list the refs of $GITHUB_REPOSITORY to resolve target_ref '$ref'"
  sha="$(awk -v r="refs/heads/$ref" '$2 == r { print $1; exit }' <<< "$listing")"
  if [ -z "$sha" ]; then
    sha="$(awk -v r="refs/tags/$ref^{}" '$2 == r { print $1; exit }' <<< "$listing")"
  fi
  if [ -z "$sha" ]; then
    sha="$(awk -v r="refs/tags/$ref" '$2 == r { print $1; exit }' <<< "$listing")"
  fi
  [ -n "$sha" ] || fail "target_ref '$ref' is neither a branch nor a tag of $GITHUB_REPOSITORY"
  resolved_sha="$sha"
}

workspace_version_at() {
  local sha="$1" manifest version
  manifest="$(gh api "repos/$GITHUB_REPOSITORY/contents/Cargo.toml?ref=$sha" --jq .content | base64 -d)" ||
    fail "could not read Cargo.toml at $sha"
  version="$(awk '
    /^\[/ { in_package = ($0 == "[workspace.package]"); next }
    in_package && /^version[[:space:]]*=/ { sub(/^version[[:space:]]*=[[:space:]]*"/, ""); sub(/".*$/, ""); print; exit }
  ' <<< "$manifest")"
  [ -n "$version" ] || fail "Cargo.toml at $sha has no [workspace.package] version"
  cargo_version="$version"
}

case "$EVENT_NAME" in
  push)
    : "${PUSH_SHA:?PUSH_SHA is required for a tag push}"
    : "${TAG:?TAG is required for a tag push}"
    [[ "$PUSH_SHA" =~ ^[0-9a-f]{40}$ ]] || fail "the pushed commit '$PUSH_SHA' is not a full commit SHA"
    sha="$PUSH_SHA"
    [[ "$TAG" == v* ]] && [[ "${TAG#v}" =~ $SEMVER_RE ]] ||
      fail "tag '$TAG' is not v followed by a semver version"
    workspace_version_at "$sha"
    [ "${TAG#v}" = "$cargo_version" ] ||
      fail "tag '$TAG' does not match Cargo.toml version $cargo_version at $sha; tag v$cargo_version, or bump Cargo.toml first"
    echo "tag $TAG matches Cargo.toml version $cargo_version"
    ;;
  workflow_dispatch)
    target="${TARGET_REF:-main}"
    if [[ "$target" =~ ^[0-9a-f]{40}$ ]]; then
      sha="$(gh api "repos/$GITHUB_REPOSITORY/commits/$target" --jq .sha 2>/dev/null)" ||
        fail "target_ref $target is not a commit of $GITHUB_REPOSITORY"
      [ "$sha" = "$target" ] || fail "target_ref $target resolved to $sha"
    elif [[ "$target" =~ ^[0-9a-f]{7,39}$ ]]; then
      fail "target_ref '$target' looks like an abbreviated commit SHA; pass the full 40-character SHA, a branch, or a tag"
    else
      resolve_named_ref "$target"
      sha="$resolved_sha"
    fi
    echo "target_ref $target resolves to $sha"

    : "${VERSION:?VERSION is required for a dispatch}"
    [[ "$VERSION" =~ $SEMVER_RE ]] ||
      fail "version '$VERSION' is not a semver version (no v prefix, e.g. 0.2.12-rc.1)"
    msi_version="${MSI_UPGRADE_VERSION:-}"
    if [ -n "$msi_version" ] && ! [[ "$msi_version" =~ ^[0-9]+\.[0-9]+\.[0-9]+$ ]]; then
      fail "msi_upgrade_version '$msi_version' must be x.y.z"
    fi
    msi_version="${msi_version:-$VERSION}"
    if [ "${MSI_SMOKES:-false}" = true ]; then
      version_greater "$msi_version" "$MSI_SMOKE_BASELINE" ||
        fail "MSI ProductVersion $msi_version does not upgrade the pinned v$MSI_SMOKE_BASELINE baseline of the MSI smoke"
      latest="$(gh api "repos/$GITHUB_REPOSITORY/releases/latest" --jq .tag_name)" ||
        fail "could not read the latest release of $GITHUB_REPOSITORY"
      version_greater "$msi_version" "${latest#v}" ||
        fail "MSI ProductVersion $msi_version does not upgrade the latest release ${latest#v}; dispatch with msi_upgrade_version above ${latest#v}"
      echo "MSI ProductVersion $msi_version upgrades the latest release ${latest#v}"
    fi
    ;;
  *)
    fail "unsupported event '$EVENT_NAME'"
    ;;
esac

output sha "$sha"

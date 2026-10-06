#!/usr/bin/env bash
set -euo pipefail

cd "$(dirname "$0")/.."
root=$(pwd)

for variable in $(compgen -e | grep '^PANEFLOW_' || true); do
  case "$variable" in
    PANEFLOW_AB_PROMOTION_DIR) ;;
    *) unset "$variable" ;;
  esac
done

out="${PANEFLOW_AB_PROMOTION_DIR:-$root/target/perf-ab-promotion}"
runs="$out/runs"
mkdir -p "$runs"

gh run list --workflow perf-ab.yml --status completed --limit 1000 \
  --json databaseId,conclusion,updatedAt \
  --jq '.[] | select(.conclusion == "success" or .conclusion == "failure") | "\(.databaseId) \(.updatedAt | fromdateiso8601)"' |
  while read -r run completed; do
    dir="$runs/$run"
    if [ -d "$dir" ] && [ ! -e "$dir/artifact-expired" ]; then
      continue
    fi
    expired=$(gh api "repos/{owner}/{repo}/actions/runs/$run/artifacts" \
      --jq '.artifacts[] | select(.name == "perf-ab") | .expired')
    rm -rf "$dir" "$dir.partial"
    case "$expired" in
      false)
        gh run download "$run" -n perf-ab -D "$dir.partial"
        mv "$dir.partial" "$dir"
        ;;
      true)
        mkdir -p "$dir"
        echo "$completed" > "$dir/artifact-expired"
        ;;
      *)
        mkdir -p "$dir"
        : > "$dir/no-artifact"
        ;;
    esac
    echo "== run $run"
  done

PANEFLOW_AB_PROMOTION_DIR="$out" cargo test --locked -q -p paneflow-host --test persistent_baseline \
  -- ab::perf_ab_promotion --ignored --exact --nocapture --test-threads=1

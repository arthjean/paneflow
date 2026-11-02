#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd -P)"
WORKFLOW="$ROOT/.github/workflows/run_tests.yml"
REQUIRED_JOBS=(check_style run_tests_linux linux_aarch64_check macos_check windows_check perf_gates security_audit)
POLL_SECONDS="${CI_VERDICT_POLL_SECONDS:-60}"
TIMEOUT_SECONDS="${CI_VERDICT_TIMEOUT_SECONDS:-3600}"

if [ "$#" -ne 2 ]; then
  echo "usage: require-ci-verdict.sh <full commit sha> <ref to re-run run_tests on>" >&2
  exit 2
fi
SHA="$1"
RESUME_REF="$2"
: "${GITHUB_REPOSITORY:?GITHUB_REPOSITORY is required}"
[[ "$SHA" =~ ^[0-9a-f]{40}$ ]] || { echo "::error::'$SHA' is not a full commit SHA"; exit 2; }
RESUME="gh workflow run run_tests.yml --ref $RESUME_REF"

job_name() {
  local id="$1" line in_job=0
  while IFS= read -r line || [ -n "$line" ]; do
    line="${line%$'\r'}"
    if [ "$in_job" -eq 0 ]; then
      [ "$line" = "  $id:" ] && in_job=1
      continue
    fi
    if [[ "$line" =~ ^\ {2}[A-Za-z0-9_-]+:[[:space:]]*$ ]]; then
      break
    fi
    if [[ "$line" =~ ^\ {4}name:[[:space:]]*(.+)$ ]]; then
      local name="${BASH_REMATCH[1]}"
      name="${name%"${name##*[![:space:]]}"}"
      name="${name#[\"\']}"
      name="${name%[\"\']}"
      printf '%s' "$name"
      return 0
    fi
  done < "$WORKFLOW"
  return 1
}

names=()
for id in "${REQUIRED_JOBS[@]}"; do
  if ! name="$(job_name "$id")"; then
    echo "::error::run_tests.yml has no job '$id' with a name; the required job list in scripts/require-ci-verdict.sh is stale"
    exit 1
  fi
  names+=("$name")
done
echo "required run_tests jobs on $SHA:"
printf '  %s\n' "${names[@]}"

deadline=$((SECONDS + TIMEOUT_SECONDS))
last_error=""
while :; do
  if runs="$(gh api --paginate "repos/$GITHUB_REPOSITORY/commits/$SHA/check-runs?per_page=100&filter=latest" \
    --jq '.check_runs[] | [.name, .status, (.conclusion // ""), (.completed_at // ""), .html_url] | @tsv' 2>&1)"; then
    last_error=""
    done_count=0
    for name in "${names[@]}"; do
      rows="$(awk -F'\t' -v n="$name" '$1 == n' <<< "$runs")"
      if [ -z "$rows" ] || awk -F'\t' '$2 != "completed" { found = 1 } END { exit !found }' <<< "$rows"; then
        continue
      fi
      latest="$(sort -t $'\t' -k4,4 <<< "$rows" | tail -n 1)"
      IFS=$'\t' read -r _ _ conclusion _ url <<< "$latest"
      if [ "$conclusion" != success ]; then
        echo "::error::Required job $name was ${conclusion:-without a conclusion} on $SHA: $url. Run: $RESUME, then re-run this release"
        exit 1
      fi
      done_count=$((done_count + 1))
    done
    if [ "$done_count" -eq "${#names[@]}" ]; then
      echo "every required run_tests job is success on $SHA"
      exit 0
    fi
    echo "Waiting for run_tests on $SHA: $done_count of ${#names[@]} required jobs done"
  else
    last_error="$runs"
    echo "::warning::Could not read check runs for $SHA: $last_error"
  fi
  if [ "$SECONDS" -ge "$deadline" ]; then
    if [ -n "$last_error" ]; then
      echo "::error::Could not read check runs for $SHA: $last_error"
    else
      echo "::error::run_tests did not finish every required job on $SHA within $((TIMEOUT_SECONDS / 60)) min. Run: $RESUME, then re-run this release"
    fi
    exit 1
  fi
  sleep "$POLL_SECONDS"
done

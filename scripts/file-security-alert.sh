#!/usr/bin/env bash
set -euo pipefail

LABEL="type: security"

if (($# != 3)); then
  echo "usage: $0 <title> <body-file> <comment>" >&2
  exit 2
fi

title="$1"
body_file="$2"
comment="$3"
repo="${GITHUB_REPOSITORY:?GITHUB_REPOSITORY must name the owner/repo to file in}"

summary() {
  if [ -n "${GITHUB_STEP_SUMMARY:-}" ]; then
    printf '%s\n' "$1" >> "$GITHUB_STEP_SUMMARY"
  fi
}

not_filed() {
  local reason
  reason="$(printf '%s' "$1" | tr '\n' ' ')"
  echo "::error::Security alert was NOT filed: ${reason}"
  summary "- **Security alert was NOT filed** for \`${title}\`: ${reason}"
  exit 1
}

if [ ! -s "$body_file" ]; then
  not_filed "body file '$body_file' is missing or empty"
fi

err="$(mktemp)"
trap 'rm -f "$err"' EXIT

if ! listing="$(gh issue list --repo "$repo" --state open --label "$LABEL" --limit 500 --json number,title 2>"$err")"; then
  not_filed "gh issue list failed: $(cat "$err")"
fi
existing="$(jq -r --arg title "$title" 'map(select(.title == $title)) | .[0].number // empty' <<<"$listing")"

if [ -z "$existing" ]; then
  if ! url="$(gh issue create --repo "$repo" --title "$title" --label "$LABEL" --body-file "$body_file" 2>"$err")"; then
    not_filed "gh issue create failed: $(cat "$err")"
  fi
  echo "filed ${url}"
  summary "- Security alert filed: ${url}"
else
  if ! url="$(gh issue comment "$existing" --repo "$repo" --body "$comment" 2>"$err")"; then
    not_filed "gh issue comment on #${existing} failed: $(cat "$err")"
  fi
  echo "commented on #${existing}: ${url}"
  summary "- Security alert commented on #${existing}: ${url}"
fi

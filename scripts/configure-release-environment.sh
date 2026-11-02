#!/usr/bin/env bash
set -euo pipefail

ENVIRONMENT=release
SECRETS=(
  APPLE_APP_SPECIFIC_PASSWORD
  APPLE_DEVELOPER_CERT_P12
  APPLE_DEVELOPER_CERT_PASSWORD
  APPLE_ID
  APPLE_TEAM_ID
  AZURE_CLIENT_ID
  AZURE_CLIENT_SECRET
  AZURE_TENANT_ID
  AZURE_TRUSTED_SIGNING_ACCOUNT
  AZURE_TRUSTED_SIGNING_CERT_PROFILE
  AZURE_TRUSTED_SIGNING_ENDPOINT
  CLOUDFLARE_AUTH_KEY
  CLOUDFLARE_ZONE
  GPG_KEY_ID
  GPG_PASSPHRASE
  GPG_PRIVATE_KEY
  HOMEBREW_TAP_DEPLOY_KEY
  MINISIGN_SECRET_KEY
  R2_ACCESS_KEY_ID
  R2_BUCKET
  R2_ENDPOINT
  R2_SECRET_ACCESS_KEY
)

usage() {
  cat >&2 <<EOF
usage: $0 status | policy

  status  read-only: show the deployment policy of the '$ENVIRONMENT' environment and,
          for each signing or publishing secret, whether it lives in the repository,
          in the environment, or both, with the command that completes the move
  policy  restrict '$ENVIRONMENT' to the branch 'main' and the tags 'v*' (idempotent)
EOF
  exit 2
}

repo="${GITHUB_REPOSITORY:-$(gh repo view --json nameWithOwner --jq .nameWithOwner)}"

policies() {
  gh api "repos/$repo/environments/$ENVIRONMENT/deployment-branch-policies" \
    --jq '.branch_policies[] | "\(.type) \(.name)"'
}

apply_policy() {
  echo "+ gh api --method PUT repos/$repo/environments/$ENVIRONMENT (custom deployment branch policies)"
  gh api --method PUT "repos/$repo/environments/$ENVIRONMENT" --input - >/dev/null <<'JSON'
{"deployment_branch_policy":{"protected_branches":false,"custom_branch_policies":true}}
JSON
  local existing
  existing="$(policies)"
  local type name
  for entry in "branch main" "tag v*"; do
    type="${entry%% *}"
    name="${entry#* }"
    if grep -qxF "$entry" <<<"$existing"; then
      echo "= $type '$name' is already allowed"
      continue
    fi
    echo "+ gh api --method POST repos/$repo/environments/$ENVIRONMENT/deployment-branch-policies -f name='$name' -f type=$type"
    gh api --method POST "repos/$repo/environments/$ENVIRONMENT/deployment-branch-policies" \
      -f name="$name" -f type="$type" >/dev/null
  done
  policies | sed 's/^/  allowed: /'
}

show_status() {
  local policy
  policy="$(gh api "repos/$repo/environments/$ENVIRONMENT" --jq '.deployment_branch_policy // "none"')"
  echo "environment '$ENVIRONMENT' deployment policy: $policy"
  if [ "$policy" != none ]; then
    policies | sed 's/^/  allowed: /'
  fi
  echo

  local repo_names env_names
  repo_names="$(gh secret list --repo "$repo" --json name --jq '.[].name')"
  env_names="$(gh secret list --repo "$repo" --env "$ENVIRONMENT" --json name --jq '.[].name')"

  local to_create=() to_delete=() in_repo in_env
  printf '%-36s %-6s %-6s\n' SECRET REPO ENV
  for name in "${SECRETS[@]}"; do
    in_repo=no
    in_env=no
    grep -qxF "$name" <<<"$repo_names" && in_repo=yes
    grep -qxF "$name" <<<"$env_names" && in_env=yes
    printf '%-36s %-6s %-6s\n' "$name" "$in_repo" "$in_env"
    if [ "$in_env" = no ]; then
      to_create+=("$name")
    elif [ "$in_repo" = yes ]; then
      to_delete+=("$name")
    fi
  done

  echo
  if [ "${#to_create[@]}" -gt 0 ]; then
    echo "1. Recreate in the '$ENVIRONMENT' environment (each command reads the value from stdin):"
    for name in "${to_create[@]}"; do
      echo "   gh secret set $name --repo $repo --env $ENVIRONMENT"
    done
  else
    echo "1. Every secret already exists in the '$ENVIRONMENT' environment."
  fi
  if [ "${#to_delete[@]}" -gt 0 ]; then
    echo "2. Only after a release.yml dry-run on main signs every leg with the environment copies, delete the repository copies:"
    for name in "${to_delete[@]}"; do
      echo "   gh secret delete $name --repo $repo"
    done
  fi
}

case "${1:-}" in
  status) show_status ;;
  policy) apply_policy ;;
  *) usage ;;
esac

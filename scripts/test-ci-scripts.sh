#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd -P)"
WORK="$(mktemp -d)"
trap 'rm -rf "$WORK"' EXIT

PINNED_CACHE="Swatinem/rust-cache@d1527c8f67ef9af357290475d4af4b0fab8f12e0"
PINNED_CHECKOUT="actions/checkout@93cb6efe18208431cddfb8368fd83d5badbf9bfd"
FAKE_COMMIT="1111111111111111111111111111111111111111"
FAKE_TAG_OBJECT="2222222222222222222222222222222222222222"
FAKE_TAGGED_COMMIT="3333333333333333333333333333333333333333"
export CHECK_WORKFLOWS_REMOTE=off
cases=0
failures=0

expect() {
  local name="$1" expected_status="$2" pattern="$3"
  shift 3
  local output status=0
  output="$("$@" 2>&1)" || status=$?
  cases=$((cases + 1))
  if [ "$status" -ne "$expected_status" ]; then
    echo "FAIL $name: exit $status, expected $expected_status"
    printf '%s\n' "$output" | sed 's/^/    /'
    failures=$((failures + 1))
    return
  fi
  if [ -n "$pattern" ] && ! grep -qE -- "$pattern" <<<"$output"; then
    echo "FAIL $name: output does not match /$pattern/"
    printf '%s\n' "$output" | sed 's/^/    /'
    failures=$((failures + 1))
    return
  fi
  echo "ok   $name"
}

workflow() {
  local path="$WORK/$1"
  mkdir -p "$(dirname "$path")"
  cat > "$path"
  printf '%s' "$path"
}

check_workflows_cases() {
  local check="$ROOT/scripts/check-workflows.sh"
  local file

  file="$(workflow cache/saves-on-main.yml <<EOF
jobs:
  build:
    runs-on: ubuntu-24.04
    timeout-minutes: 10
    steps:
      - uses: $PINNED_CACHE
        with:
          key: build
          save-if: \${{ github.ref == 'refs/heads/main' }}
EOF
)"
  expect "a rust-cache step that saves from main only passes" 0 "saves from main only \(1 step" "$check" "$file"

  file="$(workflow cache/no-save-if.yml <<EOF
jobs:
  build:
    runs-on: ubuntu-24.04
    timeout-minutes: 10
    steps:
      - uses: actions/checkout@v5
      - uses: $PINNED_CACHE
        with:
          key: build
      - run: cargo build --locked
EOF
)"
  expect "a rust-cache step without save-if is named by file and line" 1 "no-save-if\.yml:7: Swatinem/rust-cache must declare save-if" "$check" "$file"

  file="$(workflow cache/named-step.yml <<EOF
jobs:
  build:
    runs-on: ubuntu-24.04
    timeout-minutes: 10
    steps:
      - name: Cache
        uses: $PINNED_CACHE
      - name: Unrelated step
        with:
          save-if: \${{ github.ref == 'refs/heads/main' }}
EOF
)"
  expect "a save-if on the next step does not cover the cache step" 1 "named-step\.yml:7: Swatinem/rust-cache must declare save-if" "$check" "$file"

  file="$(workflow cache/saves-everywhere.yml <<EOF
jobs:
  build:
    runs-on: ubuntu-24.04
    timeout-minutes: 10
    steps:
      - uses: $PINNED_CACHE
        with:
          save-if: \${{ github.event_name != 'pull_request' }}
EOF
)"
  expect "a save-if that also saves from a tag is refused" 1 "saves-everywhere\.yml:6: Swatinem/rust-cache must declare save-if" "$check" "$file"

  file="$(workflow release-cached/release.yml <<EOF
cache-mode: none
jobs:
  build:
    runs-on: ubuntu-24.04
    timeout-minutes: 10
    steps:
      - uses: $PINNED_CACHE
        with:
          save-if: \${{ github.ref == 'refs/heads/main' }}
EOF
)"
  expect "a rust-cache step in the release workflow is refused" 1 "release\.yml:7: the release workflow builds without a cache" "$check" "$file"

  file="$(workflow release-open/release.yml <<EOF
jobs:
  build:
    runs-on: ubuntu-24.04
    timeout-minutes: 10
    steps:
      - uses: actions/checkout@v5
EOF
)"
  expect "a release workflow without cache-mode: none is refused" 1 "release\.yml:1: the release workflow must declare cache-mode: none" "$check" "$file"

  file="$(workflow timeout/unbounded.yml <<EOF
jobs:
  bounded:
    runs-on: ubuntu-24.04
    timeout-minutes: 10
    steps:
      - uses: actions/checkout@v5
  unbounded:
    runs-on: ubuntu-24.04
    steps:
      - uses: actions/checkout@v5
        timeout-minutes: 5
EOF
)"
  expect "a job without timeout-minutes is named by file and line" 1 "unbounded\.yml:7: job 'unbounded' declares no timeout-minutes" "$check" "$file"

  file="$(workflow timeout/last-job.yml <<EOF
jobs:
  only:
    runs-on: ubuntu-24.04
    steps:
      - uses: actions/checkout@v5
EOF
)"
  expect "the last job of a file is checked too" 1 "last-job\.yml:2: job 'only' declares no timeout-minutes" "$check" "$file"

  file="$(workflow timeout/bounded.yml <<EOF
jobs:
  build:
    runs-on: ubuntu-24.04
    timeout-minutes: 10
    steps: &steps
      - uses: $PINNED_CHECKOUT
  again:
    runs-on: ubuntu-24.04
    timeout-minutes: 10
    steps: *steps
  called:
    uses: ./.github/workflows/other.yml
EOF
)"
  expect "bounded jobs and a reusable workflow call pass" 0 "every job declares timeout-minutes \(3 job" "$check" "$file"
}

fake_gh() {
  local bin="$WORK/fake-gh-bin"
  mkdir -p "$bin"
  cat > "$bin/gh" <<EOF
#!/usr/bin/env bash
case "\$*" in
  "auth status") exit 0 ;;
  *"/git/commits/$FAKE_COMMIT"*) exit 0 ;;
  *"/git/tags/$FAKE_TAG_OBJECT"*) echo "$FAKE_TAGGED_COMMIT"; exit 0 ;;
  *) echo "gh: Not Found (HTTP 404)" >&2; exit 1 ;;
esac
EOF
  chmod +x "$bin/gh"
  printf '%s' "$bin"
}

action_pin_cases() {
  local check="$ROOT/scripts/check-workflows.sh"
  local file gh_bin
  gh_bin="$(fake_gh)"

  file="$(workflow pins/tag-ref.yml <<EOF
jobs:
  build:
    runs-on: ubuntu-24.04
    timeout-minutes: 10
    steps:
      - uses: foo/bar@v1
EOF
)"
  expect "a third-party action on a tag ref is named by file and line" 1 "tag-ref\.yml:6: action 'foo/bar@v1' must be pinned to a full commit SHA" "$check" "$file"

  file="$(workflow pins/first-party-tag.yml <<EOF
jobs:
  build:
    runs-on: ubuntu-24.04
    timeout-minutes: 10
    steps:
      - uses: $PINNED_CHECKOUT # v5.0.1
      - uses: actions/download-artifact@v4
EOF
)"
  expect "an actions/* action on a tag ref is refused too" 1 "first-party-tag\.yml:7: action 'actions/download-artifact@v4' must be pinned" "$check" "$file"

  file="$(workflow pins/.github/actions/composite/action.yml <<EOF
runs:
  using: composite
  steps:
    - uses: dtolnay/rust-toolchain@stable
EOF
)"
  expect "a composite action is scanned for tag refs" 1 "action\.yml:4: action 'dtolnay/rust-toolchain@stable' must be pinned" "$check" "$file"

  file="$(workflow pins/local.yml <<EOF
jobs:
  build:
    runs-on: ubuntu-24.04
    timeout-minutes: 10
    steps:
      - uses: ./.github/actions/fetch-libghostty
      - uses: owner/repo/sub/path@$FAKE_COMMIT # v1.2.3
EOF
)"
  expect "a local action and a SHA-pinned sub-path action pass" 0 "every pinned SHA is a commit of its repository \(1 distinct" \
    env PATH="$gh_bin:$PATH" CHECK_WORKFLOWS_REMOTE=required "$check" "$file"

  file="$(workflow pins/tag-object.yml <<EOF
jobs:
  build:
    runs-on: ubuntu-24.04
    timeout-minutes: 10
    steps:
      - uses: owner/cache@$FAKE_TAG_OBJECT # v2
EOF
)"
  expect "an annotated tag object SHA is refused and the commit it points to is named" 1 \
    "tag-object\.yml:6: owner/cache@$FAKE_TAG_OBJECT is an annotated tag object, not a commit; pin the commit it points to, $FAKE_TAGGED_COMMIT" \
    env PATH="$gh_bin:$PATH" CHECK_WORKFLOWS_REMOTE=required "$check" "$file"

  file="$(workflow pins/unknown.yml <<EOF
jobs:
  build:
    runs-on: ubuntu-24.04
    timeout-minutes: 10
    steps:
      - uses: owner/gone@4444444444444444444444444444444444444444
EOF
)"
  expect "a SHA that is not an object of the repository is refused" 1 "unknown\.yml:6: owner/gone@4444444444444444444444444444444444444444 is not a commit of owner/gone" \
    env PATH="$gh_bin:$PATH" CHECK_WORKFLOWS_REMOTE=required "$check" "$file"

  expect "an unknown CHECK_WORKFLOWS_REMOTE value is refused" 2 "CHECK_WORKFLOWS_REMOTE must be auto, required, or off" \
    env CHECK_WORKFLOWS_REMOTE=sometimes "$check" "$file"
}

retry_cases() {
  local retry="$ROOT/scripts/ci-retry-network.sh"
  local fake="$WORK/fake-command.sh"
  cat > "$fake" <<'EOF'
#!/usr/bin/env bash
counter="$1"
failures="$2"
message="$3"
runs=0
[ -f "$counter" ] && runs="$(cat "$counter")"
runs=$((runs + 1))
echo "$runs" > "$counter"
if [ "$runs" -le "$failures" ]; then
  echo "$message"
  exit 7
fi
echo "command output"
EOF
  chmod +x "$fake"

  local signature index=0 counter listed
  listed="$("$retry" --list-transient)"
  while IFS= read -r signature; do
    index=$((index + 1))
    counter="$WORK/retry-count-$index"
    expect "the transient signature '$signature' is retried" 0 "succeeded on attempt 2 after a transient network failure" \
      env CI_RETRY_BACKOFF_SECONDS=0 "$retry" "$fake" "$counter" 1 "error: $signature while fetching"
    cases=$((cases + 1))
    if [ "$(cat "$counter")" != "2" ]; then
      echo "FAIL the transient signature '$signature' ran the command $(cat "$counter") time(s), expected 2"
      failures=$((failures + 1))
    fi
  done <<<"$listed"
  cases=$((cases + 1))
  if [ "$index" -lt 20 ]; then
    echo "FAIL --list-transient printed $index signature(s), expected at least 20"
    failures=$((failures + 1))
  fi
  for signature in ReadFailed ProtocolError; do
    cases=$((cases + 1))
    if ! grep -qFx "$signature" <<<"$listed"; then
      echo "FAIL the Zig signature $signature is not in the transient list"
      failures=$((failures + 1))
    fi
  done

  counter="$WORK/retry-count-real-error"
  expect "a failure with no transient signature is not retried and keeps its exit code" 7 "no transient network signature; not retrying" \
    env CI_RETRY_BACKOFF_SECONDS=0 "$retry" "$fake" "$counter" 1 "error[vulnerability]: a real advisory"
  cases=$((cases + 1))
  if [ "$(cat "$counter")" != "1" ]; then
    echo "FAIL a failure with no transient signature ran the command $(cat "$counter") time(s), expected 1"
    failures=$((failures + 1))
  fi

  counter="$WORK/retry-count-exhausted"
  expect "a transient failure that never clears fails after the last attempt" 7 "still failing on a transient network error after 3 attempts" \
    env CI_RETRY_BACKOFF_SECONDS=0 "$retry" "$fake" "$counter" 9 "503 Service Unavailable"
  cases=$((cases + 1))
  if [ "$(cat "$counter")" != "3" ]; then
    echo "FAIL a transient failure that never clears ran the command $(cat "$counter") time(s), expected 3"
    failures=$((failures + 1))
  fi

  counter="$WORK/retry-count-success"
  expect "a command that succeeds runs once" 0 "command output" \
    env CI_RETRY_BACKOFF_SECONDS=0 "$retry" "$fake" "$counter" 0 "unused"
}

helper_dir() {
  local dir="$WORK/helpers/$1"
  mkdir -p "$dir"
  head -c "$2" /dev/zero > "$dir/paneflow-shim"
  head -c "$3" /dev/zero > "$dir/paneflow-ai-hook"
  head -c "$4" /dev/zero > "$dir/paneflow-mcp"
  printf '%s' "$dir"
}

helper_size_cases() {
  local check="$ROOT/scripts/check-helper-sizes.sh"
  local dir

  dir="$(helper_dir within 400000 300000 400000)"
  expect "helpers under every cap pass" 0 "embedded helpers = 1100000 B" "$check" "$dir"

  dir="$(helper_dir hook-over 400000 384001 400000)"
  expect "a helper over its cap is named with its size and the cap" 1 "paneflow-ai-hook is 384001 B, over its hard cap of 384000 B" "$check" "$dir"

  dir="$(helper_dir at-caps 524288 384000 524288)"
  expect "helpers at their caps still fit the total cap" 0 "embedded helpers = 1432576 B" "$check" "$dir"

  dir="$(helper_dir near-cap 400000 383000 400000)"
  expect "a helper within 4 KiB of its cap warns and passes" 0 "::warning::paneflow-ai-hook is 383000 B, only 1000 B under its hard cap" "$check" "$dir"

  dir="$(helper_dir missing 400000 300000 400000)"
  rm "$dir/paneflow-mcp"
  expect "a missing helper fails instead of passing an unmeasured build" 1 "paneflow-mcp is missing" "$check" "$dir"

  dir="$(helper_dir stale-total 400000 300000 400000)"
  printf 'const OTHER: u64 = 1;\n' > "$WORK/build.rs"
  expect "an unreadable total cap fails" 1 "could not read EMBED_SIZE_LIMIT_BYTES" env PANEFLOW_BUILD_RS="$WORK/build.rs" "$check" "$dir"

  printf 'const EMBED_SIZE_LIMIT_BYTES: u64 = 1_000_000;\n' > "$WORK/build.rs"
  expect "helpers over the total cap of build.rs fail" 1 "the embedded helpers total 1100000 B, over the hard cap of 1000000 B" env PANEFLOW_BUILD_RS="$WORK/build.rs" "$check" "$dir"
}

fake_objdump() {
  local path="$WORK/objdump-$1"
  {
    echo '#!/usr/bin/env bash'
    echo 'cat "$OBJDUMP_FIXTURE"'
  } > "$path"
  chmod +x "$path"
  printf '%s' "$path"
}

glibc_floor_cases() {
  local check="$ROOT/scripts/check-glibc-floor.sh"
  local objdump binary fixture
  objdump="$(fake_objdump canned)"
  binary="$WORK/paneflow-elf"
  : > "$binary"

  fixture="$WORK/symbols-at-floor.txt"
  cat > "$fixture" <<'SYMBOLS'
0000000000000000      DF *UND*	0000000000000000 (GLIBC_2.2.5) memcpy
0000000000000000      DF *UND*	0000000000000000 (GLIBC_2.34) __libc_start_main
0000000000000000      DF *UND*	0000000000000000 (GLIBC_2.35) __memcpy_chk
0000000000000000  w   D  *UND*	0000000000000000  Base        __gmon_start__
SYMBOLS
  expect "a binary whose highest glibc symbol is the floor passes" 0 "paneflow-elf: highest glibc symbol version 2\.35 \(floor 2\.35\)" env OBJDUMP="$objdump" OBJDUMP_FIXTURE="$fixture" "$check" 2.35 "$binary"

  fixture="$WORK/symbols-over-floor.txt"
  cat > "$fixture" <<'SYMBOLS'
0000000000000000      DF *UND*	0000000000000000 (GLIBC_2.34) __libc_start_main
0000000000000000      DF *UND*	0000000000000000 (GLIBC_2.36) arc4random
0000000000000000      DF *UND*	0000000000000000 (GLIBC_2.38) __isoc23_strtol
SYMBOLS
  expect "a binary that needs GLIBC_2.36 over a 2.35 floor fails and names the symbol" 1 "paneflow-elf needs GLIBC_2\.36 for arc4random, above the glibc 2\.35 floor" env OBJDUMP="$objdump" OBJDUMP_FIXTURE="$fixture" "$check" 2.35 "$binary"
  expect "every symbol over the floor is named" 1 "needs GLIBC_2\.38 for __isoc23_strtol" env OBJDUMP="$objdump" OBJDUMP_FIXTURE="$fixture" "$check" 2.35 "$binary"
  expect "the same binary passes a floor raised to 2.39" 0 "highest glibc symbol version 2\.38 \(floor 2\.39\)" env OBJDUMP="$objdump" OBJDUMP_FIXTURE="$fixture" "$check" 2.39 "$binary"

  fixture="$WORK/symbols-old-binutils.txt"
  cat > "$fixture" <<'SYMBOLS'
0000000000000000      DF *UND*	0000000000000000  GLIBC_2.17  memcpy
0000000000000000      DF *UND*	0000000000000000  GLIBC_2.36  arc4random
SYMBOLS
  expect "a version printed without parentheses is read too" 1 "needs GLIBC_2\.36 for arc4random" env OBJDUMP="$objdump" OBJDUMP_FIXTURE="$fixture" "$check" 2.35 "$binary"

  fixture="$WORK/symbols-single-digit-minor.txt"
  cat > "$fixture" <<'SYMBOLS'
0000000000000000      DF *UND*	0000000000000000 (GLIBC_2.4) __stack_chk_fail
0000000000000000      DF *UND*	0000000000000000 (GLIBC_2.9) pipe2
SYMBOLS
  expect "minor versions compare as numbers, so 2.9 is under 2.35" 0 "highest glibc symbol version 2\.9 " env OBJDUMP="$objdump" OBJDUMP_FIXTURE="$fixture" "$check" 2.35 "$binary"

  fixture="$WORK/symbols-none.txt"
  printf 'DYNAMIC SYMBOL TABLE:\nno symbols\n' > "$fixture"
  expect "a binary with no glibc symbol fails instead of passing unread" 1 "references no GLIBC_ symbol version" env OBJDUMP="$objdump" OBJDUMP_FIXTURE="$fixture" "$check" 2.35 "$binary"

  expect "a missing binary fails" 1 "absent-elf does not exist" env OBJDUMP="$objdump" OBJDUMP_FIXTURE="$fixture" "$check" 2.35 "$WORK/absent-elf"
  expect "a malformed floor is a usage error" 2 "must look like 2\.35" "$check" jammy "$binary"
  expect "a call without a binary is a usage error" 2 "usage: check-glibc-floor\.sh" "$check" 2.35
}

have_tool() {
  local tool="$1" purpose="$2"
  if command -v "$tool" >/dev/null; then
    return 0
  fi
  if [ -n "${CI:-}" ]; then
    echo "FAIL $tool is required in CI to test $purpose"
    failures=$((failures + 1))
    cases=$((cases + 1))
  else
    echo "skip the $purpose cases: $tool is not installed"
  fi
  return 1
}

new_minisign_key() {
  local dir="$WORK/minisign/$1"
  mkdir -p "$dir"
  minisign -G -W -f -p "$dir/key.pub" -s "$dir/key.sec" >/dev/null
  printf '%s' "$dir"
}

minisign_pubkey() {
  sed -n 2p "$1/key.pub"
}

sign_asset() {
  local key_dir="$1" asset="$2" name="${3:-$(basename "$2")}"
  minisign -S -s "$key_dir/key.sec" -m "$asset" -t "paneflow release artifact $name" >/dev/null
}

release_signature_cases() {
  have_tool minisign "scripts/verify-release-signatures.sh" || return 0
  local verify="$ROOT/scripts/verify-release-signatures.sh"
  local current next dir
  current="$(new_minisign_key current)"
  next="$(new_minisign_key next)"
  dir="$WORK/release-assets"
  mkdir -p "$dir"

  printf 'deb payload' > "$dir/paneflow-1.2.3-x86_64.deb"
  printf 'rpm payload' > "$dir/paneflow-1.2.3-x86_64.rpm"
  sign_asset "$current" "$dir/paneflow-1.2.3-x86_64.deb"
  sign_asset "$current" "$dir/paneflow-1.2.3-x86_64.rpm"
  expect "assets signed by the embedded key pass" 0 "2 asset\(s\) carry a valid minisign signature" \
    env PANEFLOW_MINISIGN_PUBKEY="$(minisign_pubkey "$current")" "$verify" "$dir/paneflow-1.2.3-x86_64.deb" "$dir/paneflow-1.2.3-x86_64.rpm"

  printf 'tampered' >> "$dir/paneflow-1.2.3-x86_64.rpm"
  expect "an asset changed after signing fails before publishing" 1 "Signature check failed for paneflow-1\.2\.3-x86_64\.rpm: no embedded key verifies" \
    env PANEFLOW_MINISIGN_PUBKEY="$(minisign_pubkey "$current")" "$verify" "$dir/paneflow-1.2.3-x86_64.deb" "$dir/paneflow-1.2.3-x86_64.rpm"

  printf 'aarch64 payload' > "$dir/paneflow-1.2.3-aarch64.deb"
  expect "an asset without a signature fails" 1 "Signature check failed for paneflow-1\.2\.3-aarch64\.deb: paneflow-1\.2\.3-aarch64\.deb\.minisig is missing" \
    env PANEFLOW_MINISIGN_PUBKEY="$(minisign_pubkey "$current")" "$verify" "$dir/paneflow-1.2.3-aarch64.deb"

  sign_asset "$next" "$dir/paneflow-1.2.3-aarch64.deb"
  expect "a signature from a key no binary embeds fails" 1 "Signature check failed for paneflow-1\.2\.3-aarch64\.deb: no embedded key verifies" \
    env PANEFLOW_MINISIGN_PUBKEY="$(minisign_pubkey "$current")" "$verify" "$dir/paneflow-1.2.3-aarch64.deb"
  expect "a signature from the rotation key passes" 0 "verified: paneflow-1\.2\.3-aarch64\.deb" \
    env PANEFLOW_MINISIGN_PUBKEY="$(minisign_pubkey "$current")" PANEFLOW_MINISIGN_PUBKEY_NEXT="$(minisign_pubkey "$next")" "$verify" "$dir/paneflow-1.2.3-aarch64.deb"

  printf 'old release' > "$dir/paneflow-1.2.4-x86_64.deb"
  sign_asset "$current" "$dir/paneflow-1.2.4-x86_64.deb" "paneflow-1.2.2-x86_64.deb"
  expect "a valid signature made for another asset name fails" 1 "Signature check failed for paneflow-1\.2\.4-x86_64\.deb: the signature was made for 'paneflow release artifact paneflow-1\.2\.2-x86_64\.deb'" \
    env PANEFLOW_MINISIGN_PUBKEY="$(minisign_pubkey "$current")" "$verify" "$dir/paneflow-1.2.4-x86_64.deb"

  expect "an empty embedded key fails" 1 "PANEFLOW_MINISIGN_PUBKEY is empty" \
    env PANEFLOW_MINISIGN_PUBKEY=" " "$verify" "$dir/paneflow-1.2.3-x86_64.deb"
}

cask_template() {
  local path="$WORK/cask/$1"
  mkdir -p "$(dirname "$path")"
  cat > "$path"
  printf '%s' "$path"
}

render_cask_cases() {
  have_tool ruby "scripts/render-cask.sh" || return 0
  local render="$ROOT/scripts/render-cask.sh"
  local sha="0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef"
  local template published out="$WORK/cask/out.rb"
  mkdir -p "$WORK/cask"

  expect "the repository template renders and parses" 0 "rendered .*: version 1\.2\.3, sha256 $sha, Syntax OK" \
    "$render" "$ROOT/packaging/homebrew/paneflow.rb" 1.2.3 "$sha" "$out"
  cases=$((cases + 1))
  if ! grep -qx '  version "1.2.3"' "$out" || ! grep -qx "  sha256 \"$sha\"" "$out"; then
    echo "FAIL the rendered cask does not carry the version and sha256 lines"
    failures=$((failures + 1))
  fi

  template="$(printf 'cask "paneflow" do\r\n  version "0.0.0"\r\n  sha256 "%064d"\r\nend\r\n' 0 | cask_template crlf.rb)"
  expect "a CRLF template renders" 0 "version 1\.2\.3" "$render" "$template" 1.2.3 "$sha" "$out"

  template="$(cask_template indented.rb <<'EOF'
cask "paneflow" do
    version "0.0.0"
  sha256 "0000000000000000000000000000000000000000000000000000000000000000"
end
EOF
)"
  expect "a template the sed no longer matches fails before the push" 1 "Cask version is missing, expected 1\.2\.3" "$render" "$template" 1.2.3 "$sha" "$out"

  template="$(cask_template leftover.rb <<'EOF'
cask "paneflow" do
  version "0.0.0"
  sha256 "0000000000000000000000000000000000000000000000000000000000000000"
  livecheck do
    version "0.0.0"
  end
end
EOF
)"
  expect "a leftover 0.0.0 placeholder fails" 1 "still carries a template placeholder" "$render" "$template" 1.2.3 "$sha" "$out"

  template="$(cask_template broken.rb <<'EOF'
cask "paneflow" do
  version "0.0.0"
  sha256 "0000000000000000000000000000000000000000000000000000000000000000"
EOF
)"
  expect "a cask that is not valid Ruby fails" 1 "the rendered cask is not valid Ruby" "$render" "$template" 1.2.3 "$sha" "$out"

  published="$(cask_template published.rb <<'EOF'
cask "paneflow" do
  version "1.10.0"
  sha256 "1111111111111111111111111111111111111111111111111111111111111111"
end
EOF
)"
  expect "a version lower than the tap's is refused" 1 "Cask version 1\.9\.0 is lower than the 1\.10\.0 the tap already publishes" \
    "$render" "$ROOT/packaging/homebrew/paneflow.rb" 1.9.0 "$sha" "$out" "$published"
  expect "the same version as the tap renders" 0 "the tap publishes 1\.10\.0, rendering 1\.10\.0" \
    "$render" "$ROOT/packaging/homebrew/paneflow.rb" 1.10.0 "$sha" "$out" "$published"
  expect "a higher version than the tap renders" 0 "the tap publishes 1\.10\.0, rendering 1\.11\.0" \
    "$render" "$ROOT/packaging/homebrew/paneflow.rb" 1.11.0 "$sha" "$out" "$published"
  expect "a tag that is not a stable version is refused" 1 "is not MAJOR\.MINOR\.PATCH" \
    "$render" "$ROOT/packaging/homebrew/paneflow.rb" 1.2.3-rc.1 "$sha" "$out"
}

new_gpg_home() {
  local home="$WORK/gnupg-$1"
  mkdir -p "$home"
  chmod 700 "$home"
  gpg --homedir "$home" --batch --quiet --passphrase '' --quick-gen-key "Paneflow $1 <$1@example.invalid>" ed25519 sign never 2>/dev/null
  gpg --homedir "$home" --batch --quiet --armor --export > "$home/key.asc"
  printf '%s' "$home"
}

fake_streams() {
  local root="$1" version="$2" gpg_home="$3" arch packages sha size
  mkdir -p "$root/apt/dists/stable" "$root/rpm/repodata" "$root/cask"
  {
    printf 'Origin: Paneflow test\nSuite: stable\nCodename: stable\nArchitectures: amd64 arm64\nComponents: main\nSHA256:\n'
    for arch in amd64 arm64; do
      packages="$root/apt/dists/stable/main/binary-$arch"
      mkdir -p "$packages"
      printf 'Package: paneflow\nVersion: %s-1\nArchitecture: %s\nFilename: pool/main/p/paneflow/paneflow_%s-1_%s.deb\n\n' \
        "$version" "$arch" "$version" "$arch" > "$packages/Packages"
      gzip -n -c "$packages/Packages" > "$packages/Packages.gz"
      sha="$(sha256sum "$packages/Packages.gz" | awk '{print $1}')"
      size="$(wc -c < "$packages/Packages.gz" | tr -d ' ')"
      printf ' %s %s main/binary-%s/Packages.gz\n' "$sha" "$size" "$arch"
    done
  } > "$root/apt/dists/stable/Release"
  gpg --homedir "$gpg_home" --batch --quiet --yes --clearsign --output "$root/apt/dists/stable/InRelease" "$root/apt/dists/stable/Release"

  {
    printf '<?xml version="1.0" encoding="UTF-8"?>\n<metadata packages="4">\n'
    for arch in x86_64 aarch64; do
      for release in 1.0.0 "$version"; do
        printf '<package type="rpm">\n  <name>paneflow</name>\n  <arch>%s</arch>\n  <version epoch="0" ver="%s" rel="1"/>\n</package>\n' "$arch" "$release"
      done
    done
    printf '</metadata>\n'
  } | gzip -n -c > "$root/rpm/primary.xml.gz"
  sha="$(sha256sum "$root/rpm/primary.xml.gz" | awk '{print $1}')"
  mv "$root/rpm/primary.xml.gz" "$root/rpm/repodata/$sha-primary.xml.gz"
  printf '<?xml version="1.0" encoding="UTF-8"?>\n<repomd xmlns="http://linux.duke.edu/metadata/repo">\n  <revision>1</revision>\n  <data type="primary">\n    <checksum type="sha256">%s</checksum>\n    <location href="repodata/%s-primary.xml.gz"/>\n  </data>\n</repomd>\n' \
    "$sha" "$sha" > "$root/rpm/repodata/repomd.xml"
  gpg --homedir "$gpg_home" --batch --quiet --yes --armor --detach-sign --output "$root/rpm/repodata/repomd.xml.asc" "$root/rpm/repodata/repomd.xml"

  printf 'cask "paneflow" do\n  version "%s"\n  sha256 "%s"\nend\n' "$version" "$(printf '%064d' 7)" > "$root/cask/paneflow.rb"
}

published_stream_cases() {
  have_tool gpg "scripts/verify-published-streams.sh" || return 0
  have_tool gpgv "scripts/verify-published-streams.sh" || return 0
  have_tool python3 "scripts/verify-published-streams.sh" || return 0
  local verify="$ROOT/scripts/verify-published-streams.sh"
  local signer other root="$WORK/streams" log="$WORK/streams-server.log" port="" server
  signer="$(new_gpg_home signer)"
  other="$(new_gpg_home other)"
  fake_streams "$root/current" 1.2.3 "$signer"
  fake_streams "$root/tampered" 1.2.3 "$signer"
  sed -i 's/^Origin: Paneflow test$/Origin: Paneflow forged/' "$root/tampered/apt/dists/stable/InRelease"
  fake_streams "$root/swapped" 1.2.3 "$signer"
  printf 'Package: paneflow\nVersion: 9.9.9-1\n\n' | gzip -n -c > "$root/swapped/apt/dists/stable/main/binary-arm64/Packages.gz"
  fake_streams "$root/foreign" 1.2.3 "$other"

  python3 -u -m http.server 0 --bind 127.0.0.1 --directory "$root" > "$log" 2>&1 &
  server=$!
  for _ in $(seq 1 50); do
    port="$(sed -nE 's/.* port ([0-9]+).*/\1/p' "$log" | head -1)"
    [ -n "$port" ] && break
    sleep 0.1
  done
  if [ -z "$port" ]; then
    echo "FAIL the fixture HTTP server did not start: $(cat "$log")"
    failures=$((failures + 1))
    cases=$((cases + 1))
    kill "$server" 2>/dev/null || true
    return 0
  fi
  local base="http://127.0.0.1:$port"
  local common=(env PANEFLOW_RELEASE_KEY="$signer/key.asc" PANEFLOW_VERIFY_INTERVAL_SECONDS=0)

  expect "every stream serving the version, with valid signatures, passes" 0 "cask: version 1\.2\.3" \
    "${common[@]}" PANEFLOW_PKG_BASE_URL="$base/current" PANEFLOW_CASK_URL="$base/current/cask/paneflow.rb" PANEFLOW_VERIFY_ATTEMPTS=1 "$verify" 1.2.3
  expect "the apt check reports the signed InRelease and both architectures" 0 "apt: InRelease signature valid; paneflow amd64 1\.2\.3-1 arm64 1\.2\.3-1" \
    "${common[@]}" PANEFLOW_PKG_BASE_URL="$base/current" PANEFLOW_VERIFY_ATTEMPTS=1 "$verify" --stream apt v1.2.3
  cases=$((cases + 1))
  if ! grep -qE 'GET /current/apt/dists/stable/InRelease\?cb=[0-9]+-' "$log"; then
    echo "FAIL the stream check did not bypass the CDN cache with a cache-busting query"
    failures=$((failures + 1))
  fi

  expect "a stream still serving the old version fails after every attempt, naming the stream" 1 \
    "apt stream: main/binary-amd64/Packages\.gz still lists paneflow 1\.2\.3-1, expected 1\.3\.0, after 5 attempt\(s\) 0 s apart" \
    "${common[@]}" PANEFLOW_PKG_BASE_URL="$base/current" PANEFLOW_CASK_URL="$base/current/cask/paneflow.rb" "$verify" 1.3.0
  expect "a stale rpm stream is named with the newest version it lists" 1 "rpm stream: repodata lists no paneflow 1\.3\.0 for x86_64 \(newest: 1\.2\.3\)" \
    "${common[@]}" PANEFLOW_PKG_BASE_URL="$base/current" PANEFLOW_VERIFY_ATTEMPTS=2 "$verify" --stream rpm 1.3.0
  expect "a stale cask is named" 1 "cask stream: the cask still declares version 1\.2\.3, expected 1\.3\.0" \
    "${common[@]}" PANEFLOW_CASK_URL="$base/current/cask/paneflow.rb" PANEFLOW_VERIFY_ATTEMPTS=2 "$verify" --stream cask 1.3.0
  expect "a tampered InRelease fails at once on its signature" 1 "::error::apt stream: InRelease does not carry a valid signature from key\.asc" \
    "${common[@]}" PANEFLOW_PKG_BASE_URL="$base/tampered" "$verify" --stream apt 1.2.3
  cases=$((cases + 1))
  if PANEFLOW_RELEASE_KEY="$signer/key.asc" PANEFLOW_VERIFY_INTERVAL_SECONDS=0 PANEFLOW_PKG_BASE_URL="$base/tampered" "$verify" --stream apt 1.2.3 2>&1 | grep -q 'attempt 1/'; then
    echo "FAIL a bad signature was retried instead of failing at once"
    failures=$((failures + 1))
  fi
  expect "a Packages.gz that does not match the signed InRelease fails" 1 "main/binary-arm64/Packages\.gz has SHA256 [0-9a-f]+, the signed InRelease expects" \
    "${common[@]}" PANEFLOW_PKG_BASE_URL="$base/swapped" PANEFLOW_VERIFY_ATTEMPTS=1 "$verify" --stream apt 1.2.3
  expect "repositories signed by another key fail" 1 "rpm stream: repomd\.xml does not carry a valid signature" \
    "${common[@]}" PANEFLOW_PKG_BASE_URL="$base/foreign" "$verify" --stream rpm 1.2.3
  expect "a prerelease version is refused" 2 "is not a stable MAJOR\.MINOR\.PATCH version" "$verify" 1.3.0-rc.1

  kill "$server" 2>/dev/null || true
  wait "$server" 2>/dev/null || true
}

canned_gh() {
  local bin="$WORK/canned-gh-bin"
  mkdir -p "$bin"
  cat > "$bin/gh" <<'EOF'
#!/usr/bin/env bash
printf '%s\n' "$*" >> "$CANNED_GH_LOG"
if [ "${CANNED_GH_EXIT:-0}" -ne 0 ]; then
  cat "$CANNED_GH_OUTPUT" >&2
  exit "$CANNED_GH_EXIT"
fi
cat "$CANNED_GH_OUTPUT"
EOF
  chmod +x "$bin/gh"
  printf '%s' "$bin"
}

release_publication_cases() {
  local guard="$ROOT/scripts/require-unpublished-release.sh"
  local digests="$ROOT/scripts/verify-release-digests.sh"
  local bin answer="$WORK/canned-gh-answer" log="$WORK/canned-gh.log" dir="$WORK/built-assets"
  bin="$(canned_gh)"
  local run=(env PATH="$bin:$PATH" GITHUB_REPOSITORY=owner/paneflow CANNED_GH_OUTPUT="$answer" CANNED_GH_LOG="$log")

  printf 'false true https://github.com/owner/paneflow/releases/tag/v1.2.3\n' > "$answer"
  expect "a re-run after publication fails with a message instead of replacing the assets" 1 \
    "Release v1\.2\.3 is already published \(https://github\.com/owner/paneflow/releases/tag/v1\.2\.3, immutable: true\)\. A re-run never replaces published assets: cut a patch version" \
    "${run[@]}" "$guard" v1.2.3
  printf 'true false https://github.com/owner/paneflow/releases/tag/untagged-1\n' > "$answer"
  expect "a draft left by an earlier attempt may be replaced" 0 "exists as a draft" "${run[@]}" "$guard" v1.2.3
  printf 'gh: Not Found (HTTP 404)\n' > "$answer"
  expect "a tag with no release yet passes" 0 "no published release for v1\.2\.3 yet" "${run[@]}" CANNED_GH_EXIT=1 "$guard" v1.2.3
  printf 'gh: HTTP 502: Bad Gateway\n' > "$answer"
  expect "an unreadable release fails closed" 1 "Could not read the release of v1\.2\.3: gh: HTTP 502" "${run[@]}" CANNED_GH_EXIT=1 "$guard" v1.2.3

  mkdir -p "$dir"
  printf 'msi bytes' > "$dir/paneflow-1.2.3-x86_64-pc-windows-msvc.msi"
  printf 'sig bytes' > "$dir/paneflow-1.2.3-x86_64-pc-windows-msvc.msi.minisig"
  local msi_sha sig_sha
  msi_sha="$(sha256sum "$dir/paneflow-1.2.3-x86_64-pc-windows-msvc.msi" | awk '{print $1}')"
  sig_sha="$(sha256sum "$dir/paneflow-1.2.3-x86_64-pc-windows-msvc.msi.minisig" | awk '{print $1}')"

  printf 'paneflow-1.2.3-x86_64-pc-windows-msvc.msi\tsha256:%s\npaneflow-1.2.3-x86_64-pc-windows-msvc.msi.minisig\tsha256:%s\n' "$msi_sha" "$sig_sha" > "$answer"
  : > "$log"
  expect "uploaded digests equal to the built files pass" 0 "every one of the 2 uploaded assets matches" "${run[@]}" "$digests" 4242 "$dir"
  cases=$((cases + 1))
  if ! grep -q 'repos/owner/paneflow/releases/4242/assets' "$log"; then
    echo "FAIL the digest check did not read the assets of the draft release"
    failures=$((failures + 1))
  fi

  printf 'paneflow-1.2.3-x86_64-pc-windows-msvc.msi\tsha256:%064d\npaneflow-1.2.3-x86_64-pc-windows-msvc.msi.minisig\tsha256:%s\n' 0 "$sig_sha" > "$answer"
  expect "a digest gap fails and leaves the draft for inspection" 1 \
    "Digest check failed for paneflow-1\.2\.3-x86_64-pc-windows-msvc\.msi: uploaded sha256:0{64}, built sha256:$msi_sha" \
    "${run[@]}" "$digests" 4242 "$dir"
  expect "the digest failure names the draft left in place" 1 "The draft release 4242 stays in place for inspection" "${run[@]}" "$digests" 4242 "$dir"

  printf 'paneflow-1.2.3-x86_64-pc-windows-msvc.msi\tsha256:%s\n' "$msi_sha" > "$answer"
  expect "a built asset missing from the release fails" 1 "Digest check failed for paneflow-1\.2\.3-x86_64-pc-windows-msvc\.msi\.minisig: the asset is missing from release 4242" \
    "${run[@]}" "$digests" 4242 "$dir"

  printf 'paneflow-1.2.3-x86_64-pc-windows-msvc.msi\tsha256:%s\npaneflow-1.2.3-x86_64-pc-windows-msvc.msi.minisig\tsha256:%s\nstale.msi\tsha256:%064d\n' "$msi_sha" "$sig_sha" 1 > "$answer"
  expect "an extra asset on the release fails" 1 "Digest check failed for stale\.msi: release 4242 carries an asset this run did not build" \
    "${run[@]}" "$digests" 4242 "$dir"

  printf 'paneflow-1.2.3-x86_64-pc-windows-msvc.msi\t\npaneflow-1.2.3-x86_64-pc-windows-msvc.msi.minisig\tsha256:%s\n' "$sig_sha" > "$answer"
  expect "an asset GitHub reports no digest for fails" 1 "GitHub reports no digest for the uploaded asset" "${run[@]}" "$digests" 4242 "$dir"
}

powershell_retry_cases() {
  if ! command -v pwsh >/dev/null; then
    if [ -n "${CI:-}" ]; then
      echo "FAIL pwsh is required in CI to test scripts/ci-retry-network.ps1"
      failures=$((failures + 1))
      cases=$((cases + 1))
    else
      echo "skip the PowerShell retry cases: pwsh is not installed"
    fi
    return
  fi
  local probe="$WORK/retry-probe.ps1"
  cat > "$probe" <<'EOF'
param([string]$Retry, [int]$Failures, [string]$Message, [string]$Counter)
Set-Content -LiteralPath $Counter -Value "0"
$env:PROBE_COUNTER = $Counter
$env:PROBE_FAILURES = "$Failures"
$env:PROBE_MESSAGE = $Message
& $Retry -BackoffSeconds 0 -Command {
    $runs = [int](Get-Content -LiteralPath $env:PROBE_COUNTER) + 1
    Set-Content -LiteralPath $env:PROBE_COUNTER -Value "$runs"
    if ($runs -le [int]$env:PROBE_FAILURES) {
        throw "build failed: $($env:PROBE_MESSAGE)"
    }
    Write-Output "command output"
}
exit $LASTEXITCODE
EOF
  local retry="$ROOT/scripts/ci-retry-network.ps1"
  local counter="$WORK/ps-retry-count"

  expect "PowerShell: a transient Zig signature is retried" 0 "succeeded on attempt 2 after a transient network failure" \
    pwsh -NoProfile -File "$probe" -Retry "$retry" -Failures 1 -Message "error: ReadFailed" -Counter "$counter"
  expect "PowerShell: a failure with no transient signature is not retried" 1 "no transient network signature; not retrying" \
    pwsh -NoProfile -File "$probe" -Retry "$retry" -Failures 1 -Message "cannot extract the Zig source tar" -Counter "$counter"
  cases=$((cases + 1))
  if [ "$(tr -d '[:space:]' < "$counter")" != "1" ]; then
    echo "FAIL PowerShell: a failure with no transient signature ran the command $(cat "$counter") time(s), expected 1"
    failures=$((failures + 1))
  fi
  expect "PowerShell: a transient failure that never clears fails after the last attempt" 1 "still failing on a transient network error after 3 attempts" \
    pwsh -NoProfile -File "$probe" -Retry "$retry" -Failures 9 -Message "ProtocolError" -Counter "$counter"
  cases=$((cases + 1))
  if [ "$(tr -d '[:space:]' < "$counter")" != "3" ]; then
    echo "FAIL PowerShell: a transient failure that never clears ran the command $(cat "$counter") time(s), expected 3"
    failures=$((failures + 1))
  fi
}

check_workflows_cases
action_pin_cases
release_signature_cases
render_cask_cases
published_stream_cases
release_publication_cases
helper_size_cases
glibc_floor_cases
retry_cases
powershell_retry_cases

if [ "$failures" -ne 0 ]; then
  echo "::error::$failures of $cases CI script case(s) failed"
  exit 1
fi
echo "$cases CI script case(s) passed"

#!/usr/bin/env bash
set -euo pipefail

cd "$(dirname "$0")/.."

profile="release"
target=""
out=""
while [ $# -gt 0 ]; do
  case "$1" in
    --profile) shift; profile="${1:-release}" ;;
    --target) shift; target="${1:-}" ;;
    --out) shift; out="${1:-}" ;;
    *) echo "unknown argument: $1" >&2; exit 2 ;;
  esac
  shift
done

if [ -z "$target" ]; then
  target=$(rustc -vV | sed -n 's/^host: //p')
fi
case "$target" in
  *windows*) exe=".exe" ;;
  *) exe="" ;;
esac

bin_dir="target/$profile"
if [ -d "target/$target/$profile" ]; then
  bin_dir="target/$target/$profile"
fi
helper_dir="target/embed-build/$target/release-min"

sha=$(git rev-parse HEAD)
dirty=false
if [ -n "$(git status --porcelain --untracked-files=no)" ]; then
  dirty=true
fi

hash_file() {
  if command -v sha256sum >/dev/null 2>&1; then
    sha256sum "$1" | cut -d' ' -f1
  else
    shasum -a 256 "$1" | cut -d' ' -f1
  fi
}

failures=()
entries=()
add_artifact() {
  local name="$1" path="$2" required="$3" executable="$4"
  if [ ! -f "$path" ]; then
    if [ "$required" = "true" ]; then
      failures+=("missing required artifact $name at $path")
    fi
    entries+=("\"$name\": {\"path\": \"$path\", \"present\": false, \"required\": $required}")
    return
  fi
  if [ "$executable" = "true" ] && [ -z "$exe" ] && [ ! -x "$path" ]; then
    failures+=("artifact $name at $path is not executable")
  fi
  local bytes
  bytes=$(wc -c <"$path" | tr -d ' ')
  entries+=("\"$name\": {\"path\": \"$path\", \"present\": true, \"required\": $required, \"bytes\": $bytes, \"sha256\": \"$(hash_file "$path")\"}")
}

add_artifact desktop "$bin_dir/paneflow$exe" true true
add_artifact host "$bin_dir/paneflow-host$exe" true true
add_artifact fixture "$bin_dir/paneflow-session-fixture$exe" false true
for helper in paneflow-shim paneflow-ai-hook paneflow-mcp; do
  add_artifact "helper.$helper" "$helper_dir/$helper$exe" true true
done
add_artifact engine.manifest native/libghostty/manifest.toml true false
for archive in native/libghostty/prebuilt/"$target"/lib/*; do
  [ -f "$archive" ] || continue
  add_artifact "engine.$(basename "$archive")" "$archive" true false
done
if [ ! -d "native/libghostty/prebuilt/$target/lib" ]; then
  failures+=("no libghostty archive for $target under native/libghostty/prebuilt; run scripts/fetch-libghostty.sh")
fi
add_artifact conpty.manifest native/conpty/manifest.json true false
case "$target" in
  *windows*)
    add_artifact conpty.dll "native/conpty/prebuilt/$target/conpty.dll" true false
    add_artifact conpty.openconsole "native/conpty/prebuilt/$target/OpenConsole.exe" true false
    ;;
esac

if [ -z "$out" ]; then
  mkdir -p bench/results
  out="bench/results/candidate-$(git rev-parse --short=12 HEAD)-$target.json"
fi

{
  echo "{"
  echo "  \"schema_version\": 1,"
  echo "  \"candidate_sha\": \"$sha\","
  echo "  \"dirty\": $dirty,"
  echo "  \"target\": \"$target\","
  echo "  \"profile\": \"$profile\","
  echo "  \"recorded_at\": \"$(date -u +%Y-%m-%dT%H:%M:%SZ)\","
  echo "  \"preflight\": $([ ${#failures[@]} -eq 0 ] && echo '"pass"' || echo '"fail"'),"
  if [ ${#failures[@]} -eq 0 ]; then
    echo "  \"failures\": [],"
  else
    echo "  \"failures\": [$(printf '"%s",' "${failures[@]}" | sed 's/,$//')],"
  fi
  echo "  \"artifacts\": {"
  printf '    %s' "${entries[0]}"
  for entry in "${entries[@]:1}"; do
    printf ',\n    %s' "$entry"
  done
  echo
  echo "  }"
  echo "}"
} >"$out"

echo "candidate manifest: $out"
if [ ${#failures[@]} -ne 0 ]; then
  printf 'preflight failed:\n' >&2
  printf '  %s\n' "${failures[@]}" >&2
  exit 1
fi

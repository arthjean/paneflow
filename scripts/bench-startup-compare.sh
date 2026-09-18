#!/usr/bin/env bash
set -euo pipefail

cd "$(dirname "$0")/.."

usage() {
  cat <<'EOF'
usage: scripts/bench-startup-compare.sh [options]

Launches Paneflow and cmux alternately on this Mac and measures, for each
launch, the time from posix_spawn to the first on-screen window and to the
first painted frame. Writes bench/results/startup-compare-<stamp>-<sha>.json
and prints the comparison table.

options:
  --runs N          timed launches per app (default 10)
  --warmups N       untimed launches per app before timing (default 1)
  --paneflow PATH   Paneflow binary or .app (default: cargo build --release)
  --cmux PATH       cmux.app (default: /Applications/cmux.app, else download)
  --cmux-version V  cmux release tag to download, without the v (default: latest)
  --no-paint        skip the first-paint metric (needs Screen Recording)
  -h, --help        this text
EOF
}

die() {
  echo "bench-startup-compare: $*" >&2
  exit 1
}

runs=10
warmups=1
paneflow_path=""
cmux_path=""
cmux_version="${CMUX_VERSION:-}"
paint_flag=()
while [ $# -gt 0 ]; do
  case "$1" in
    --runs) runs="$2"; shift 2 ;;
    --warmups) warmups="$2"; shift 2 ;;
    --paneflow) paneflow_path="$2"; shift 2 ;;
    --cmux) cmux_path="$2"; shift 2 ;;
    --cmux-version) cmux_version="$2"; shift 2 ;;
    --no-paint) paint_flag=(--no-paint); shift ;;
    -h|--help) usage; exit 0 ;;
    *) die "unknown option $1 (see --help)" ;;
  esac
done

[ "$(uname -s)" = "Darwin" ] || die "cmux only runs on macOS; run this on a Mac"
command -v swiftc >/dev/null || die "swiftc not found; install the Xcode Command Line Tools"
if pgrep -x cmux >/dev/null; then
  die "cmux is running; quit it so the timed launches start from a cold process"
fi
if pgrep -x paneflow >/dev/null; then
  echo "PANEFLOW_BENCH_WARNING a Paneflow instance is running and competes for the GPU; quit it before publishing this run" >&2
fi

root=$(pwd)
sha=$(git rev-parse --short=12 HEAD)
dirty=false
if [ -n "$(git status --porcelain --untracked-files=no)" ]; then
  dirty=true
fi
stamp=$(date -u +%Y%m%dT%H%M%SZ)
scratch=$(mktemp -d "${TMPDIR:-/tmp}/paneflow-startup-compare.XXXXXX")
trap 'rm -rf "$scratch"' EXIT

if [ -z "$paneflow_path" ]; then
  cargo build --release --locked -p paneflow-app
  paneflow_path="$root/target/release/paneflow"
fi
[ -e "$paneflow_path" ] || die "no Paneflow at $paneflow_path"
case "$paneflow_path" in
  *.app) paneflow_exe="$paneflow_path/Contents/MacOS/paneflow" ;;
  *) paneflow_exe="$paneflow_path" ;;
esac
paneflow_version=$("$paneflow_exe" --version | awk '{print $2}')

if [ -z "$cmux_path" ]; then
  if [ -d /Applications/cmux.app ]; then
    cmux_path=/Applications/cmux.app
  else
    if [ -n "$cmux_version" ]; then
      dmg_url="https://github.com/manaflow-ai/cmux/releases/download/v${cmux_version}/cmux-macos.dmg"
    else
      dmg_url="https://github.com/manaflow-ai/cmux/releases/latest/download/cmux-macos.dmg"
    fi
    echo "downloading $dmg_url"
    curl -fsSL --retry 3 -o "$scratch/cmux-macos.dmg" "$dmg_url"
    mount_point="$scratch/dmg"
    mkdir -p "$mount_point"
    hdiutil attach -nobrowse -readonly -mountpoint "$mount_point" "$scratch/cmux-macos.dmg" >/dev/null
    cp -R "$mount_point/cmux.app" "$scratch/cmux.app"
    hdiutil detach "$mount_point" >/dev/null
    cmux_path="$scratch/cmux.app"
  fi
fi
[ -d "$cmux_path" ] || die "no cmux.app at $cmux_path"
cmux_version=$(/usr/libexec/PlistBuddy -c 'Print :CFBundleShortVersionString' "$cmux_path/Contents/Info.plist")

home="$scratch/paneflow-home"
mkdir -p "$home"
cat > "$home/session.json" <<'EOF'
{"version":2,"active_workspace":0,"workspaces":[]}
EOF
printf '{}\n' > "$home/paneflow.json"
printf '{"width":1400.0,"height":900.0}\n' > "$home/window-state.json"
printf 'startup-compare\n' > "$home/telemetry_id"

probe="$scratch/probe"
swiftc -O -suppress-warnings -o "$probe" scripts/bench-startup-compare.swift

mkdir -p bench/results
out="$root/bench/results/startup-compare-${stamp}-${sha}.json"

"$probe" \
  --runs "$runs" \
  --warmups "$warmups" \
  --out "$out" \
  --meta "paneflow_git_sha=$sha" \
  --meta "paneflow_git_dirty=$dirty" \
  --meta "stamp=$stamp" \
  --app paneflow "$paneflow_path" "$paneflow_version" \
  --env paneflow "PANEFLOW_HOME=$home" \
  --env paneflow "PANEFLOW_SOCKET_PATH=$home/ipc.sock" \
  --app cmux "$cmux_path" "$cmux_version" \
  ${paint_flag[@]+"${paint_flag[@]}"}

echo "result: $out"

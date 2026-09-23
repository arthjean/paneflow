#!/usr/bin/env bash
set -uo pipefail

package=""
root="$HOME/paneflow-qualification"
min_free_gib=20
while [ $# -gt 0 ]; do
  case "$1" in
    --package) shift; package="${1:-}" ;;
    --root) shift; root="${1:-}" ;;
    --min-free-gib) shift; min_free_gib="${1:-20}" ;;
    *) echo "unknown argument: $1" >&2; exit 2 ;;
  esac
  shift
done

if [ -z "$package" ]; then
  package=$(cd "$(dirname "$0")/.." && pwd -P)
fi
package=$(cd "$package" && pwd -P) || { echo "package directory not found" >&2; exit 2; }
if [ "$(uname -s)" != "Darwin" ]; then
  echo "this preflight runs on the rented Mac only" >&2
  exit 2
fi

stamp=$(date -u +%Y%m%dT%H%M%SZ)
mkdir -p "$root/evidence" "$root/homes"
root=$(cd "$root" && pwd -P)
evidence="$root/evidence/preflight-$stamp"
mkdir -p "$evidence"
ledger="$evidence/ledger.tsv"
printf 'check\tresult\tdetail\n' >"$ledger"
failed=0

record() {
  printf '%s\t%s\t%s\n' "$1" "$2" "$3" | tee -a "$ledger"
  if [ "$2" = "fail" ]; then
    failed=1
  fi
}

candidate="$package/candidate"
app="$candidate/PaneFlow.app"
desktop="$app/Contents/MacOS/paneflow"
host="$app/Contents/MacOS/paneflow-host"
fixture="$candidate/bin/paneflow-session-fixture"

sysctl hw.model machdep.cpu.brand_string hw.memsize hw.ncpu hw.perflevel0.physicalcpu hw.perflevel1.physicalcpu >"$evidence/hardware.txt" 2>&1
sw_vers >"$evidence/os.txt" 2>&1
uname -a >>"$evidence/os.txt" 2>&1
system_profiler SPHardwareDataType SPDisplaysDataType SPStorageDataType >"$evidence/system_profiler.txt" 2>&1

if [ "$(uname -m)" = "arm64" ]; then
  record M01-arch pass "$(sysctl -n hw.model) $(sysctl -n machdep.cpu.brand_string), $(($(sysctl -n hw.memsize) / 1073741824)) GiB, $(sysctl -n hw.ncpu) CPUs"
else
  record M01-arch fail "uname -m is $(uname -m); only Apple Silicon is qualified"
fi

record M02-os info "macOS $(sw_vers -productVersion) build $(sw_vers -buildVersion)"

console_user=$(stat -f %Su /dev/console 2>/dev/null)
if [ "$console_user" = "$(id -un)" ] && launchctl print "gui/$(id -u)" >/dev/null 2>&1; then
  record M03-gui-session pass "console user $console_user owns the graphical session"
else
  record M03-gui-session fail "console user is '${console_user:-none}', not $(id -un); log in through VNC first, then rerun from a Terminal inside that session"
fi

metal=$(sed -n 's/^ *Metal Support: //p' "$evidence/system_profiler.txt" | head -n 1)
resolution=$(sed -n 's/^ *Resolution: //p' "$evidence/system_profiler.txt" | head -n 1)
if [ -n "$metal" ]; then
  record M04-metal pass "$metal; display ${resolution:-not reported}"
else
  record M04-metal fail "system_profiler reports no Metal support"
fi

free_kib=$(df -k "$root" | awk 'NR == 2 {print $4}')
free_gib=$((free_kib / 1048576))
if [ "$free_gib" -ge "$min_free_gib" ]; then
  record M05-disk pass "$free_gib GiB free under $root (minimum $min_free_gib)"
else
  record M05-disk fail "$free_gib GiB free under $root, below the $min_free_gib GiB minimum"
fi

filevault=$(fdesetup status 2>/dev/null | head -n 1)
case "$filevault" in
  *Off*) record M06-filevault pass "$filevault" ;;
  *) record M06-filevault fail "${filevault:-fdesetup unavailable}; FileVault blocks remote access after a reboot" ;;
esac

if [ -f "$package/SHA256SUMS" ] && (cd "$package" && shasum -a 256 -c SHA256SUMS >"$evidence/sha256-check.txt" 2>&1); then
  record M07-package-integrity pass "$(wc -l <"$package/SHA256SUMS" | tr -d ' ') files match SHA256SUMS"
else
  record M07-package-integrity fail "SHA256SUMS missing or mismatched; see sha256-check.txt"
fi

if xattr -r "$package" 2>/dev/null | grep -q com.apple.quarantine; then
  record M08-quarantine fail "quarantine attribute present; run: xattr -dr com.apple.quarantine $package"
else
  record M08-quarantine pass "no quarantine attribute on the package"
fi

codesign -dv --verbose=2 "$app" >"$evidence/codesign.txt" 2>&1
spctl --assess --type execute -vv "$app" >"$evidence/gatekeeper.txt" 2>&1
signature=$(sed -n 's/^Signature=//p' "$evidence/codesign.txt")
authority=$(sed -n 's/^Authority=//p' "$evidence/codesign.txt" | head -n 1)
if codesign --verify --deep --strict "$app" >>"$evidence/codesign.txt" 2>&1; then
  verified="bundle signature verifies"
else
  verified="bundle signature does not verify (linker ad-hoc signatures only)"
fi
record M09-signature info "${signature:-no signature}${authority:+, $authority}; $verified; Gatekeeper: $(tail -n 1 "$evidence/gatekeeper.txt"); distribution signing is not certified by this cell"

others=$(pgrep -x paneflow; pgrep -x paneflow-host; pgrep -x paneflow-session-fixture)
if [ -z "$others" ]; then
  record M10-no-competing-paneflow pass "no paneflow, paneflow-host, or fixture process is running"
else
  record M10-no-competing-paneflow fail "processes already running: $(echo "$others" | tr '\n' ' ')"
fi

tmp_len=${#TMPDIR}
if [ "$tmp_len" -le 60 ]; then
  record M11-socket-path pass "TMPDIR is $tmp_len bytes, leaving room under the 104-byte sun_path limit"
else
  record M11-socket-path fail "TMPDIR is $tmp_len bytes; the host endpoint may exceed the 104-byte sun_path limit"
fi

host_home="$root/homes/preflight-host-$stamp"
mkdir -p "$host_home"
if [ -x "$host" ] && [ -x "$fixture" ]; then
  PANEFLOW_HOME="$host_home" "$host" serve >"$evidence/host.log" 2>&1 </dev/null &
  host_pid=$!
  serving=false
  for _ in $(seq 1 100); do
    if PANEFLOW_HOME="$host_home" "$host" session list >/dev/null 2>&1; then
      serving=true
      break
    fi
    sleep 0.1
  done
  session=""
  if [ "$serving" = "true" ]; then
    session=$(PANEFLOW_HOME="$host_home" "$host" session create --cwd "$host_home" --shell "$fixture" -- idle 2>>"$evidence/host.log" | sed -n 's/.*"session": "\([^"]*\)".*/\1/p' | head -n 1)
  fi
  live=false
  if [ -n "$session" ]; then
    sleep 1
    PANEFLOW_HOME="$host_home" "$host" session inspect "$session" >"$evidence/session-inspect.json" 2>&1 && grep -q '"live": true' "$evidence/session-inspect.json" && live=true
    PANEFLOW_HOME="$host_home" "$host" session stop "$session" >>"$evidence/host.log" 2>&1
  fi
  kill -TERM "$host_pid" 2>/dev/null
  wait "$host_pid" 2>/dev/null
  if [ "$live" = "true" ]; then
    record M12-host pass "packaged host served $host_home and ran one idle fixture session"
  else
    record M12-host fail "packaged host did not serve or run a fixture; see host.log"
  fi
else
  record M12-host fail "packaged host or fixture missing under $candidate"
fi

desktop_home="$root/homes/preflight-desktop-$stamp"
mkdir -p "$desktop_home"
if [ -x "$desktop" ]; then
  PANEFLOW_HOME="$desktop_home" RUST_LOG=info "$desktop" >"$evidence/desktop.log" 2>&1 </dev/null &
  desktop_pid=$!
  rendered=false
  for _ in $(seq 1 60); do
    if grep -q "font: family=" "$evidence/desktop.log" 2>/dev/null; then
      rendered=true
      break
    fi
    kill -0 "$desktop_pid" 2>/dev/null || break
    sleep 0.5
  done
  sleep 3
  screencapture -x "$evidence/desktop.png" 2>>"$evidence/desktop.log"
  alive=false
  kill -0 "$desktop_pid" 2>/dev/null && alive=true
  kill -TERM "$desktop_pid" 2>/dev/null
  wait "$desktop_pid" 2>/dev/null
  PANEFLOW_HOME="$desktop_home" "$desktop" host stop >>"$evidence/desktop.log" 2>&1
  if [ "$rendered" = "true" ] && [ "$alive" = "true" ]; then
    record M13-desktop pass "packaged desktop reached its first font configuration under Metal and stayed up; screenshot desktop.png"
  else
    record M13-desktop fail "desktop rendered=$rendered alive=$alive; see desktop.log and desktop.png"
  fi
else
  record M13-desktop fail "packaged desktop missing at $desktop"
fi

archive="$root/evidence/preflight-$stamp.tar.gz"
tar -C "$root/evidence" -czf "$archive" "preflight-$stamp"
shasum -a 256 "$archive" >"$archive.sha256"
if tar -tzf "$archive" >/dev/null 2>&1; then
  record M14-evidence-export pass "$archive"
else
  record M14-evidence-export fail "the evidence archive does not list"
fi

echo
echo "ledger: $ledger"
echo "fetch from the operator machine: scp '$(id -un)@<mac ip>:$archive' '$(id -un)@<mac ip>:$archive.sha256' ."
exit "$failed"

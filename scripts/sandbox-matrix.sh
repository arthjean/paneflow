#!/usr/bin/env bash
set -euo pipefail

usage() {
  cat >&2 <<'EOF'
usage: sandbox-matrix.sh [--suite sandbox|identity] [--out <dir>] [--skip-build] [<distribution>...]

Runs Linux tests of paneflow-host on the real kernel of each distribution, in
a throwaway KVM virtual machine booted from its cloud image, and prints one
Markdown row per distribution.

Suites:
  sandbox   the sandbox tests (sandboxed_pane); default distributions:
            ubuntu-22.04 ubuntu-24.04 debian-13 arch opensuse-leap-16
  identity  the caller identity tests (caller_identity, linux_kernel_matrix),
            run a second time with /proc remounted hidepid=2; default
            distributions: ubuntu-22.04 ubuntu-24.04

Distributions: ubuntu-22.04 ubuntu-24.04 debian-13 arch opensuse-leap-16
(the suite's defaults when none is named).

Needs: /dev/kvm, qemu-system-x86_64, qemu-img, genisoimage, podman, curl, ssh.
Images are kept in ${PANEFLOW_MATRIX_CACHE:-~/.cache/paneflow-sandbox-matrix}.
EOF
  exit 2
}

REPO="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
CACHE="${PANEFLOW_MATRIX_CACHE:-${XDG_CACHE_HOME:-$HOME/.cache}/paneflow-sandbox-matrix}"
BUILD_IMAGE="${PANEFLOW_MATRIX_BUILD_IMAGE:-docker.io/library/ubuntu:22.04}"
GUEST_ROOT="/opt/paneflow-matrix"
GUEST_USER="matrix"
OUT=""
SKIP_BUILD=0
SUITE=sandbox
DISTROS=()

while [ "$#" -gt 0 ]; do
  case "$1" in
    --out)
      [ "$#" -ge 2 ] || usage
      OUT="$2"
      shift 2
      ;;
    --skip-build)
      SKIP_BUILD=1
      shift
      ;;
    --suite)
      [ "$#" -ge 2 ] || usage
      case "$2" in
        sandbox | identity) SUITE="$2" ;;
        *) usage ;;
      esac
      shift 2
      ;;
    -h | --help) usage ;;
    -*) usage ;;
    *)
      DISTROS+=("$1")
      shift
      ;;
  esac
done
if [ "${#DISTROS[@]}" -eq 0 ]; then
  if [ "$SUITE" = identity ]; then
    DISTROS=(ubuntu-22.04 ubuntu-24.04)
  else
    DISTROS=(ubuntu-22.04 ubuntu-24.04 debian-13 arch opensuse-leap-16)
  fi
fi
TESTS=(sandboxed_pane caller_identity linux_kernel_matrix)

image_url() {
  case "$1" in
    ubuntu-22.04) echo "https://cloud-images.ubuntu.com/jammy/current/jammy-server-cloudimg-amd64.img" ;;
    ubuntu-24.04) echo "https://cloud-images.ubuntu.com/noble/current/noble-server-cloudimg-amd64.img" ;;
    debian-13) echo "https://cloud.debian.org/images/cloud/trixie/latest/debian-13-genericcloud-amd64.qcow2" ;;
    arch) echo "https://geo.mirror.pkgbuild.com/images/latest/Arch-Linux-x86_64-cloudimg.qcow2" ;;
    opensuse-leap-16) echo "https://download.opensuse.org/distribution/leap/16.0/appliances/Leap-16.0-Minimal-VM.x86_64-Cloud.qcow2" ;;
    *)
      echo "unknown distribution '$1'" >&2
      return 1
      ;;
  esac
}

for tool in qemu-system-x86_64 qemu-img genisoimage curl ssh scp ssh-keygen; do
  command -v "$tool" >/dev/null || {
    echo "sandbox-matrix: $tool is missing" >&2
    exit 1
  }
done
[ -w /dev/kvm ] || {
  echo "sandbox-matrix: /dev/kvm is not writable, so the virtual machines cannot use KVM" >&2
  exit 1
}
for distro in "${DISTROS[@]}"; do
  image_url "$distro" >/dev/null
done

mkdir -p "$CACHE/images" "$CACHE/target"
if [ -z "$OUT" ]; then
  OUT="$(mktemp -d "${TMPDIR:-/tmp}/paneflow-sandbox-matrix.XXXXXX")"
fi
mkdir -p "$OUT"
RUN="$(mktemp -d "${TMPDIR:-/tmp}/paneflow-matrix-run.XXXXXX")"
QEMU_PID=""

cleanup() {
  if [ -n "$QEMU_PID" ] && kill -0 "$QEMU_PID" 2>/dev/null; then
    kill "$QEMU_PID" 2>/dev/null || true
    wait "$QEMU_PID" 2>/dev/null || true
  fi
  rm -rf "$RUN"
}
trap cleanup EXIT

build() {
  command -v podman >/dev/null || {
    echo "sandbox-matrix: podman is missing; it builds the tests against the oldest glibc" >&2
    exit 1
  }
  local cargo_home="${CARGO_HOME:-$HOME/.cargo}"
  local rustup_home="${RUSTUP_HOME:-$HOME/.rustup}"
  podman run --rm --security-opt label=disable \
    -v "$REPO:$REPO:ro" \
    -v "$cargo_home:/cargo" \
    -v "$rustup_home:/rustup:ro" \
    -v "$CACHE/target:$GUEST_ROOT/target" \
    -e CARGO_HOME=/cargo -e RUSTUP_HOME=/rustup \
    -e CARGO_TARGET_DIR="$GUEST_ROOT/target" \
    -e CARGO_BUILD_BUILD_DIR="$GUEST_ROOT/target" \
    -e PATH=/cargo/bin:/usr/local/sbin:/usr/local/bin:/usr/sbin:/usr/bin:/sbin:/bin \
    -w "$REPO" "$BUILD_IMAGE" bash -euc '
      export DEBIAN_FRONTEND=noninteractive
      apt-get update -qq >/dev/null
      apt-get install -y -qq gcc libc6-dev >/dev/null
      cargo test --locked --offline -p paneflow-host --test sandboxed_pane --test caller_identity --test linux_kernel_matrix --no-run
    ' >&2
}

if [ "$SKIP_BUILD" -eq 0 ]; then
  echo "sandbox-matrix: building the sandbox tests in $BUILD_IMAGE" >&2
  build
fi
HOST_BINARY="$CACHE/target/debug/paneflow-host"
FIXTURE_BINARY="$CACHE/target/debug/paneflow-session-fixture"
test_binary() {
  find "$CACHE/target/debug/deps" -maxdepth 1 -type f -name "$1-*" -perm -u+x -printf '%T@ %p\n' 2>/dev/null | sort -rn | head -n 1 | cut -d' ' -f2-
}
declare -A TEST_BINARIES=()
for name in "${TESTS[@]}"; do
  TEST_BINARIES[$name]="$(test_binary "$name")"
  if [ -z "${TEST_BINARIES[$name]}" ]; then
    echo "sandbox-matrix: the $name test binary is missing under $CACHE/target; run without --skip-build" >&2
    exit 1
  fi
done
if [ ! -x "$HOST_BINARY" ] || [ ! -x "$FIXTURE_BINARY" ]; then
  echo "sandbox-matrix: the built binaries are missing under $CACHE/target; run without --skip-build" >&2
  exit 1
fi

ssh-keygen -q -t ed25519 -N "" -f "$RUN/key"
SSH_OPTS=(-i "$RUN/key" -o StrictHostKeyChecking=no -o UserKnownHostsFile=/dev/null -o LogLevel=ERROR -o ConnectTimeout=5 -o BatchMode=yes)

free_port() {
  python3 -c 'import socket; s = socket.socket(); s.bind(("127.0.0.1", 0)); print(s.getsockname()[1])'
}

guest() {
  ssh "${SSH_OPTS[@]}" -p "$PORT" "$GUEST_USER@127.0.0.1" "$@"
}

cat >"$RUN/probe.sh" <<'EOF'
#!/bin/sh
field() {
  printf '%s=%s\n' "$1" "$(printf '%s' "$2" | tr '\n' ' ' | sed 's/  */ /g; s/ $//')"
}
. /etc/os-release
field distribution "$PRETTY_NAME"
field kernel "$(uname -r)"
field bwrap "$(bwrap --version 2>&1)"
field landlock_abi "$(python3 -c 'import ctypes; print(max(ctypes.CDLL(None, use_errno=True).syscall(444, None, 0, 1), 0))' 2>&1)"
field lsm "$(cat /sys/kernel/security/lsm 2>&1)"
field userns_sysctl "$(sysctl kernel.apparmor_restrict_unprivileged_userns kernel.unprivileged_userns_clone user.max_user_namespaces 2>/dev/null)"
field bwrap_apparmor_profile "$(ls /etc/apparmor.d 2>/dev/null | grep -i bwrap)"
field xdg_runtime_dir "${XDG_RUNTIME_DIR:-unset}"
raw="$(bwrap --unshare-all --die-with-parent --bind / / --proc /proc --dev /dev sh -c 'echo started as pid $$; grep NoNewPrivs /proc/self/status' 2>&1)"
field bwrap_unshare_all_exit "$?"
field bwrap_unshare_all "$raw"
tests="$(PANEFLOW_SANDBOX_REQUIRED=1 "$1" --test-threads=1 2>&1)"
field paneflow_sandbox_exit "$?"
field paneflow_sandbox "$(printf '%s' "$tests" | grep -E '^test |test result|PANEFLOW_SANDBOX_REQUIRED|panicked' | tail -n 12)"
if [ "$(sysctl -n kernel.apparmor_restrict_unprivileged_userns 2>/dev/null)" = "1" ]; then
  sudo sysctl -q -w kernel.apparmor_restrict_unprivileged_userns=0
  raw="$(bwrap --unshare-all --die-with-parent --bind / / --proc /proc --dev /dev sh -c 'echo started as pid $$' 2>&1)"
  field restriction_off_bwrap_exit "$?"
  field restriction_off_bwrap "$raw"
  tests="$(PANEFLOW_SANDBOX_REQUIRED=1 "$1" --test-threads=1 2>&1)"
  field restriction_off_paneflow_sandbox_exit "$?"
  field restriction_off_paneflow_sandbox "$(printf '%s' "$tests" | grep -E 'test result|panicked' | tail -n 4)"
  sudo sysctl -q -w kernel.apparmor_restrict_unprivileged_userns=1
fi
EOF

cat >"$RUN/probe-identity.sh" <<'EOF'
#!/bin/sh
field() {
  printf '%s=%s\n' "$1" "$(printf '%s' "$2" | tr '\n' ' ' | sed 's/  */ /g; s/ $//')"
}
matrix="$1"
identity="$2"
. /etc/os-release
field distribution "$PRETTY_NAME"
field kernel "$(uname -r)"
expected="$(uname -r | awk -F. '{ if ($1 > 6 || ($1 == 6 && $2 + 0 >= 5)) print "SO_PEERPIDFD"; else print "SO_PEERCRED" }')"
field expected_peer_source "$expected"
suite() {
  out="$(PANEFLOW_EXPECT_PEER_SOURCE="$expected" "$matrix" --test-threads=1 --nocapture 2>&1)"
  field "${1}matrix_exit" "$?"
  field "${1}matrix" "$(printf '%s\n' "$out" | awk '/^kernel-matrix:|^test |test result/ { print } /panicked/ { print; getline; print }')"
  out="$("$identity" --test-threads=1 2>&1)"
  field "${1}identity_exit" "$?"
  field "${1}identity" "$(printf '%s\n' "$out" | awk '/^test |test result/ { print } /panicked/ { print; getline; print }')"
}
field proc_mount "$(grep ' /proc proc ' /proc/mounts)"
suite ""
sudo mount -o remount,hidepid=2 /proc
field hidepid_proc_mount "$(grep ' /proc proc ' /proc/mounts)"
field hidepid_visible_root_processes "$(ps -e -o user= | grep -c '^root')"
suite hidepid_
sudo mount -o remount,hidepid=0 /proc
EOF

value() {
  sed -n "s/^$2=//p" "$1" | head -n 1
}

run_distro() {
  local distro="$1" url image overlay log report
  url="$(image_url "$distro")"
  image="$CACHE/images/$distro.qcow2"
  overlay="$RUN/$distro.qcow2"
  log="$OUT/$distro.console.log"
  report="$OUT/$distro.report"
  if [ ! -s "$image" ]; then
    echo "sandbox-matrix: downloading $url" >&2
    curl -fL --retry 3 --proto '=https' -o "$image.part" "$url" >&2
    mv "$image.part" "$image"
  fi
  qemu-img create -q -f qcow2 -F qcow2 -b "$image" "$overlay" 12G
  mkdir -p "$RUN/seed-$distro"
  cat >"$RUN/seed-$distro/user-data" <<EOF
#cloud-config
users:
  - name: $GUEST_USER
    sudo: ALL=(ALL) NOPASSWD:ALL
    shell: /bin/sh
    ssh_authorized_keys:
      - $(cat "$RUN/key.pub")
package_update: true
packages:
  - bubblewrap
runcmd:
  - mkdir -p $GUEST_ROOT/target/debug/deps
  - chown -R $GUEST_USER $GUEST_ROOT
  - touch $GUEST_ROOT/ready
EOF
  printf 'instance-id: paneflow-matrix-%s\nlocal-hostname: matrix\n' "$distro" >"$RUN/seed-$distro/meta-data"
  genisoimage -quiet -output "$RUN/seed-$distro.iso" -volid cidata -joliet -rock \
    "$RUN/seed-$distro/user-data" "$RUN/seed-$distro/meta-data"
  PORT="$(free_port)"
  qemu-system-x86_64 -enable-kvm -cpu host -m 3072 -smp 4 \
    -drive "file=$overlay,if=virtio" \
    -drive "file=$RUN/seed-$distro.iso,if=virtio,format=raw,readonly=on" \
    -nic "user,model=virtio-net-pci,hostfwd=tcp:127.0.0.1:$PORT-:22" \
    -display none -serial "file:$log" &
  QEMU_PID="$!"
  local deadline=$((SECONDS + 900))
  until guest test -e "$GUEST_ROOT/ready" 2>/dev/null; do
    if ! kill -0 "$QEMU_PID" 2>/dev/null || [ "$SECONDS" -ge "$deadline" ]; then
      echo "sandbox-matrix: $distro never became ready; see $log" >&2
      printf 'distribution=%s\nboot=failed\n' "$distro" >"$report"
      return 1
    fi
    sleep 5
  done
  local deps="$GUEST_ROOT/target/debug/deps" name
  scp -q "${SSH_OPTS[@]}" -P "$PORT" "$HOST_BINARY" "$GUEST_USER@127.0.0.1:$GUEST_ROOT/target/debug/paneflow-host"
  scp -q "${SSH_OPTS[@]}" -P "$PORT" "$FIXTURE_BINARY" "$GUEST_USER@127.0.0.1:$GUEST_ROOT/target/debug/paneflow-session-fixture"
  for name in "${TESTS[@]}"; do
    scp -q "${SSH_OPTS[@]}" -P "$PORT" "${TEST_BINARIES[$name]}" "$GUEST_USER@127.0.0.1:$deps/$name"
  done
  scp -q "${SSH_OPTS[@]}" -P "$PORT" "$RUN/probe.sh" "$RUN/probe-identity.sh" "$GUEST_USER@127.0.0.1:$GUEST_ROOT/"
  if [ "$SUITE" = identity ]; then
    guest sh "$GUEST_ROOT/probe-identity.sh" "$deps/linux_kernel_matrix" "$deps/caller_identity" >"$report" || true
  else
    guest sh "$GUEST_ROOT/probe.sh" "$deps/sandboxed_pane" >"$report" || true
  fi
  guest sudo poweroff >/dev/null 2>&1 || true
  local stop=$((SECONDS + 60))
  while kill -0 "$QEMU_PID" 2>/dev/null && [ "$SECONDS" -lt "$stop" ]; do
    sleep 1
  done
  kill "$QEMU_PID" 2>/dev/null || true
  wait "$QEMU_PID" 2>/dev/null || true
  QEMU_PID=""
  rm -f "$overlay"
}

verdict() {
  if [ "$(value "$1" "$2")" = "0" ]; then
    echo "Starts"
  else
    echo "Fails"
  fi
}

passes() {
  local key
  for key in "${@:2}"; do
    if [ "$(value "$1" "$key")" != "0" ]; then
      echo "Fails"
      return
    fi
  done
  echo "Pass"
}

status=0
if [ "$SUITE" = identity ]; then
  echo "| Distribution | Kernel | Expected peer source | Identity tests | Identity tests, \`/proc\` hidepid=2 |"
  echo "|---|---|---|---|---|"
else
  echo "| Distribution | Kernel, bubblewrap, Landlock ABI | \`bwrap --unshare-all --die-with-parent\` | Paneflow sandbox tests |"
  echo "|---|---|---|---|"
fi
for distro in "${DISTROS[@]}"; do
  echo "sandbox-matrix: running $distro" >&2
  if ! run_distro "$distro"; then
    echo "| $distro | not booted | not run | not run |"
    status=1
    continue
  fi
  report="$OUT/$distro.report"
  if [ "$SUITE" = identity ]; then
    [ "$(passes "$report" matrix_exit identity_exit hidepid_matrix_exit hidepid_identity_exit)" = "Pass" ] || status=1
    echo "| $(value "$report" distribution) | $(value "$report" kernel) | $(value "$report" expected_peer_source) | $(passes "$report" matrix_exit identity_exit) | $(passes "$report" hidepid_matrix_exit hidepid_identity_exit) |"
  else
    [ "$(value "$report" paneflow_sandbox_exit)" = "0" ] || status=1
    echo "| $(value "$report" distribution) | $(value "$report" kernel), $(value "$report" bwrap), ABI $(value "$report" landlock_abi) | $(verdict "$report" bwrap_unshare_all_exit) | $(verdict "$report" paneflow_sandbox_exit) |"
  fi
done
echo "sandbox-matrix: full reports and console logs are in $OUT" >&2
exit "$status"

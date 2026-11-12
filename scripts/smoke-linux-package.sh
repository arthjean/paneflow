#!/usr/bin/env bash
set -euo pipefail

if [ "$#" -ne 3 ]; then
  echo "usage: smoke-linux-package.sh <ubuntu|debian|fedora|opensuse|arch> <version> <asset-directory>" >&2
  echo "  installs the x86_64 package of <asset-directory> the way the distribution does, as root in a throwaway container," >&2
  echo "  and checks what the package wrote; ci-retry-network.sh must sit next to this script" >&2
  exit 2
fi

DISTRO="$1"
VERSION="${2#v}"
ASSETS="$(cd "$3" && pwd -P)"
SCRIPTS="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd -P)"
RETRY=(bash "$SCRIPTS/ci-retry-network.sh")
export DEBIAN_FRONTEND=noninteractive

stage() {
  echo "== $*"
}

single_asset() {
  local pattern="$1" matches
  shopt -s nullglob
  # shellcheck disable=SC2206
  matches=("$ASSETS"/$pattern)
  shopt -u nullglob
  if [ "${#matches[@]}" -ne 1 ]; then
    echo "::error::expected exactly one $pattern in $ASSETS, found ${#matches[@]}" >&2
    exit 1
  fi
  printf '%s' "${matches[0]}"
}

assert_version() {
  local binary="$1" name="$2" actual expected
  actual="$("$binary" --version)"
  expected="$name $VERSION"
  if [ "$actual" != "$expected" ]; then
    echo "::error::$name --version returned '$actual', expected '$expected'"
    exit 1
  fi
  echo "$actual"
}

three_attempts() {
  local label="$1" attempt
  shift
  for attempt in 1 2 3; do
    if "$@"; then
      return 0
    fi
    echo "::warning::$label failed, retry $attempt/3"
    if [ "$attempt" -eq 3 ]; then
      echo "::error::$label failed 3 times"
      exit 1
    fi
    sleep 5
  done
}

smoke_ubuntu() {
  local deb policy status custom
  stage "Install baseline tools"
  "${RETRY[@]}" apt-get update -qq
  "${RETRY[@]}" apt-get install -y --no-install-recommends ca-certificates

  stage "Install paneflow.deb (exercises postinst)"
  deb="$(single_asset 'paneflow-*.deb')"
  "${RETRY[@]}" apt-get install -y --no-install-recommends "$deb"

  stage "Assert postinst wrote paneflow.list with expected content"
  test -f /etc/apt/sources.list.d/paneflow.list
  test -s /usr/share/keyrings/paneflow-archive.asc
  grep -Fxq 'deb [signed-by=/usr/share/keyrings/paneflow-archive.asc] https://pkg.paneflow.dev/apt stable main' \
    /etc/apt/sources.list.d/paneflow.list

  stage "Assert apt update completes without error"
  "${RETRY[@]}" apt-get update

  stage "Assert paneflow visible via apt-cache policy"
  policy=$(apt-cache policy paneflow)
  grep -Eq '^  Installed: [0-9]' <<< "$policy"
  status=$(dpkg-query -W -f='${Status}' paneflow)
  grep -q 'install ok installed' <<< "$status"

  stage "Assert paneflow starts on the glibc floor and reports the released version"
  assert_version paneflow paneflow

  stage "Idempotency - pre-existing paneflow.list is not overwritten"
  custom='deb [trusted=yes] https://mirror.example.com/paneflow stable main'
  echo "$custom" > /etc/apt/sources.list.d/paneflow.list
  "${RETRY[@]}" apt-get install --reinstall -y --no-install-recommends "$deb"
  grep -Fxq "$custom" /etc/apt/sources.list.d/paneflow.list \
    || { echo "::error::postinst overwrote a pre-existing paneflow.list"; exit 1; }
}

smoke_debian() {
  local deb
  stage "Install baseline tools"
  "${RETRY[@]}" apt-get update -qq
  "${RETRY[@]}" apt-get install -y --no-install-recommends ca-certificates

  stage "Install paneflow.deb (exercises postinst on Debian 12)"
  deb="$(single_asset 'paneflow-*.deb')"
  "${RETRY[@]}" apt-get install -y --no-install-recommends "$deb"

  stage "Assert paneflow --version matches the released tag"
  assert_version paneflow paneflow

  stage "Assert postinst wrote paneflow.list (Debian path)"
  test -f /etc/apt/sources.list.d/paneflow.list
}

smoke_fedora() {
  local rpm
  stage "Install paneflow.rpm (exercises %post scriptlet)"
  rpm="$(single_asset 'paneflow-*.rpm')"
  "${RETRY[@]}" dnf install -y --allowerasing "$rpm"

  stage "Assert %post wrote /etc/yum.repos.d/paneflow.repo"
  test -f /etc/yum.repos.d/paneflow.repo
  grep -Fq '[paneflow]' /etc/yum.repos.d/paneflow.repo
  grep -Fq 'baseurl=https://pkg.paneflow.dev/rpm' /etc/yum.repos.d/paneflow.repo
  grep -Fq 'gpgcheck=1' /etc/yum.repos.d/paneflow.repo
  grep -Fq 'gpgkey=https://pkg.paneflow.dev/gpg' /etc/yum.repos.d/paneflow.repo

  stage "Assert dnf check-update paneflow does not hard-fail"
  dnf check-update paneflow || true
  dnf list installed paneflow

  stage "Idempotency - pre-existing paneflow.repo is not overwritten"
  cat > /etc/yum.repos.d/paneflow.repo <<'CUSTOMEOF'
[paneflow]
name=PaneFlow (custom mirror)
baseurl=https://mirror.example.com/paneflow
enabled=1
gpgcheck=0
CUSTOMEOF
  "${RETRY[@]}" dnf reinstall -y --allowerasing "$rpm"
  grep -Fq 'PaneFlow (custom mirror)' /etc/yum.repos.d/paneflow.repo \
    || { echo "::error::%post overwrote a pre-existing paneflow.repo"; exit 1; }
}

smoke_opensuse() {
  local rpm
  stage "Refresh zypper repos"
  sed -i 's#^baseurl=https\?://download\.opensuse\.org/#baseurl=https://downloadcontentcdn.opensuse.org/#' /etc/zypp/repos.d/*.repo
  grep -q 'downloadcontentcdn' /etc/zypp/repos.d/*.repo
  mkdir -p /etc/zypp
  touch /etc/zypp/zypp.conf
  sed -i \
    -e 's/^#\?[[:space:]]*download.connect_timeout.*/download.connect_timeout = 120/' \
    -e 's/^#\?[[:space:]]*download.transfer_timeout.*/download.transfer_timeout = 600/' \
    /etc/zypp/zypp.conf
  grep -q '^download.connect_timeout = 120' /etc/zypp/zypp.conf || \
    echo 'download.connect_timeout = 120' >> /etc/zypp/zypp.conf
  grep -q '^download.transfer_timeout = 600' /etc/zypp/zypp.conf || \
    echo 'download.transfer_timeout = 600' >> /etc/zypp/zypp.conf
  three_attempts "zypper refresh" zypper --non-interactive --gpg-auto-import-keys refresh

  stage "Install paneflow.rpm via zypper"
  rpm="$(single_asset 'paneflow-*.rpm')"
  three_attempts "zypper install" zypper --non-interactive --no-gpg-checks install --allow-unsigned-rpm "$rpm"

  stage "Assert paneflow --version matches the released tag"
  assert_version paneflow paneflow

  stage "Assert %post wrote paneflow.repo"
  test -f /etc/zypp/repos.d/paneflow.repo
  grep -Fq '[paneflow]' /etc/zypp/repos.d/paneflow.repo
}

smoke_arch() {
  local tarball paneflow_bin host_bin
  stage "Install runtime libs (tar.gz has no dep metadata)"
  three_attempts "pacman install" pacman -Sy --noconfirm --needed \
    glibc libxcb libxkbcommon libxkbcommon-x11 libx11 \
    vulkan-icd-loader fontconfig freetype2

  stage "Extract paneflow.app/ and assert --version matches the tag"
  tarball="$(single_asset 'paneflow-*.tar.gz')"
  mkdir -p /opt/paneflow-smoke
  tar -xzf "$tarball" -C /opt/paneflow-smoke
  paneflow_bin="$(find /opt/paneflow-smoke -type f -name paneflow -executable | head -n1)"
  if [ -z "$paneflow_bin" ]; then
    echo "::error::paneflow binary not found in tar.gz layout"
    ls -laR /opt/paneflow-smoke
    exit 1
  fi
  assert_version "$paneflow_bin" paneflow
  host_bin="$(dirname "$paneflow_bin")/paneflow-host"
  if [ ! -x "$host_bin" ]; then
    echo "::error::paneflow-host is missing next to $paneflow_bin; hosted terminal sessions cannot start"
    ls -laR /opt/paneflow-smoke
    exit 1
  fi
  assert_version "$host_bin" paneflow-host
}

case "$DISTRO" in
  ubuntu) smoke_ubuntu ;;
  debian) smoke_debian ;;
  fedora) smoke_fedora ;;
  opensuse) smoke_opensuse ;;
  arch) smoke_arch ;;
  *)
    echo "::error::unknown distribution '$DISTRO' (expected ubuntu, debian, fedora, opensuse or arch)" >&2
    exit 2
    ;;
esac
echo "$DISTRO package smoke passed for paneflow $VERSION"

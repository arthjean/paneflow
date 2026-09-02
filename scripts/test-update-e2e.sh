#!/usr/bin/env bash

set -euo pipefail

OLD_VERSION="${OLD_VERSION:-0.2.10}"
OLD_TAG="${OLD_TAG:-v${OLD_VERSION}}"
WORK_DIR="${WORK_DIR:-/tmp/paneflow-e2e}"
HTTP_PORT="${HTTP_PORT:-0}"
SCENARIO="${SCENARIO:-all}"

E2E_NEW_TARBALL="${E2E_NEW_TARBALL:-}"
E2E_OLD_TARBALL="${E2E_OLD_TARBALL:-}"
E2E_NEW_MINISIG="${E2E_NEW_MINISIG:-}"

REPO_ROOT="$(cd "$(dirname "$0")/.." && pwd -P)"
NEW_VERSION="$(awk -F'"' '/^version = / { print $2; exit }' "$REPO_ROOT/Cargo.toml")"

log()  { printf '[e2e] %s\n' "$*" >&2; }
fail() { printf '[e2e] FAIL: %s\n' "$*" >&2; exit 1; }
ok()   { printf '[e2e] PASS: %s\n' "$*" >&2; }

HTTP_PID=""
HTTP_LOG=""
WORKTREE_PATH=""
cleanup() {
    local rc=$?
    if [ -n "${HTTP_PID}" ] && kill -0 "${HTTP_PID}" 2>/dev/null; then
        kill "${HTTP_PID}" 2>/dev/null || true
        wait "${HTTP_PID}" 2>/dev/null || true
    fi
    if [ -n "${WORKTREE_PATH}" ] && [ -d "${WORKTREE_PATH}" ]; then
        git -C "${REPO_ROOT}" worktree remove --force "${WORKTREE_PATH}" 2>/dev/null || true
    fi
    if [ "${rc}" -ne 0 ] && [ -n "${HTTP_LOG}" ] && [ -f "${HTTP_LOG}" ]; then
        log "http.server log:"
        sed 's/^/[http] /' "${HTTP_LOG}" >&2 || true
    fi
    return "${rc}"
}
trap cleanup EXIT INT TERM

log "OLD_VERSION=${OLD_VERSION}  NEW_VERSION=${NEW_VERSION}  WORK_DIR=${WORK_DIR}"
rm -rf "${WORK_DIR}"
mkdir -p "${WORK_DIR}"/{home,fixture,install-bin}

export HOME="${WORK_DIR}/home"
mkdir -p "${HOME}/.local"

NEW_TARBALL_DEST="${WORK_DIR}/fixture/paneflow-${NEW_VERSION}-x86_64.tar.gz"
if [ -n "${E2E_NEW_TARBALL}" ]; then
    log "phase 1: using prebuilt NEW tarball from ${E2E_NEW_TARBALL}"
    [ -s "${E2E_NEW_TARBALL}" ] || fail "E2E_NEW_TARBALL points at missing/empty file: ${E2E_NEW_TARBALL}"
    cp "${E2E_NEW_TARBALL}" "${NEW_TARBALL_DEST}"
else
    log "phase 1: building NEW paneflow at v${NEW_VERSION}"
    ( cd "${REPO_ROOT}" && cargo build --release -p paneflow-app --quiet )
    log "phase 1: bundling tar.gz with bundle-tarball.sh"
    ( cd "${REPO_ROOT}" && ARCH=x86_64 bash scripts/bundle-tarball.sh "${NEW_VERSION}" >/dev/null )
    cp "${REPO_ROOT}/target/bundle/paneflow-${NEW_VERSION}-x86_64.tar.gz" "${NEW_TARBALL_DEST}"
fi
[ -s "${NEW_TARBALL_DEST}" ] || fail "NEW tarball not staged at ${NEW_TARBALL_DEST}"
NEW_TARBALL="${NEW_TARBALL_DEST}"

( cd "${WORK_DIR}/fixture" && sha256sum "paneflow-${NEW_VERSION}-x86_64.tar.gz" \
      > "paneflow-${NEW_VERSION}-x86_64.tar.gz.sha256" )

NEW_MINISIG_SRC="${E2E_NEW_MINISIG:-}"
if [ -z "${NEW_MINISIG_SRC}" ] && [ -n "${E2E_NEW_TARBALL}" ]; then
    NEW_MINISIG_SRC="${E2E_NEW_TARBALL}.minisig"
fi
if [ -n "${NEW_MINISIG_SRC}" ] && [ -s "${NEW_MINISIG_SRC}" ]; then
    cp "${NEW_MINISIG_SRC}" "${NEW_TARBALL_DEST}.minisig"
    log "phase 1: staged NEW tarball signature from ${NEW_MINISIG_SRC}"
elif [ "$(printf '%s\n' "0.3.9" "${OLD_VERSION}" | sort -V | head -n1)" = "0.3.9" ]; then
    log "phase 1: no .minisig for the NEW tarball and OLD v${OLD_VERSION} verifies fail-closed - harness no-op"
    exit 0
fi

WORKTREE_PATH=""
if [ -n "${E2E_OLD_TARBALL}" ]; then
    log "phase 2: using prebuilt OLD tarball from ${E2E_OLD_TARBALL}"
    [ -s "${E2E_OLD_TARBALL}" ] || fail "E2E_OLD_TARBALL points at missing/empty file: ${E2E_OLD_TARBALL}"
    OLD_EXTRACT_DIR="${WORK_DIR}/old-extract"
    mkdir -p "${OLD_EXTRACT_DIR}"
    tar xzf "${E2E_OLD_TARBALL}" -C "${OLD_EXTRACT_DIR}"
    OLD_BIN_SRC="${OLD_EXTRACT_DIR}/paneflow.app/bin/paneflow"
    [ -x "${OLD_BIN_SRC}" ] \
        || fail "OLD binary not found at expected layout ${OLD_BIN_SRC} (bundle-tarball.sh layout is paneflow.app/bin/paneflow)"
else
    WORKTREE_PATH="${WORK_DIR}/old-src"
    log "phase 2: checking out ${OLD_TAG} into ${WORKTREE_PATH}"
    git -C "${REPO_ROOT}" worktree add --detach "${WORKTREE_PATH}" "${OLD_TAG}"
    OLD_BUILD_TOOLCHAIN=""
    if [ -f "${REPO_ROOT}/rust-toolchain.toml" ]; then
        OLD_BUILD_TOOLCHAIN="$(awk -F'"' '/^channel/ { print $2; exit }' "${REPO_ROOT}/rust-toolchain.toml")"
    fi
    log "phase 2: building OLD paneflow at v${OLD_VERSION} (toolchain=${OLD_BUILD_TOOLCHAIN:-system default}, slow step)"
    (
        cd "${WORKTREE_PATH}"
        if [ -n "${OLD_BUILD_TOOLCHAIN}" ]; then
            RUSTUP_TOOLCHAIN="${OLD_BUILD_TOOLCHAIN}" cargo build --release -p paneflow-app --quiet
        else
            cargo build --release -p paneflow-app --quiet
        fi
    )
    OLD_BIN_SRC="${WORKTREE_PATH}/target/release/paneflow"
    [ -x "${OLD_BIN_SRC}" ] || fail "OLD binary not built at ${OLD_BIN_SRC}"
fi

INSTALL_DIR="${HOME}/.local/paneflow.app"
mkdir -p "${INSTALL_DIR}/bin"
cp "${OLD_BIN_SRC}" "${INSTALL_DIR}/bin/paneflow"
INSTALL_BIN="${INSTALL_DIR}/bin/paneflow"

actual_old="$("${INSTALL_BIN}" --version)"
[ "${actual_old}" = "paneflow ${OLD_VERSION}" ] \
    || fail "staged OLD binary reported '${actual_old}', expected 'paneflow ${OLD_VERSION}'"
log "phase 2: OLD binary staged at ${INSTALL_BIN}"

if ! strings "${INSTALL_BIN}" 2>/dev/null | grep -q -- "--update-and-exit"; then
    log "phase 2: OLD binary v${OLD_VERSION} predates --update-and-exit (added in commit 0e733e3, first shipped in v0.2.12)"
    log "phase 2: skipping update scenarios - re-engages automatically once OLD_VERSION advances to 0.2.12+"
    ok "self-bootstrap: harness no-op until next release cycle"
    exit 0
fi

HTTP_LOG="${WORK_DIR}/http-server.log"
log "phase 3: starting python3 -m http.server in ${WORK_DIR}/fixture (port=${HTTP_PORT})"
( cd "${WORK_DIR}/fixture" && exec python3 -u -m http.server "${HTTP_PORT}" --bind 127.0.0.1 ) \
    >"${HTTP_LOG}" 2>&1 &
HTTP_PID=$!

for _ in $(seq 1 50); do
    if grep -E 'Serving HTTP on 127\.0\.0\.1 port [0-9]+' "${HTTP_LOG}" >/dev/null 2>&1; then
        break
    fi
    sleep 0.1
done
HTTP_PORT_ACTUAL="$(grep -oE 'port [0-9]+' "${HTTP_LOG}" | head -n1 | awk '{print $2}')"
[ -n "${HTTP_PORT_ACTUAL}" ] || fail "http.server did not announce a port within 5s"
FEED_BASE="http://127.0.0.1:${HTTP_PORT_ACTUAL}"
log "phase 3: server up at ${FEED_BASE}"

LATEST_JSON="${WORK_DIR}/fixture/latest"
cat > "${LATEST_JSON}" <<EOF
{
  "tag_name": "v${NEW_VERSION}",
  "html_url": "${FEED_BASE}/release-page-stub",
  "assets": [
    {
      "name": "paneflow-${NEW_VERSION}-x86_64.tar.gz",
      "browser_download_url": "${FEED_BASE}/paneflow-${NEW_VERSION}-x86_64.tar.gz"
    }
  ]
}
EOF

reset_install() {
    rm -rf "${INSTALL_DIR}" "${HOME}/.cache/paneflow"
    mkdir -p "${INSTALL_DIR}/bin"
    cp "${OLD_BIN_SRC}" "${INSTALL_BIN}"
}

run_happy() {
    log "scenario: tar.gz happy path"
    reset_install

    set +e
    PANEFLOW_UPDATE_FEED_URL="${FEED_BASE}/latest" \
    RUST_LOG=info \
        "${INSTALL_BIN}" --update-and-exit \
        > "${WORK_DIR}/happy.stdout" 2> "${WORK_DIR}/happy.stderr"
    rc=$?
    set -e

    [ "${rc}" -eq 0 ] || {
        log "happy: stderr:"; cat "${WORK_DIR}/happy.stderr" >&2
        fail "happy: --update-and-exit returned ${rc}, expected 0"
    }
    actual_new="$("${INSTALL_BIN}" --version)"
    [ "${actual_new}" = "paneflow ${NEW_VERSION}" ] \
        || fail "happy: post-swap version is '${actual_new}', expected 'paneflow ${NEW_VERSION}'"
    ok "tar.gz happy path: v${OLD_VERSION} → v${NEW_VERSION}"
}

run_hash_mismatch() {
    log "scenario: tampered artifact (integrity mismatch)"
    reset_install

    tarball_path="${WORK_DIR}/fixture/paneflow-${NEW_VERSION}-x86_64.tar.gz"
    tarball_backup="${tarball_path}.real"
    cp "${tarball_path}" "${tarball_backup}"
    printf 'tampered-by-e2e-harness' >> "${tarball_path}"

    set +e
    PANEFLOW_UPDATE_FEED_URL="${FEED_BASE}/latest" \
    RUST_LOG=info \
        "${INSTALL_BIN}" --update-and-exit \
        > "${WORK_DIR}/mismatch.stdout" 2> "${WORK_DIR}/mismatch.stderr"
    rc=$?
    set -e

    mv "${tarball_backup}" "${tarball_path}"

    [ "${rc}" -eq 4 ] || {
        log "mismatch: stderr:"; cat "${WORK_DIR}/mismatch.stderr" >&2
        fail "mismatch: --update-and-exit returned ${rc}, expected 4"
    }
    actual_unchanged="$("${INSTALL_BIN}" --version)"
    [ "${actual_unchanged}" = "paneflow ${OLD_VERSION}" ] \
        || fail "mismatch: post-fail version is '${actual_unchanged}', expected unchanged 'paneflow ${OLD_VERSION}'"
    ok "tampered artifact: rejected, install path unchanged"
}

run_feed_unreachable() {
    log "scenario: feed unreachable"
    reset_install

    kill "${HTTP_PID}" 2>/dev/null || true
    wait "${HTTP_PID}" 2>/dev/null || true
    HTTP_PID=""

    set +e
    PANEFLOW_UPDATE_FEED_URL="http://127.0.0.1:1/latest" \
    RUST_LOG=info \
        "${INSTALL_BIN}" --update-and-exit \
        > "${WORK_DIR}/unreach.stdout" 2> "${WORK_DIR}/unreach.stderr"
    rc=$?
    set -e

    [ "${rc}" -eq 3 ] || {
        log "unreach: stderr:"; cat "${WORK_DIR}/unreach.stderr" >&2
        fail "unreach: --update-and-exit returned ${rc}, expected 3 (feed unreachable)"
    }
    grep -F "feed unreachable" "${WORK_DIR}/unreach.stderr" >/dev/null \
        || fail "unreach: stderr missing explicit 'feed unreachable' substring (AC6)"
    ok "feed unreachable: explicit error surfaced"
}

case "${SCENARIO}" in
    all|happy)             run_happy ;;
esac
case "${SCENARIO}" in
    all|hash_mismatch)     run_hash_mismatch ;;
esac
case "${SCENARIO}" in
    all|feed_unreachable)  run_feed_unreachable ;;
esac

log "all scenarios passed"

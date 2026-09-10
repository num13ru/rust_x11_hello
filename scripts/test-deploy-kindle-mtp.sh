#!/usr/bin/env bash
set -euo pipefail

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
TEST_TMP_DIR="$(mktemp -d "${TMPDIR:-/tmp}/rust-x11-hello-deploy-test.XXXXXX")"

cleanup() {
    if [[ -n "$TEST_TMP_DIR" && -d "$TEST_TMP_DIR" ]]; then
        rm -rf -- "$TEST_TMP_DIR"
    fi
}

trap cleanup EXIT

LOCAL_PACKAGE="${TEST_TMP_DIR}/package/rust_x11_hello"
FAKE_MTP_BIN="${TEST_TMP_DIR}/fake-mtp-rs"
cp "${REPO_ROOT}/scripts/test-fixtures/fake-mtp-rs.sh" "$FAKE_MTP_BIN"
chmod +x "$FAKE_MTP_BIN"

mkdir -p "${LOCAL_PACKAGE}/bin"
printf 'new-binary' >"${LOCAL_PACKAGE}/bin/rust_x11_hello"
printf 'run' >"${LOCAL_PACKAGE}/bin/run.sh"
printf 'config' >"${LOCAL_PACKAGE}/config.xml"
printf 'menu' >"${LOCAL_PACKAGE}/menu.json"

prepare_remote() {
    local fake_root="$1"
    mkdir -p "${fake_root}/extensions/rust_x11_hello/bin"
    printf 'old-binary' >"${fake_root}/extensions/rust_x11_hello/bin/rust_x11_hello"
    printf 'retired-stop' >"${fake_root}/extensions/rust_x11_hello/bin/stop.sh"
    printf 'retired-show' >"${fake_root}/extensions/rust_x11_hello/bin/show.sh"
}

run_update() {
    local fake_root="$1"
    local fake_log="$2"
    shift 2
    env \
        FAKE_MTP_ROOT="$fake_root" \
        FAKE_MTP_LOG="$fake_log" \
        LOCAL_EXT="$LOCAL_PACKAGE" \
        MTP_RS_BIN="$FAKE_MTP_BIN" \
        "$@" \
        "${REPO_ROOT}/scripts/deploy-kindle-mtp.sh" update --confirm-stopped
}

SUCCESS_ROOT="${TEST_TMP_DIR}/success-remote"
SUCCESS_LOG="${TEST_TMP_DIR}/success.log"
SUCCESS_OUTPUT="${TEST_TMP_DIR}/success.output"
prepare_remote "$SUCCESS_ROOT"
run_update "$SUCCESS_ROOT" "$SUCCESS_LOG" >"$SUCCESS_OUTPUT"

cmp "${LOCAL_PACKAGE}/bin/rust_x11_hello" \
    "${SUCCESS_ROOT}/extensions/rust_x11_hello/bin/rust_x11_hello"
printf 'old-binary' | cmp - \
    "${SUCCESS_ROOT}/extensions/rust_x11_hello/bin/rust_x11_hello.previous"
test ! -e "${SUCCESS_ROOT}/extensions/rust_x11_hello/bin/rust_x11_hello.new"
test ! -e "${SUCCESS_ROOT}/extensions/rust_x11_hello/bin/stop.sh"
test ! -e "${SUCCESS_ROOT}/extensions/rust_x11_hello/bin/show.sh"
grep -Fq 'rm /extensions/rust_x11_hello/bin/rust_x11_hello.new --yes' "$SUCCESS_LOG"
grep -Fq 'rm /extensions/rust_x11_hello/bin/stop.sh --yes' "$SUCCESS_LOG"
grep -Fq 'rm /extensions/rust_x11_hello/bin/show.sh --yes' "$SUCCESS_LOG"
if grep -Fq 'rename ' "$SUCCESS_LOG"; then
    printf 'unexpected rename command in successful update\n' >&2
    exit 1
fi

FAILURE_ROOT="${TEST_TMP_DIR}/failure-remote"
FAILURE_LOG="${TEST_TMP_DIR}/failure.log"
FAILURE_OUTPUT="${TEST_TMP_DIR}/failure.output"
prepare_remote "$FAILURE_ROOT"
if run_update "$FAILURE_ROOT" "$FAILURE_LOG" FAKE_FAIL_NEW_ACTIVATION=1 \
    >"$FAILURE_OUTPUT" 2>&1; then
    printf 'forced activation failure unexpectedly succeeded\n' >&2
    exit 1
fi

printf 'old-binary' | cmp - \
    "${FAILURE_ROOT}/extensions/rust_x11_hello/bin/rust_x11_hello"
printf 'old-binary' | cmp - \
    "${FAILURE_ROOT}/extensions/rust_x11_hello/bin/rust_x11_hello.previous"
test "$(grep -Fc ' /extensions/rust_x11_hello/bin/rust_x11_hello --replace --verify' \
    "$FAILURE_LOG")" -eq 2
grep -Fq 'Rollback restored the previous binary.' "$FAILURE_OUTPUT"

REMOVAL_FAILURE_ROOT="${TEST_TMP_DIR}/removal-failure-remote"
REMOVAL_FAILURE_LOG="${TEST_TMP_DIR}/removal-failure.log"
REMOVAL_FAILURE_OUTPUT="${TEST_TMP_DIR}/removal-failure.output"
prepare_remote "$REMOVAL_FAILURE_ROOT"
run_update "$REMOVAL_FAILURE_ROOT" "$REMOVAL_FAILURE_LOG" \
    FAKE_FAIL_RETIRED_REMOVAL=show.sh >"$REMOVAL_FAILURE_OUTPUT" 2>&1

cmp "${LOCAL_PACKAGE}/bin/rust_x11_hello" \
    "${REMOVAL_FAILURE_ROOT}/extensions/rust_x11_hello/bin/rust_x11_hello"
test ! -e "${REMOVAL_FAILURE_ROOT}/extensions/rust_x11_hello/bin/stop.sh"
test -e "${REMOVAL_FAILURE_ROOT}/extensions/rust_x11_hello/bin/show.sh"
grep -Fq 'WARNING: new menu installed, but retired show.sh cleanup failed.' \
    "$REMOVAL_FAILURE_OUTPUT"

STOP_REMOVAL_FAILURE_ROOT="${TEST_TMP_DIR}/stop-removal-failure-remote"
STOP_REMOVAL_FAILURE_LOG="${TEST_TMP_DIR}/stop-removal-failure.log"
STOP_REMOVAL_FAILURE_OUTPUT="${TEST_TMP_DIR}/stop-removal-failure.output"
prepare_remote "$STOP_REMOVAL_FAILURE_ROOT"
run_update "$STOP_REMOVAL_FAILURE_ROOT" "$STOP_REMOVAL_FAILURE_LOG" \
    FAKE_FAIL_RETIRED_REMOVAL=stop.sh >"$STOP_REMOVAL_FAILURE_OUTPUT" 2>&1

cmp "${LOCAL_PACKAGE}/bin/rust_x11_hello" \
    "${STOP_REMOVAL_FAILURE_ROOT}/extensions/rust_x11_hello/bin/rust_x11_hello"
test -e "${STOP_REMOVAL_FAILURE_ROOT}/extensions/rust_x11_hello/bin/stop.sh"
test ! -e "${STOP_REMOVAL_FAILURE_ROOT}/extensions/rust_x11_hello/bin/show.sh"
grep -Fq 'WARNING: new menu installed, but retired stop.sh cleanup failed.' \
    "$STOP_REMOVAL_FAILURE_OUTPUT"

printf 'deploy-kindle-mtp mock success, rollback, and retired-file checks passed\n'

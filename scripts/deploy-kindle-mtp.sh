#!/usr/bin/env bash
set -euo pipefail

MTP_RS_BIN="${MTP_RS_BIN:-mtp-rs}"
LOCAL_EXT="kindle-extension/rust_x11_hello"
LOCAL_BIN="${LOCAL_EXT}/bin/rust_x11_hello"
REMOTE_EXT="/extensions/rust_x11_hello"
REMOTE_BIN_DIR="${REMOTE_EXT}/bin"
REMOTE_BIN="${REMOTE_BIN_DIR}/rust_x11_hello"
REMOTE_STAGED_BIN="${REMOTE_BIN}.new"
REMOTE_PREVIOUS_BIN="${REMOTE_BIN}.previous"

MODE="${1:-}"
CONFIRMATION="${2:-}"

usage() {
    cat <<'EOF'
Usage:
  scripts/deploy-kindle-mtp.sh install
  scripts/deploy-kindle-mtp.sh update --confirm-stopped

install refuses to touch an existing /extensions/rust_x11_hello.
update requires an explicit stopped-process confirmation and retains the
previous binary as rust_x11_hello.previous; it refuses to overwrite that backup.
EOF
}

mtp() {
    "$MTP_RS_BIN" "$@"
}

remote_name_exists() {
    local remote_folder="$1"
    local remote_name="$2"

    mtp ls "$remote_folder" | awk -v expected="$remote_name" '
        $NF == expected { found = 1 }
        END { exit(found ? 0 : 1) }
    '
}

require_local_file() {
    if [[ ! -f "$1" ]]; then
        printf 'ERROR: required package file is missing: %s\n' "$1" >&2
        exit 1
    fi
}

upload_package_files() {
    mtp put "${LOCAL_EXT}/bin/run.sh" "${REMOTE_BIN_DIR}/run.sh" --replace --verify
    mtp put "${LOCAL_EXT}/bin/show.sh" "${REMOTE_BIN_DIR}/show.sh" --replace --verify
    mtp put "${LOCAL_EXT}/bin/stop.sh" "${REMOTE_BIN_DIR}/stop.sh" --replace --verify
    mtp put "${LOCAL_EXT}/config.xml" "${REMOTE_EXT}/config.xml" --replace --verify

    # Upload menu.json last so KUAL does not expose a partially installed package.
    mtp put "${LOCAL_EXT}/menu.json" "${REMOTE_EXT}/menu.json" --replace --verify
}

case "$MODE" in
    install)
        if [[ -n "$CONFIRMATION" ]]; then
            usage >&2
            exit 2
        fi
        ;;
    update)
        if [[ "$CONFIRMATION" != "--confirm-stopped" ]]; then
            printf 'ERROR: update requires --confirm-stopped\n' >&2
            usage >&2
            exit 2
        fi
        ;;
    *)
        usage >&2
        exit 2
        ;;
esac

for required_file in \
    "$LOCAL_BIN" \
    "${LOCAL_EXT}/bin/run.sh" \
    "${LOCAL_EXT}/bin/show.sh" \
    "${LOCAL_EXT}/bin/stop.sh" \
    "${LOCAL_EXT}/config.xml" \
    "${LOCAL_EXT}/menu.json"
do
    require_local_file "$required_file"
done

if ! command -v "$MTP_RS_BIN" >/dev/null 2>&1; then
    printf 'ERROR: mtp-rs executable not found: %s\n' "$MTP_RS_BIN" >&2
    exit 1
fi

case "$MODE" in
    install)
        mtp ls /extensions >/dev/null
        if remote_name_exists /extensions rust_x11_hello; then
            printf 'ERROR: %s already exists; install mode refuses replacement\n' "$REMOTE_EXT" >&2
            exit 1
        fi

        mtp mkdir "$REMOTE_EXT"
        mtp mkdir "$REMOTE_BIN_DIR"
        mtp put "$LOCAL_BIN" "$REMOTE_BIN" --verify
        upload_package_files
        ;;
    update)
        mtp ls "$REMOTE_BIN_DIR" >/dev/null
        if ! remote_name_exists "$REMOTE_BIN_DIR" rust_x11_hello; then
            printf 'ERROR: installed binary is missing: %s\n' "$REMOTE_BIN" >&2
            exit 1
        fi
        if remote_name_exists "$REMOTE_BIN_DIR" rust_x11_hello.previous; then
            printf 'ERROR: retained backup already exists: %s\n' "$REMOTE_PREVIOUS_BIN" >&2
            printf 'Download and remove or rename it explicitly before another update.\n' >&2
            exit 1
        fi

        mtp put "$LOCAL_BIN" "$REMOTE_STAGED_BIN" --replace --verify
        upload_package_files
        mtp rename "$REMOTE_BIN" "rust_x11_hello.previous"
        if ! mtp rename "$REMOTE_STAGED_BIN" "rust_x11_hello"; then
            printf 'ERROR: activating staged binary failed; attempting rollback\n' >&2
            mtp rename "$REMOTE_PREVIOUS_BIN" "rust_x11_hello" || true
            exit 1
        fi
        ;;
esac

mtp ls "$REMOTE_EXT" --recursive
printf '\nHost artifact checksum:\n'
shasum -a 256 "$LOCAL_BIN"
printf '\nDeployment transfer completed with mtp-rs readback verification.\n'
printf 'Runtime path: /mnt/us/extensions/rust_x11_hello/bin/rust_x11_hello\n'

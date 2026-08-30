#!/usr/bin/env bash
set -euo pipefail

: "${FAKE_MTP_ROOT:?FAKE_MTP_ROOT is required}"
: "${FAKE_MTP_LOG:?FAKE_MTP_LOG is required}"

command_name="${1:-}"
shift || true
printf '%s %s\n' "$command_name" "$*" >>"$FAKE_MTP_LOG"

remote_path() {
    case "$1" in
        /*) printf '%s%s\n' "$FAKE_MTP_ROOT" "$1" ;;
        *)
            printf 'fake-mtp-rs: remote path must be absolute: %s\n' "$1" >&2
            exit 2
            ;;
    esac
}

case "$command_name" in
    ls)
        source_path="$(remote_path "$1")"
        [[ -d "$source_path" ]] || exit 1
        shopt -s nullglob
        for entry in "$source_path"/*; do
            if [[ -d "$entry" ]]; then
                printf 'DIR 0 %s\n' "${entry##*/}"
            else
                printf 'FILE %s %s\n' "$(wc -c <"$entry")" "${entry##*/}"
            fi
        done
        ;;
    get)
        source_path="$(remote_path "$1")"
        destination_path="$2"
        cp "$source_path" "$destination_path"
        ;;
    put)
        source_path="$1"
        destination_path="$(remote_path "$2")"
        if [[ "${FAKE_FAIL_NEW_ACTIVATION:-0}" == 1 \
            && "$2" == "/extensions/rust_x11_hello/bin/rust_x11_hello" \
            && "$(<"$source_path")" == new-binary ]]; then
            printf 'fake-mtp-rs: forced activation failure\n' >&2
            exit 1
        fi
        mkdir -p "${destination_path%/*}"
        cp "$source_path" "$destination_path"
        ;;
    rm)
        destination_path="$(remote_path "$1")"
        [[ " ${*:2} " == *" --yes "* ]] || {
            printf 'fake-mtp-rs: rm requires --yes\n' >&2
            exit 2
        }
        rm -f -- "$destination_path"
        ;;
    mkdir)
        mkdir -p "$(remote_path "$1")"
        ;;
    rename)
        printf 'fake-mtp-rs: rename unsupported\n' >&2
        exit 1
        ;;
    *)
        printf 'fake-mtp-rs: unsupported command: %s\n' "$command_name" >&2
        exit 2
        ;;
esac

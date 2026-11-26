#!/usr/bin/env bash
set -euo pipefail
export LC_ALL=C

BIN="${1:-kindle-extension/rust_x11_hello/bin/rust_x11_hello}"
case "$BIN" in
    /*) ;;
    *) BIN="./$BIN" ;;
esac

fail() {
    printf 'ERROR: %s: %s\n' "$1" "$BIN" >&2
    exit 1
}

[[ -f "$BIN" ]] || fail 'binary not found'
[[ -s "$BIN" ]] || fail 'binary is empty'

if command -v llvm-readelf >/dev/null 2>&1; then
    reader=llvm-readelf
elif command -v readelf >/dev/null 2>&1; then
    reader=readelf
else
    fail 'install llvm-readelf (LLVM) or readelf (GNU binutils), or use nix develop'
fi

magic=$(od -An -t x1 -N 4 "$BIN" | awk 'NF == 4 { print $1, $2, $3, $4 }')
[[ "$magic" == '7f 45 4c 46' ]] || fail 'expected an ELF file'

diagnostics=$(mktemp)
trap 'rm -f "$diagnostics"' EXIT
if ! elf=$("$reader" -W -h -l -d -V -A "$BIN" 2>"$diagnostics") || [[ -s "$diagnostics" ]]; then
    cat "$diagnostics" >&2
    fail "$reader could not inspect the ELF file"
fi

require_match() {
    grep -Eq "$1" <<<"$elf" || fail "$2"
}

require_match '^[[:space:]]*Class:[[:space:]]+ELF32$' 'expected ELF32'
require_match '^[[:space:]]*Data:.*little endian' 'expected little-endian ELF'
require_match '^[[:space:]]*Machine:[[:space:]]+ARM$' 'expected ARM'
require_match '^[[:space:]]*Type:[[:space:]]+(EXEC|DYN)[[:space:]]' 'expected an executable or static PIE'

entry=$(awk '$1 == "Entry" && $2 == "point" && $3 == "address:" { print $4 }' <<<"$elf")
[[ "$entry" =~ ^0x[[:xdigit:]]{1,8}$ ]] || fail 'invalid entry point'
(( entry != 0 )) || fail 'missing entry point'
load_segments=$(awk '$1 == "LOAD" { print $2, $3, $5, $6, $(NF - 1) }' <<<"$elf")
file_size=$(wc -c <"$BIN")
entry_in_load=false
while read -r offset address file_bytes memory_bytes permissions; do
    for field in "$offset" "$address" "$file_bytes" "$memory_bytes"; do
        [[ "$field" =~ ^0x[[:xdigit:]]{1,8}$ ]] || fail 'invalid ELF load segment'
    done
    (( file_bytes <= memory_bytes && offset + file_bytes <= file_size )) || fail 'invalid ELF load segment size'
    # Thumb entry points set the low bit in the ELF header.
    if [[ "$permissions" =~ ^[RW]*E[RW]*$ ]] \
        && (( (entry & ~1) >= address && (entry & ~1) < address + file_bytes )); then
        entry_in_load=true
    fi
done <<<"$load_segments"
$entry_in_load || fail 'entry point is outside the executable load segments'

flags=$(awk '$1 == "Flags:" { print $2 }' <<<"$elf")
flags=${flags%,}
[[ "$flags" =~ ^0x[[:xdigit:]]{1,8}$ ]] || fail 'invalid ARM ELF flags'
(( (flags & 0xff000600) == 0x05000400 )) || fail 'expected ARM EABI5 hard-float flags'
require_match '^[[:space:]]*(Tag_CPU_arch:[[:space:]]+v7|Description:[[:space:]]+ARM v7)$' 'expected Arm v7 attributes'
require_match '^[[:space:]]*(Tag_CPU_arch_profile:|Description:)[[:space:]]+Application$' 'expected Arm Application profile'
require_match '^[[:space:]]*(Tag_FP_arch:|Description:)[[:space:]]+VFPv3-D16$' 'expected VFPv3-D16 attributes'

if grep -Eq '^[[:space:]]*INTERP[[:space:]]' <<<"$elf"; then
    fail 'binary has a PT_INTERP dynamic interpreter'
fi
if grep -Eq 'GLIBC_[[:alnum:]_.]+' <<<"$elf"; then
    fail 'binary has GLIBC symbol versions'
fi
if grep -Eq '\(NEEDED\)' <<<"$elf"; then
    fail 'binary has DT_NEEDED shared-library dependencies'
fi

printf 'Verified static Arm v7-A EABI5 hard-float ELF: %s\n' "$BIN"

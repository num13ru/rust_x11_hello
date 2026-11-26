#!/usr/bin/env bash
set -euo pipefail

CHECKER="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)/check-static.sh"
FIXTURES=$(mktemp -d "${TMPDIR:-/tmp}/paperpad-elf-test.XXXXXX")
trap 'rm -rf "$FIXTURES"' EXIT
cd "$FIXTURES"

host=$(rustc -vV | awk '$1 == "host:" { print $2 }')
linker="$(rustc --print sysroot)/lib/rustlib/$host/bin/rust-lld"

assemble_arm() {
    llvm-mc -triple=armv7-unknown-linux-musleabihf -filetype=obj -o "$1"
}

link() {
    "$linker" -flavor gnu "$@"
}

assemble_arm entry.o <<'EOF'
.syntax unified
.arch armv7-a
.fpu vfpv3-d16
.eabi_attribute 28, 1
.globl _start
.type _start, %function
_start:
    bx lr
EOF

assemble_arm library.o <<'EOF'
.globl fixture
.type fixture, %function
fixture:
    bx lr
EOF

assemble_arm caller.o <<'EOF'
.syntax unified
.arch armv7-a
.fpu vfpv3-d16
.eabi_attribute 28, 1
.globl _start
.type _start, %function
_start:
    b fixture
EOF

link entry.o -o static
link entry.o -pie --no-dynamic-linker -o static-pie
link entry.o -pie --dynamic-linker /lib/ld-linux-armhf.so.3 -o interpreter
link library.o -shared -soname libfixture.so -o libfixture.so
link caller.o libfixture.so -pie --no-dynamic-linker -o needed
printf 'GLIBC_2.3 { global: fixture; local: *; };\n' > glibc.map
link library.o -shared -soname libfixture.so --version-script glibc.map -o libversioned.so
link caller.o libversioned.so -pie --no-dynamic-linker -o glibc-version

printf '.globl _start\n_start:\n    ret\n' |
    llvm-mc -triple=i386-unknown-linux-gnu -filetype=obj -o x86.o
link x86.o -o wrong-machine
printf '.globl _main\n_main:\n    ret\n' |
    llvm-mc -triple=arm64-apple-macosx -filetype=obj -o macho.o
"$linker" -flavor darwin -arch arm64 -platform_version macos 14.0 14.0 -e _main macho.o -o macho

: > empty
head -c 52 static > truncated
cp static soft-float
printf '\000\002\000\005' | dd of=soft-float bs=1 seek=36 conv=notrunc 2>/dev/null
python3 - <<'PY'
import struct
from pathlib import Path

elf = bytearray(Path("static").read_bytes())
program_headers = struct.unpack_from("<I", elf, 28)[0]
header_size, header_count = struct.unpack_from("<HH", elf, 42)
for offset in range(program_headers, program_headers + header_count * header_size, header_size):
    if struct.unpack_from("<I", elf, offset)[0] == 1:  # PT_LOAD
        struct.pack_into("<II", elf, offset + 16, len(elf) + 1, len(elf) + 1)
        break
else:
    raise ValueError("No PT_LOAD fixture segment")
Path("oversized-segment").write_bytes(elf)
PY

reject() {
    local bin=$1 expected=$2 output
    if output=$(bash "$CHECKER" "$bin" 2>&1); then
        printf 'ERROR: Accepted %s\n' "$bin" >&2
        exit 1
    fi
    if [[ "$output" != *"$expected"* ]]; then
        printf 'ERROR: Expected %s for %s, got %s\n' "$expected" "$bin" "$output" >&2
        exit 1
    fi
}

bash "$CHECKER" static
bash "$CHECKER" static-pie
reject missing 'binary not found'
reject empty 'binary is empty'
reject truncated 'could not inspect'
reject oversized-segment 'invalid ELF load segment size'
reject macho 'expected an ELF file'
reject wrong-machine 'expected ARM'
reject soft-float 'expected ARM EABI5 hard-float flags'
reject interpreter 'PT_INTERP'
reject needed 'DT_NEEDED'
reject glibc-version 'GLIBC symbol versions'

mkdir tools
PATH="$FIXTURES/tools" "$(command -v bash)" "$CHECKER" static > missing-tool.log 2>&1 && exit 1
grep -q 'install llvm-readelf' missing-tool.log

cat > tools/llvm-readelf <<'EOF'
#!/bin/sh
echo 'injected inspection failure' >&2
exit 0
EOF
chmod +x tools/llvm-readelf
PATH="$FIXTURES/tools:$PATH" reject static 'could not inspect'
echo 'Static ELF verifier tests passed'

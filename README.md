# rust_hello_kual

Minimal Rust “Hello World” application launched from **KUAL** on a **Kindle Paperwhite 12**.

The goal of this project is to prove a simple pipeline:

```text
Rust source
  ↓
Docker cross-build
  ↓
static ARMv7 musl binary
  ↓
KUAL extension
  ↓
Kindle launch
  ↓
hello.log proof
```

## Verified device

Tested on:

```text
Device: Kindle Paperwhite 12
Kernel: Linux 5.15.41-lab126
Arch:   armv7l
CPU:    ARMv7 / Cortex-A7
libc:   GNU libc 2.20
Shell:  /bin/ash
KUAL:   installed and working
```

Runtime probe showed:

```text
/lib/ld-linux-armhf.so.3 -> ld-2.20.so
```

However, the final working binary does **not** depend on Kindle glibc. It is built as a **static musl** binary.

## Why musl?

The first GNU/glibc build used:

```text
armv7-unknown-linux-gnueabihf
```

It produced a correct ARM hard-float binary, but it required newer glibc symbols:

```text
GLIBC_2.28
GLIBC_2.32
GLIBC_2.33
GLIBC_2.34
```

Kindle Paperwhite 12 has:

```text
glibc 2.20
```

So the GNU build is not suitable as the default.

The working target is:

```text
armv7-unknown-linux-musleabihf
```

Expected result:

```text
ELF 32-bit LSB executable, ARM, EABI5, statically linked
No dynamic interpreter
No GLIBC symbols
```

## Project layout

```text
.
├── .cargo
│   └── config.toml
├── Cargo.lock
├── Cargo.toml
├── Dockerfile
├── Makefile
├── kindle-extension
│   └── rust_hello
│       ├── bin
│       │   ├── run.sh
│       │   ├── rust_hello
│       │   └── show.sh
│       ├── config.xml
│       └── menu.json
├── scripts
│   ├── build-kindle-armv7hf.sh
│   └── check-static.sh
└── src
    └── main.rs
```

## Requirements

On the host machine:

```text
Docker
make
```

On Kindle:

```text
KUAL
filesystem access to /mnt/us/extensions
```

This project assumes KUAL is already installed and working.

## Build

Build the Docker image:

```bash
make image
```

Build the Kindle extension:

```bash
make build
```

Verify the produced binary:

```bash
make verify
```

Expected verification output:

```text
Binary:
kindle-extension/rust_hello/bin/rust_hello: ELF 32-bit LSB executable, ARM, EABI5, statically linked, not stripped

Interpreter check:
OK: no dynamic interpreter

GLIBC symbols check:
OK: not a dynamic object

OK: static binary, no dynamic interpreter, no GLIBC symbols
```

## Install on Kindle

Copy this folder:

```text
kindle-extension/rust_hello
```

to the Kindle extensions directory:

```text
Kindle/extensions/rust_hello
```

On the device, the final path should be:

```text
/mnt/us/extensions/rust_hello
```

Expected extension layout on Kindle:

```text
/mnt/us/extensions/rust_hello/
├── bin
│   ├── run.sh
│   ├── rust_hello
│   └── show.sh
├── config.xml
└── menu.json
```

## Run

On Kindle:

```text
KUAL → Rust Hello → Run Rust Hello
```

After running, check:

```text
Kindle/extensions/rust_hello/hello.log
```

or on the device:

```text
/mnt/us/extensions/rust_hello/hello.log
```

Expected log:

```text
========================================
KUAL Rust Hello launcher
Date: ...
PWD: /mnt/us/extensions/rust_hello
UID/GID: uid=0(root) gid=0(root) groups=0(root)
BIN: /mnt/us/extensions/rust_hello/bin/rust_hello

---- running Rust binary ----
Hello from Rust on Kindle via KUAL. ts=...
---- Rust-side diagnostics ----
target_arch: arm
target_os: linux
current_dir: Ok("/mnt/us/extensions/rust_hello")
uname -a: Linux kindle 5.15.41-lab126 ... armv7l GNU/Linux
---- env ----
...
---- done ----

Rust binary exit status: 0
========================================
```

## KUAL extension files

The KUAL extension is stored in:

```text
kindle-extension/rust_hello
```

### `config.xml`

Defines the extension metadata and points KUAL to `menu.json`.

### `menu.json`

Defines KUAL menu items:

```text
Run Rust Hello
Show Last Result
```

### `bin/run.sh`

Shell wrapper launched by KUAL.

It runs the Rust binary and appends stdout/stderr to:

```text
/mnt/us/extensions/rust_hello/hello.log
```

### `bin/show.sh`

Small helper that uses `eips` to show the last log line on the Kindle screen.

## Build target

Current canonical target:

```text
armv7-unknown-linux-musleabihf
```

The project intentionally uses a static binary to avoid Kindle glibc compatibility problems.

## `.cargo/config.toml`

```toml
[target.armv7-unknown-linux-gnueabihf]
linker = "arm-linux-gnueabihf-gcc"

[target.armv7-unknown-linux-musleabihf]
linker = "rust-lld"
rustflags = [
  "-C", "target-feature=+crt-static"
]
```

The GNU target is kept only as a reference/fallback. The default working build should use:

```text
armv7-unknown-linux-musleabihf
```

## Docker

The Docker image contains the cross-build tools and verification tools.

The image is used for:

```text
cross-compilation
binary inspection
static linking verification
```

This avoids depending on host machine tools.

## Useful commands

```bash
make image
```

Build Docker image.

```bash
make build
```

Build static Kindle binary and copy it into the KUAL extension folder.

```bash
make verify
```

Verify that the binary is static and does not depend on glibc.

```bash
make shell
```

Open an interactive shell inside the builder container.

```bash
make clean
```

Remove build artifacts.

## Troubleshooting

### `GLIBC_2.xx not found`

You built the GNU/glibc version instead of the static musl version.

Use:

```text
armv7-unknown-linux-musleabihf
```

and verify:

```bash
make verify
```

### `not found`, but the file exists

For dynamic binaries this usually means the dynamic linker is missing or incompatible.

For this project, the expected binary is static. Check:

```bash
make verify
```

There should be no dynamic interpreter.

### `Permission denied`

KUAL launches:

```json
"action": "sh bin/run.sh"
```

so the shell script does not need to rely on executable bit. The Rust binary itself is made executable during build.

### No visible output on Kindle

The Rust binary writes to:

```text
/mnt/us/extensions/rust_hello/hello.log
```

KUAL does not provide a normal terminal stdout view.

### `objdump: not a dynamic object`

This is expected for the static musl binary.

It means the binary has no dynamic symbol table, which is good for this project.

## Safety notes

KUAL runs the script as root on the Kindle.

For this project, the Rust binary only writes inside:

```text
/mnt/us/extensions/rust_hello
```

Avoid writing to system directories such as:

```text
/
/usr
/var/local
/chroot
```

unless you know exactly what you are doing.

## Current status

Working:

```text
Rust static ARMv7 musl binary
KUAL shell launcher
Kindle Paperwhite 12 runtime
hello.log output
eips screen feedback
Docker-based build
static binary verification
```

Not implemented:

```text
GUI
GTK/X11 integration
```


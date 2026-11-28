# Kindle build validation history

## Native-first validation (2026-11-27)

| Apple Silicon macOS environment | Workspace checks | Kindle build and ELF verification |
| --- | --- | --- |
| Nix 2.36.0, plain Cargo | Passed, 301 Rust tests plus script checks | Passed with the default backend and an unavailable container CLI |
| Nix 2.36.0, MBX v1.31.0 | Passed, same checks | Passed with the default backend. Cargo used `rust-lld` and `+crt-static`. |
| Docker Desktop 29.9.3 | Host checks run in Nix | Passed |
| Apple Container 1.7.1 | Host checks run in Nix | Passed with `PROXY=true` |

`nix flake check` passed for Apple Silicon. Linux and Intel macOS were outside
this run. All four builds used Rust and Cargo 1.97.1 and produced little-endian
ELF32 Arm v7-A EABI5 hard-float executables. The verifier found no dynamic
interpreter, shared-library dependencies, or GLIBC versions. Plain Cargo and
MBX produced identical 1,211,204-byte binaries that matched the recorded
Paperwhite 6 readback. Docker and Apple Container produced identical
1,211,120-byte binaries. The recorded hardware run used plain native Cargo.
[Paperwhite 6 device evidence](mxcfb-device-evidence.md#native-nix-build-2026-11-27)

The MBX shell resolved `cargo` to the upstream wrapper and resolved `rustc`
and underlying Cargo to the Nix toolchain. `mbx doctor` reported zero failures.
Its setup warning refers to the global shim. It also reported that the test
filesystem did not refresh access times, so MBX skipped unused-unit collection
there. Switching between plain Cargo and MBX worked, and an intentional Rust
compilation failure preserved the previous package. A rebuild after reverting
that failure passed ELF verification. PaperSpoon built as a separate macOS
ARM64 executable, and the Kindle verifier correctly rejected it.

### Local cache measurements

Single wall-clock samples with downloaded dependencies, the same `Cargo.lock`,
and the default release profile for the ARMv7 musl target. The first MBX shell
provisioning and diagnostics took about 4 minutes 41 seconds. The plain shell
was already provisioned.

| Operation | Plain Cargo | MBX |
| --- | ---: | ---: |
| Warm Nix shell activation | 0.50 s | 0.98 s |
| Clean release build | 2.66 s | 5.41 s |
| Repeated build | 0.17 s | 0.20 s |
| Build after a comment-only source edit | 0.98 s | 1.03 s |
| Fresh second checkout | 2.19 s | 2.87 s |
| ELF verification | 0.06 s | 0.06 s |

`mbx stats` recorded three compiler cache hits in the second checkout. Clearing
its managed target with `mbx clean` and running `make clean-target` and
`make build` yielded three more hits in a 2.74 s rebuild. `mbx cache stats`
reported 35.4 MiB of logical data for two checkouts, including their managed
targets. Each plain Cargo target directory used 13.5 MiB on disk. The MBX cache
reused compiler outputs in this run, and plain Cargo had lower wall-clock build
times in all measured scenarios.

## Original containerless build validation

### Build audit

`cargo tree --locked --target armv7-unknown-linux-musleabihf --package rust_x11_hello`
resolves `anyhow` 1.0.102, `libc` 0.2.186, and the local `paper-protocol`
crate. The Kindle build uses the Rust target standard library, the bundled
musl runtime, and `rust-lld` through `.cargo/config.toml`. Neither registry
crate compiles C code, and the target graph has no procedural macros or
external native library dependencies. The existing GNU cross-compiler and
cross-libc packages correspond to the separate GNU target configuration. The
old verifier uses GNU `readelf` and ARM `objdump`.

The Rust 2024 edition requires Rust 1.85 or later. Docker's Rust 1.86 builds
the Kindle package. The host workspace has a higher requirement, including
`cosmic-text` 0.19.0 (Rust 1.89 or later). Host checks also use Bash, Python,
`jq`, and loopback networking. The old build script queries `rustup`, assumes
Cargo's default `target/` output path, and copies the binary before verifying
it. Make mounts that checkout at `/work` and a backend-specific dependency
cache at `/cargo`. MTP deployment reads the canonical extension directory and
uses independent USB tooling. It does not invoke Cargo or a container.

### Reference build

An isolated checkout of `aaeaf22` built with Docker Desktop 29.9.3 on Apple
Silicon using the existing AArch64 Linux builder. Rust and Cargo were both
1.86.0. The linker was the toolchain's `rust-lld` with `+crt-static`. The
initial build took 8.22 seconds with a cached image and dependency cache and
an empty Cargo target directory. Existing verification took 0.20 seconds.

GNU `readelf` reported ELF32, little-endian ARM, `ET_EXEC`, EABI5 hard-float
(`0x05000400`), Arm v7 Application profile, Thumb-2, VFPv3-D16, and VFP-register
arguments. There was no interpreter, dynamic section, or symbol version
information. The binary was 1,208,776 bytes and matched the checkout's
existing local deployment binary byte for byte. The reference build was kept
outside the working checkout.

### Native toolchain

The Nix shell pins Rust and Cargo 1.97.1 via `rust-overlay`, along with the
`armv7-unknown-linux-musleabihf` standard library. Rust's Darwin `rust-lld`
linked a smoke program and then the full PaperPad executable with the existing
target configuration and `+crt-static`. The Rust distribution supplied the
musl startup objects, `libc.a`, and `libunwind.a`. This target build requires
no separate C cross-compiler, external target sysroot, or container runtime.

| Nix package | Purpose |
| --- | --- |
| Rust minimal profile, `rustfmt`, Clippy, ARMv7 target | Compile PaperPad, compile and check host workspace code |
| GNU Make and Bash | Run the shared build recipes and scripts |
| LLVM | Inspect ELF with `llvm-readelf` and assemble regression fixtures with `llvm-mc` |
| `file` | Report the verified binary format during packaging |
| `jq` | Check the KUAL menu and read Cargo's executable path |
| Python 3 | Run the Apple Container proxy and latency analyzer checks |

The shared build now checks the active compiler's target library directory,
builds with `Cargo.lock`, and uses Cargo's reported executable path. It
verifies a temporary copy in the extension directory before atomically
publishing it. Cargo failure and an injected invalid artifact both left the
previous binary in place and left no staging file. The isolated native build
also passed with a custom Cargo target directory containing a space.

### Build matrix

| Host | Backend | Result |
| --- | --- | --- |
| Apple Silicon macOS | Native Nix 2.36.0 | Built and verified in an isolated checkout. `make check` passed 301 Rust tests and the shell and mock deployment checks. |
| Apple Silicon macOS | Docker Desktop 29.9.3 | Built and verified. Final executable matched the pre-change container binary. |
| Apple Silicon macOS | Apple Container 1.7.1 | Built and verified. Final executable matched the pre-change container binary. Initial network provisioning needed `PROXY=true`. |
| Intel macOS | Native Nix | Shell evaluation passed. Build untested on this host. |
| x86_64 Linux | Native Nix | Shell evaluation passed. Build untested on this host. |
| ARM64 Linux | Native Nix | Shell evaluation passed. Build untested on this host. |

`nix flake check` and `nix flake check --all-systems --no-build` passed with
the final pin. The first Nixpkgs unstable pin failed Intel macOS evaluation
because 26.11 dropped that platform. The final pin uses
`nixpkgs-26.05-darwin`. Apple Container was stopped for the final native build.
Docker had active workloads, so the native run used failing CLI stubs and an
invalid Docker socket. Neither stub was invoked, and no container cache or
image stamp was created during the native run.

### ELF comparison

The reference and native artifacts both reported ELF32 little-endian ARM
`ET_EXEC`, Arm v7 Application profile, Thumb-2, VFPv3-D16, and EABI5 hard-float
flags `0x05000400`. LLVM and GNU inspection accepted both with no interpreter,
shared-library dependencies, or `GLIBC_` version requirements. The Rust 1.97.1
native artifact was 2,428 bytes larger.

| Artifact | Bytes | SHA-256 |
| --- | ---: | --- |
| Docker reference and both final container builds | 1,208,776 | `4e0967b8828e39002980c5332910d0b683f34878f735ac94f0aa1371c4d8d00a` |
| Native Nix | 1,211,204 | `bfbf40545347ee4703f256372590d91a9d35d6d14b420334b8f821c714e82126` |

The regression suite exercises static `ET_EXEC` and static PIE (`ET_DYN`),
missing and empty files, Mach-O, non-ARM and soft-float ELF, corrupt segment
sizes, truncated ELF, dynamic interpreter and shared-library tags, GLIBC
versions, missing inspectors, and inspector diagnostics. It passed with both
LLVM and GNU `readelf`. Run it with
`nix develop -c bash scripts/test-check-static.sh`.

### Build timings

Three sequential samples per backend on the same Apple Silicon host, shown
as median wall-clock seconds. All used the release profile, eight Cargo jobs,
and an already populated dependency cache with Cargo offline. Containers ran
with eight vCPUs and 2 GiB of RAM. Native builds had access to host memory and
used Rust 1.97.1. Containers used Rust 1.86.0. Release incremental compilation
was disabled. The source-change test added a comment after a clean build to
rebuild PaperPad with dependency artifacts cached.

| Backend | Clean build | Source-change rebuild | Cached build | `make verify` | Cargo build output |
| --- | ---: | ---: | ---: | ---: | ---: |
| Native Nix | 2.33 s | 1.13 s | 0.20 s | 0.07 s | 13.5 MiB |
| Docker | 1.97 s | 1.71 s | 0.39 s | 0.19 s | 12.0 MiB |
| Apple Container | 2.55 s | 1.98 s | 0.93 s | 0.61 s | 12.1 MiB |

Native build timings were measured inside the shell. Warm Nix shell entry
added a median 0.14 s. The final shell took 40.92 s to provision in a store
partly warmed by the earlier pin. The Nix development environment closure
was 2.3 GiB. Docker's builder reported 2.35 GB on disk and 599 MB of image
content. Apple Container reported 599 MB of image content. These tool-reported
sizes exclude the separate Cargo dependency caches and container runtimes.

Runtime installations were already present. Docker was already running and
Apple Container service startup took 0.04 s. Docker's updated image took
104.17 s to build, including a transient Rust distribution DNS failure that
recovered on retry. Apple Container's direct image build failed after 58.47 s
when it could not reach Debian. The build succeeded in 27.63 s using the
existing restricted proxy. These are one-time provisioning observations under
different cache and network conditions.

### Physical Kindle validation

The operator verified the deployed native build on a Kindle Paperwhite 6.
Independent MTP readback matched the native artifact and confirmed that the
retained rollback matched the previously installed binary. The operator
reported the overall result. Individual acceptance checks remain unrecorded.
See the [device evidence](mxcfb-device-evidence.md#native-nix-build-2026-11-27).

The native backend became the default after this device validation. The GNU
cross-compiler and cross-libc packages are candidates for removal from the
container image if the separate GNU target is retired.

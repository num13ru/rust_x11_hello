# Containerless Kindle Build

## Objective

Investigate and implement a containerless cross-compilation workflow for PaperPad using a reproducible Nix development environment.

The primary goal is to build a statically linked ARMv7 Linux executable directly on Apple Silicon macOS without Docker, Apple Container, Linux VMs, QEMU, or Rosetta.

Linux hosts should eventually use the same native build workflow.

## Target workflow

```sh
nix develop

make check
make build BUILD_BACKEND=native
make verify BUILD_BACKEND=native
```

The resulting Kindle extension must remain compatible with the existing deployment workflow.

## Constraints

- Preserve `armv7-unknown-linux-musleabihf`.
- Preserve static musl linking and the existing Kindle ABI.
- Preserve the current executable name and extension layout.
- Preserve Docker and Apple Container as supported alternatives.
- Do not change the default build backend until native builds have been validated on physical hardware.
- Do not introduce unrelated application changes.
- Do not install global dependencies through Homebrew, rustup, or cargo install.
- Do not introduce Mise, Mr Boxington, or additional build orchestration tools during this experiment.
- Prefer removing infrastructure complexity over introducing new abstraction layers.
- Read AGENTS.md and existing project documentation before implementation.

## Phase 1 — Audit the current build pipeline

Inspect:

- `AGENTS.md`
- `Cargo.toml`
- `Cargo.lock`
- `.cargo/config.toml`
- `Dockerfile`
- `Makefile`
- `scripts/build-kindle-armv7hf.sh`
- `scripts/check-static.sh`
- `scripts/deploy-kindle-mtp.sh`
- Existing documentation under `docs/`

Identify:

- Host-specific build dependencies.
- Target-specific dependencies.
- Rust compiler version and target requirements.
- Native dependencies, build scripts and proc macros.
- Dependencies on GNU binutils.
- Dependencies on rustup.
- Container-specific assumptions.
- Hardcoded artifact and cache paths.
- Dependencies between build, verification and deployment.

Inspect the actual target dependency graph:

```sh
cargo tree --locked \
  --target armv7-unknown-linux-musleabihf \
  --package rust_x11_hello
```

Determine whether the Kindle binary requires any external C compiler, target sysroot, or native libraries beyond the Rust toolchain.

Record the findings before modifying the build infrastructure.

## Phase 2 — Establish the container baseline

Use the existing container workflow as the reference implementation.

Record:

- Rust version.
- Cargo version.
- Target triple.
- Linker configuration.
- ELF headers.
- ARM architecture attributes.
- Floating-point ABI.
- Dynamic interpreter presence.
- Dynamic dependencies.
- GLIBC symbol version requirements.
- Binary size.
- Build duration.

Preserve the baseline binary outside the canonical deployment path.

Do not overwrite or deploy the baseline artifact during subsequent experiments.

If the container build is unavailable, explicitly report the missing baseline.

## Phase 3 — Introduce a minimal Nix flake

Add:

- `flake.nix`
- `flake.lock`

Use Nixpkgs and rust-overlay to provide a reproducible Rust toolchain with the required target installed declaratively.

Initially use the same Rust version as the existing Dockerfile, if compatible with the current dependency graph.

Required target:

```text
armv7-unknown-linux-musleabihf
```

Provide the minimum necessary packages:

- Rust toolchain
- Cargo
- rustfmt
- Clippy
- GNU Make
- Bash
- LLVM inspection tools
- file
- jq
- Python 3

Add other packages only when justified by an observed requirement.

Do not install Rust documentation unless explicitly required.

Support the following Nix systems where dependencies are available:

- `aarch64-darwin`
- `x86_64-darwin`
- `aarch64-linux`
- `x86_64-linux`

Prioritize `aarch64-darwin`.

Keep host architecture separate from the Kindle compilation target.

Do not set a global Cargo build target: PaperSpoon and host-side tests must continue to build for the host platform.

Commit the generated flake.lock with the implementation.

## Phase 4 — Prove native cross-compilation

Perform the experiment before refactoring the Makefile.

Inside the Nix development environment, inspect the active tools:

```sh
command -v rustc
command -v cargo
rustc -vV
cargo -V

rustc --print sysroot

rustc --print target-libdir \
  --target armv7-unknown-linux-musleabihf
```

Verify that the installed target libraries and appropriate rust-lld executable are available.

Ensure that Cargo is not silently using a rustup-managed toolchain from the host.

First compile a minimal Rust executable for the Kindle target using the existing static linking approach.

Then attempt the real PaperPad build:

```sh
cargo build \
  --locked \
  --release \
  --target armv7-unknown-linux-musleabihf \
  --package rust_x11_hello
```

Use the existing `.cargo/config.toml` configuration as the starting point.

Isolate experimental build artifacts from existing container artifacts.

### Preferred approach

Use:

```text
rustc
  + target standard library
  + bundled musl components
  + rust-lld
```

Do not assume that an external cross-compiler is required.

### Fallback investigation

If the preferred approach fails:

1. Capture the exact compiler or linker error.
2. Determine whether target libraries, linker discovery, startup objects, compiler runtime, native dependencies, or host-specific build scripts caused the failure.
3. Investigate explicit ELF linker configuration and self-contained linking support.
4. Investigate Nix-provided cross-compilation toolchains if necessary.
5. Evaluate Zig and cargo-zigbuild only if the simpler approaches are insufficient.

Do not install several alternative toolchains simultaneously without establishing why they are needed.

Do not silently fall back to containers.

Document unsuccessful approaches and their failure reasons.

## Phase 5 — Make ELF verification portable

Refactor `scripts/check-static.sh` so that it can verify the Kindle executable on both macOS and Linux.

Prefer LLVM tools where appropriate:

- `llvm-readelf`
- `llvm-objdump`

Preserve compatibility with existing GNU binutils in the container environment.

Verification must check:

- The artifact is a valid ELF executable.
- ELF class is ELF32.
- Byte order is little-endian.
- Machine architecture is ARM.
- ARM architecture and hard-float ABI are compatible with the baseline.
- No PT_INTERP segment exists.
- No DT_NEEDED shared-library dependencies exist.
- No GLIBC version requirements are present.

Do not reject a valid static PIE solely because its ELF type is ET_DYN.

Do not treat a failed inspection command as a successful verification.

Missing inspection tools must produce an actionable error.

Ensure verification rejects:

- A missing binary.
- An empty file.
- A malformed ELF file.
- A Mach-O executable.
- An ELF executable for the wrong architecture.
- A dynamically linked ARM executable.

Add regression tests for the verification script.

## Phase 6 — Remove rustup-specific build assumptions

Refactor `scripts/build-kindle-armv7hf.sh` to work with both:

- rustup-managed toolchains in containers;
- Nix-managed toolchains on the host.

Replace rustup-specific target detection with checks against the active Rust toolchain and installed target libraries.

Keep Cargo as the single source of truth for compiling the Kindle executable.

Avoid duplicating packaging logic between native and container builds.

Ensure:

- Build failures return a non-zero exit code.
- Failed builds do not publish stale binaries as successful artifacts.
- The actual newly built binary is verified before replacing the canonical deployment artifact.
- Custom Cargo target directories are either supported or explicitly rejected with a clear error.
- Existing extension permissions and layout are preserved.

Keep the canonical executable path:

```text
kindle-extension/rust_x11_hello/bin/rust_x11_hello
```

## Phase 7 — Integrate with Makefile

Introduce an explicit build backend selector:

```makefile
BUILD_BACKEND ?= container
```

Supported combinations:

| BUILD_BACKEND | CONTAINER_CLI | Execution |
|---|---|---|
| native | unused | Host toolchain |
| container | docker | Docker |
| container | container | Apple Container |

The native backend must not:

- Check for a container image.
- Start or invoke a container runtime.
- Create container-specific caches.
- Execute Apple Container proxy helpers.

Preserve the existing behavior of:

- `make check`
- `make image`
- `make shell`
- `make clean`
- `make clean-cache`
- `make clean-target`

Define and document backend-specific behavior where necessary.

Reject unsupported backend values with an actionable error.

Do not create separate Makefiles for macOS and Linux.

Expected native workflow:

```sh
nix develop -c make build BUILD_BACKEND=native

nix develop -c make verify BUILD_BACKEND=native
```

Existing container workflow:

```sh
make build BUILD_BACKEND=container CONTAINER_CLI=docker

make verify BUILD_BACKEND=container CONTAINER_CLI=docker
```

## Phase 8 — Test the build matrix

Validate the available combinations:

| Host | Backend | Priority |
|---|---|---|
| Apple Silicon macOS | Native Nix | Required |
| Apple Silicon macOS | Docker | Regression |
| Apple Silicon macOS | Apple Container | Regression |
| x86_64 Linux | Native Nix | Secondary |
| ARM64 Linux | Native Nix | Secondary |
| Intel macOS | Native Nix | Optional |

Run:

```sh
nix flake check

nix develop -c make check

nix develop -c make build BUILD_BACKEND=native

nix develop -c make verify BUILD_BACKEND=native

git diff --check
```

Validate from a fresh checkout or isolated worktree.

Verify that native builds succeed when the Docker and Apple Container services are not running.

Do not claim support for platforms that were not actually tested.

## Phase 9 — Benchmark

Compare the available build backends:

- Docker
- Apple Container
- Native Nix

Measure separately:

- Initial environment provisioning.
- Container image creation.
- Nix shell activation.
- Clean compilation with downloaded dependencies.
- Incremental compilation after a small source change.
- Cached compilation without source changes.
- ELF verification.
- Resulting binary size.
- Disk usage.

Use comparable compiler versions, build profiles, CPU limits, and cache conditions where possible.

Repeat measurements and report the median.

Document non-equivalent benchmark conditions.

Do not introduce Mr Boxington or sccache until an uncontaminated baseline has been established.

## Phase 10 — Physical Kindle validation

Prepare the verified native artifact for manual deployment to the Kindle Paperwhite.

Do not automatically deploy experimental binaries without explicit approval.

Manual acceptance tests:

- Launch from KUAL.
- Successful initialization.
- MXCFB framebuffer access.
- evdev touch input.
- PaperSpoon connection.
- Mono1 rendering.
- Gray8 rendering.
- Existing waveform selection behavior.
- Local Exit.
- Watchdog termination.
- Framebuffer restoration.
- Clean process termination.

Preserve the existing rollback mechanism.

A successful ELF inspection does not constitute proof of physical-device compatibility.

Mark hardware validation as pending until actual results are available.

## Phase 11 — Documentation

Update README.md with:

- Nix prerequisites.
- Native build commands.
- Supported host platforms.
- Existing container alternatives.
- Artifact verification.
- Troubleshooting.
- Known limitations.

Document the experiment results in a dedicated file under `docs/`.

The documentation must distinguish:

- Verified functionality.
- Untested configurations.
- Failed approaches.
- Remaining hardware validation.
- Potential future simplifications.

## Non-goals

- Removing Dockerfile immediately.
- Replacing Make with Nix build derivations.
- Migrating PaperSpoon to a different build system.
- Introducing a new Rust dependency management strategy.
- Changing the Kindle runtime or PPFB protocol.
- Adding distributed build caches.
- Migrating the host operating system to nix-darwin.

## Definition of done

- [ ] Nix provides a pinned Rust toolchain and Kindle target.
- [ ] PaperPad builds directly on Apple Silicon macOS.
- [ ] No container runtime is required by the native backend.
- [ ] The artifact passes portable ELF verification.
- [ ] Existing container builds remain functional.
- [ ] Host-side workspace checks remain functional.
- [ ] Build and verification failures are handled correctly.
- [ ] Native and container artifacts have been compared.
- [ ] Benchmark results are documented.
- [ ] Physical Kindle validation has been completed or explicitly marked as pending.

## Final report

Provide:

1. Summary of changes.
2. Actual toolchain and linker used.
3. Required Nix packages and justification.
4. Removed container-specific assumptions.
5. Successful and unsuccessful experiments.
6. Build matrix results.
7. Performance comparison.
8. Remaining limitations.
9. Whether native compilation is ready to become the default.

Prefer the smallest working solution. If direct Rust cross-compilation succeeds, do not introduce an additional cross-compilation framework.

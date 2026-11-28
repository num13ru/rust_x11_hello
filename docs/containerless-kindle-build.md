# Kindle build guide

## Current workflow

Use the default Nix shell for the standard native workflow with plain Cargo.

```sh
nix develop -c make check build verify
```

### Optional Mr Boxington cache

Use the MBX shell to run the same checks and build with the shared compiler
cache.

```sh
nix develop .#mbx -c make check build verify
```

Both shells use the same pinned Rust toolchain, ARMv7 musl target, and LLVM
tools. The MBX shell adds the pinned Mr Boxington package and puts its Cargo
wrapper first on `PATH`. The Kindle build still uses `rust-lld` and static CRT
linking. Use `nix develop .#mbx -c mbx doctor` to check setup and find the active
cache, or `nix develop .#mbx -c mbx stats` to see cache reuse.

### Container builds

Use Docker or Apple Container when a container build is needed.

```sh
make build verify BUILD_BACKEND=container CONTAINER_CLI=docker
make build verify BUILD_BACKEND=container CONTAINER_CLI=container
```

For Apple Container networks that require the host HTTP proxy, add
`PROXY=true`. To refresh the builder image, run `make image` with the selected
`CONTAINER_CLI`. Update the Rust pins in `flake.nix` and `Dockerfile` together.
`make check-toolchain` checks both pins as part of `make check`.

Use the [README](../README.md#install-or-update-the-kindle-extension) for MTP
deployment. Prior toolchain validation and measurements are in the
[build validation history](archive/kindle-build-validation.md).

### Cache and cleanup

| Storage | Contents and ownership |
| --- | --- |
| `/nix/store` | Toolchains and Nix packages, managed by Nix |
| Cargo registry under `CARGO_HOME` (usually `~/.cargo`) | Native Cargo dependency downloads |
| MBX cache | Shared compiler outputs and MBX-managed targets. `mbx doctor` reports the active location. |
| Repository `target/` | Regular Cargo build outputs and container image stamps |
| Repository `.cache/cargo/` | Dependency downloads for the container builds, separated by CLI |

Use `mbx stats` for cache reuse, `mbx cache stats` for disk usage, and
`mbx gc --dry-run` to preview shared cache collection. `mbx clean` clears the
current checkout's MBX-managed target. `make clean-target` clears the checkout's
`target/`, which MBX can link to its managed target directory. Plain Cargo
recreates a local `target/` when you remove that link and build in `nix develop`.
`make clean` also removes `logs/` and the packaged Kindle binary.
`make clean-cache` clears `.cache/cargo/`. These Make targets leave the shared
MBX cache, native Cargo registry, and Nix store in place.

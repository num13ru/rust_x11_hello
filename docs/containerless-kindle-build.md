# Kindle build guide

## Current workflow

Use `nix develop` for the default native build, or `nix develop .#mbx` to opt
into Mr Boxington v1.31.0. Both shells supply Rust 1.97.1 with the same ARMv7
musl libraries and LLVM tools. The MBX shell adds the pinned upstream package
and puts its `libexec/mbx/cargo` wrapper first on the shell's `PATH`. The
project's `.cargo/config.toml` continues to select `rust-lld` and static CRT
linking. Run `command -v cargo rustc`, `cargo -V`, and `mbx doctor` inside the
MBX shell to check tool discovery. Use `nix develop` if you need plain Cargo.

Run the standard native workflow with the pinned toolchain.

```sh
nix develop -c make check build verify
```

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

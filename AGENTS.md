# AGENTS.md

## Build and verify

Use the pinned Nix environment for `make check build verify`. The default
build produces a verified static ARMv7 binary at
`kindle-extension/rust_x11_hello/bin/rust_x11_hello`. Use the
[build guide](docs/containerless-kindle-build.md) for other toolchains.

Keep every PaperPad KUAL launcher in the single group named **PaperPad**.
The canonical extension is `/extensions/rust_x11_hello`. Publish launchers
through its menu and run `make check-menu` after editing it.

## Architecture

PaperSpoon owns application layout, rendering, hit testing, and action dispatch.
PaperPad receives opaque Mono1 or Gray8 frames and sends viewport-relative
touch events. Keep device-local Exit available when disconnected.
PaperSpoon decodes JPEGs before sending pixels. Keep framebuffer types
independent of transport and display lifecycle. PaperPad uses native MXCFB
and evdev. The `rust_x11_hello` package and extension paths provide deployment
compatibility.

## Kindle deployment

PaperPad source, shared protocol code used on the Kindle, and changes to the
KUAL package require verified deployment when exactly one Kindle is accessible.
PaperSpoon-only changes can be checked on the host.

- Confirm USB access with `mtp-rs devices` and use read-only MTP checks to
  identify the target. Use the verified default Nix build for deployment.
- Use `scripts/deploy-kindle-mtp.sh install` for a fresh extension or
  `scripts/deploy-kindle-mtp.sh update` for an existing one. When a retained
  `bin/rust_x11_hello.previous` blocks an update, archive it and remove that
  file before retrying. A single accessible Kindle authorizes this workflow.
- Before updating, compare the host binary SHA-256 with an independent MTP
  readback. Skip the transfer when the binary and extension support files
  match. After an update, verify the flashed binary independently and retain
  the previous binary for rollback.
- Report an unavailable device. Ask for direction if the target is ambiguous
  or the operation requires materially different destructive steps.

## Evidence and documentation

Keep the README focused on running the app. Put operational details in
`docs/`, completed plans and device history in `docs/archive/`, and
historical compression results in `docs/benchmarks/`.

Before an agent-directed device run, preserve any needed evidence and clear
`rust_x11_hello.log` and `paperpad.latency.jsonl`. After stopping the run,
read back only the records needed for the claim. Store device logs and status
snapshots outside the checkout in a task-specific temporary directory.
Summarize relevant evidence and remove temporary raw logs unless the user
requests retention.

Use the deployed checksum and Kindle-side logs to support device claims.
Native-input verification requires the evdev reader to open and log Press
and Release records. Host checks verify host behavior and buildability.

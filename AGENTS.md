# AGENTS.md

## Build and verify

Use the pinned Nix environment for `make check build verify`. The default
build produces a verified static ARMv7 binary at
`kindle-extension/rust_x11_hello/bin/rust_x11_hello`. Use the
[build guide](docs/containerless-kindle-build.md) for other toolchains.

Keep every PaperPad KUAL launcher in the single group named **PaperPad**.
The canonical extension is `/extensions/rust_x11_hello`. Publish launchers
through its menu and run `make check-menu` after editing it.

## Source Code Editing

- Use the native `apply_patch` tool for manual source code and documentation changes.
- Do not generate Python, Perl, Ruby, AWK, or shell scripts to perform ordinary source code edits.
- Do not use string replacement, regular expressions, or substring slicing to implement structural code changes.
- If a patch fails, inspect the current file contents and construct a corrected patch.
- Do not silently fall back to scripted file modifications.
- Use language-aware refactoring tools when available and appropriate.
- Scripted modifications are acceptable for genuinely mechanical bulk transformations, generated files, and dedicated migration tooling. Explain why scripting is necessary before proceeding.
- Python remains permitted for testing, benchmarking, data processing, and diagnostics.
- Verify the resulting changes with `git diff --check` and inspect the complete diff.
- Run relevant formatting, compilation, linting, and test checks.
- Explicitly report any verification that could not be performed.

## Code quality

-  Run [`deslop-please`](https://github.com/deterministic-machines/deslop-please) to check for sloppy code structure.
-  Errors flagged by `deslop-please` must be fixed from first principles, don't do mechanical moves to hide them.
  - `module-cycle` & `tangle`: what is the indented layer of abstraction? Which module should be at which layer?
  - `field-leak` & `shallow-module`: Why we have these fields/functions at all? What are the patterns of use? Can they be hidden behind a narrow interface? Don't fix each leak one at a time, fix them holistically.
  - Draw a diagram to help user understand the before and proposed after structure.

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

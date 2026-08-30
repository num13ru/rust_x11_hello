# AGENTS.md

Workflow notes for the rust_x11_hello repository. The source of truth for
behavior is README.md and the KUAL scripts; this file describes the task-level
steps an agent should follow, especially around physical-device work.

## Project shape

- Rust binary (`src/`) builds for ARMv7 musl via Docker (`make build`, `make verify`).
- Host artifact: `kindle-extension/rust_x11_hello/bin/rust_x11_hello` (untracked, rebuilt by `make build`).
- Device extension root: `/extensions/rust_x11_hello` (MTP) = `/mnt/us/extensions/rust_x11_hello` (runtime).
- KUAL actions: **Run Rust X11 Hello (90s)**, **Run Rust X11 Hello (WiFi)**, **Stop Rust X11 Hello**, **Show Last Result**.
- Physical device: Kindle Paperwhite 6 (Sangria / Bellatrix4), serial `GN433X11518401E8`, FW 5.17.1.0.4.
  USBNetwork is NOT available on this device (no maintained package accepts it; see
  `docs/usbnetwork-pw2-report.md`); transport is Wi-Fi via `RUST_X11_HELLO_COMPANION`.


## Companion process (operator-owned)

- NEVER start, restart, or run the companion automatically — not via an agent,
  not in the background, not with `nohup`/`&`/`hub`, not as part of a test or
  deploy step. The companion is an interactive terminal process owned by the
  user: it must be started manually by the user and kept alive in their
  terminal so they can observe its stdout and type `display` commands.
- A companion started by an agent (even with the fixed broadcast code) blocks
  physical testing: the user must be able to see the companion output on
  their own terminal and feed it stdin. The user cannot do that if the agent
  holds the process.
- If a companion is already running, NEVER kill or restart it unless the user
  explicitly asks. Killing the user's companion is destructive.
- When the user needs a companion for a device run, state the exact command
  for them to run in their own terminal, and wait for them to confirm it is
  up:
  ```sh
  cd tools/companion && ./target/release/companion 5581 /private/tmp/companion.log ./companion.id 5580
  ```
  (The user may substitute their own log path/port. The companion persists its
  identity in `companion.id`.)
- After the user confirms the companion is listening, the agent may verify
  transport endpoints (`lsof`/network checks) but must not take ownership of
  the process.
## Device deployment (MTP)

Prerequisites:

- Kindle unlocked with USB accessory access allowed; `mtp-rs devices` lists it.
- Binary built and verified first: `make check && make build && make verify`.
- App stopped: watchdog ended the run, or **Stop Rust X11 Hello** used and window confirmed gone.
  MTP cannot prove process state; `--confirm-stopped` is an operator assertion.

Fresh install (only when `/extensions/rust_x11_hello` does not exist):

```sh
scripts/deploy-kindle-mtp.sh install
```

Update (replaces existing binary, retains a verified `rust_x11_hello.previous`):

```sh
scripts/deploy-kindle-mtp.sh update --confirm-stopped
```

Rules the deploy script enforces:

- `install` refuses if the canonical extension already exists.
- `update` refuses if the retained backup exists. Clear it explicitly before retrying:
  `mtp-rs rm /extensions/rust_x11_hello/bin/rust_x11_hello.previous --yes`
- Uploads are verified by MTP readback; `menu.json` is uploaded last so KUAL never
  exposes a partially transferred extension.

After a successful deploy, `mtp-rs ls /extensions/rust_x11_hello --recursive` shows:
`rust_x11_hello.log`, `rust_x11_hello.status`, `bin/{rust_x11_hello, run.sh, show.sh, stop.sh}`,
`config.xml`, `menu.json`, plus `bin/rust_x11_hello.previous` retained from the update.

## Log handling

- Retrieve evidence after a run:
  `mtp-rs get /extensions/rust_x11_hello/rust_x11_hello.log rust_x11_hello.device.log --replace`
- The on-device log grows unbounded across runs. If it is too large (or before a clean
  evidence run), delete it over MTP — the next run recreates it:
  `mtp-rs rm /extensions/rust_x11_hello/rust_x11_hello.log --yes`

## Verification discipline

- Kindle-side logs (`rust_x11_hello.log`, `rust_x11_hello.status`) are authoritative for
  input/transport behavior; host checks prove buildability only.
- Match the deployed binary's SHA-256 against the host artifact before trusting a run.
- A missing `ButtonPress`/`ButtonRelease` after verifying checksum, event mask, mapped
  window, and geometry means core-X11 touch is unverified on that configuration — not
  that the build failed.
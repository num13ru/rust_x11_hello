# AGENTS.md

Workflow notes for the rust_x11_hello repository. The source of truth for
behavior is README.md and the KUAL scripts; this file describes the task-level
steps an agent should follow, especially around physical-device work.

## Project shape

- Rust binary (`src/`) builds for ARMv7 musl via Docker (`make build`, `make verify`).
- Host artifact: `kindle-extension/rust_x11_hello/bin/rust_x11_hello` (untracked, rebuilt by `make build`).
- Device extension root: `/extensions/rust_x11_hello` (MTP) = `/mnt/us/extensions/rust_x11_hello` (runtime).
- KUAL actions: **Run Paperpad (5m)** uses the X11 default; **Run Paperpad
  MXCFB (5m, experimental)** selects direct framebuffer output while retaining
  a backgroundless X11 InputOutput window for input and clipping, without an
  X11 GC. Stop from
  the in-window **Exit** button or let the
  watchdog end the run; the full-screen window covers KUAL. A first MXCFB
  trial on the Paperwhite 6 exercised direct display, X11 touch, and local Exit;
  remaining visual and lifecycle checks are recorded in
  `docs/archive/mxcfb-device-evidence.md`.
- **Inspect framebuffer metadata** is a read-only KUAL diagnostic. It logs
  `/dev/fb0` metadata without using the normal launcher or drawing.
- **Inspect input devices** is a read-only KUAL diagnostic. It logs evdev names,
  capabilities, properties, and absolute-axis ranges without consuming events
  or requesting `EVIOCGRAB`.
- **Trace input events (15s; tap)** is a bounded, nonexclusive KUAL diagnostic.
  It logs raw touchscreen records for protocol analysis and does not request
  `EVIOCGRAB`.
- The four **Benchmark MXCFB** KUAL actions are bounded, local completion tests
  for GC16, DU, A2, and DU4. They bypass PaperSpoon, retain X11 for local Exit,
  and must not be described as visual FPS without camera/operator evidence.
- The **Benchmark A2 512px/256px/64px** actions use centered dirty regions to
  isolate update-area scaling. They retain full-framebuffer snapshot cleanup;
  do not compare them to full-screen throughput without stating region size.
- Physical device: Kindle Paperwhite 6 (Sangria / Bellatrix4), FW 5.17.1.0.4.
  USBNetwork is NOT available on this device; transport is Wi-Fi via `PAPERPAD_COMPANION`.

## Documentation hygiene

- Keep `README.md` concise: it is the project overview and operator entry point,
  not a destination for raw logs, per-sample benchmark dumps, or accumulating
  device-test history.
- Maintain benchmark methodology and the latest compact result set in
  `docs/mxcfb-benchmarks.md`. Put dated runs, checksums, failures, and historical
  device evidence in `docs/archive/mxcfb-device-evidence.md`.
- Keep retrieved device logs and status snapshots local and uncommitted. Summarize
  only the evidence needed to support a claim in the appropriate documentation.

## Architecture ownership

PaperSpoon owns application state, layout, rendering, hit testing, and the
mapping from application controls to semantic actions. PaperPad treats the
remote viewport as an opaque, negotiated Mono1 or Gray8 framebuffer: it may
interpret physical X11 coordinates only to map them into viewport-relative
pointer coordinates or to handle its own device-local system UI, including
Exit. Do not duplicate
application button geometry in PaperPad or send application action IDs across
the PaperPad/PaperSpoon protocol boundary. Exit and other device-local
lifecycle controls must work even when PaperSpoon is disconnected. PaperPad's
device-local system strip may also report transport state; its connection
indicator must not become an application action or touch target.

The dependency direction is PaperSpoon application layout/rendering/input and
host-side JPEG decoding → transport-independent `paper-protocol` PPFB v2
frames → PaperPad framebuffer receiver → sibling X11 or MXCFB display backend.
JPEG bytes and paths never cross the protocol boundary. Viewport-relative
pointer messages travel in the opposite direction through the X11 input window
for both display modes. Keep framebuffer primitives independent of transport
and display backend; X11-specific bitmap conversion belongs in PaperPad's X11
adapter, not in `paper-protocol` or MXCFB.

## Device deployment (MTP)

Prerequisites:

- Kindle unlocked with USB accessory access allowed; `mtp-rs devices` lists it.
- PaperPad-affecting changes built and verified first:
  `make check && make build && make verify`.

### Agent deployment authorization

After making and verifying changes, classify their deployment scope:

- PaperSpoon-only changes under `tools/paperspoon/` do not require Kindle
  deployment.
- PaperPad changes under `src/`, changes to shared protocol code used by the
  PaperPad binary, or changes under `kindle-extension/rust_x11_hello/` require
  automatic deployment when exactly one Kindle is accessible.
- Mixed PaperPad and PaperSpoon changes follow the PaperPad rule.

Do not ask whether to deploy and do not require a separate stopped-process or
backup-replacement confirmation. A single accessible Kindle is standing
authorization to resolve install versus update, remove only
`/extensions/rust_x11_hello/bin/rust_x11_hello.previous` when it blocks the
update, and run the normal verified deployment workflow.

Before transferring, use read-only MTP checks and an independent readback to
compare the built host binary's SHA-256 with the deployed binary. Skip transfer
when the hashes match and no extension support file changed. After transferring,
independently read back the deployed binary and require its SHA-256 to match the
host artifact. Keep the required build verification and use the canonical deploy
script. If no Kindle is accessible, report that deployment could not run. Ask
for direction only when multiple targets are ambiguous or a materially different
destructive action would be required.

Fresh install (only when `/extensions/rust_x11_hello` does not exist):

```sh
scripts/deploy-kindle-mtp.sh install
```

Update (replaces existing binary, retains a verified `rust_x11_hello.previous`):

```sh
scripts/deploy-kindle-mtp.sh update
```

Rules the deploy script enforces:

- `install` refuses if the canonical extension already exists.
- `update` refuses if the retained backup exists. Clear it explicitly before retrying:
  `mtp-rs rm /extensions/rust_x11_hello/bin/rust_x11_hello.previous --yes`
- Uploads are verified by MTP readback; `menu.json` is uploaded last so KUAL never
  exposes a partially transferred extension.

After a successful deploy, `mtp-rs ls /extensions/rust_x11_hello --recursive` shows:
`rust_x11_hello.log`, `rust_x11_hello.status`,
`bin/{rust_x11_hello, run.sh, probe-fb.sh, probe-input.sh}`,
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

# AGENTS.md

Workflow notes for the rust_x11_hello repository. The source of truth for
behavior is README.md and the KUAL scripts; this file describes the task-level
steps an agent should follow, especially around physical-device work.

## Project shape

- Rust binary (`src/`) builds for ARMv7 musl via Docker (`make build`, `make verify`).
- Host artifact: `kindle-extension/rust_x11_hello/bin/rust_x11_hello` (untracked, rebuilt by `make build`).
- Device extension root: `/extensions/rust_x11_hello` (MTP) = `/mnt/us/extensions/rust_x11_hello` (runtime).
- KUAL actions: **Run Paperpad (5m)** uses the X11 display backend; **Run
  Paperpad MXCFB (5m, experimental)** selects direct framebuffer output. Both
  normal runs use capability-selected native evdev touch while retaining an X11
  window for screen ownership, clipping, geometry, and lifecycle; MXCFB uses a
  backgroundless `InputOutput` window without an X11 GC. Stop from
  the in-window **Exit** button or let the
  watchdog end the run; the full-screen window covers KUAL. A first MXCFB
  historical first MXCFB trial on the Paperwhite 6 exercised direct display,
  the then-current X11 touch path, and local Exit;
  remaining visual and lifecycle checks are recorded in
  `docs/archive/mxcfb-device-evidence.md`.
- **Test MXCFB TERM cleanup (15s)** runs the normal MXCFB path with a short
  watchdog so the launcher's `TERM` forwarding, orderly framebuffer restore,
  and process exit can be checked without waiting five minutes. It is not a
  benchmark.
- **Trial MXCFB without X11 (15s)** is an opt-in, transport-free ownership
  experiment. It opens no X11 connection, uses native evdev for local Exit,
  stops itself after 15 seconds, and has a 30-second launcher watchdog. Normal
  launch behavior is unchanged. If cleanup does not visibly restore the stock
  UI, wait for the watchdog, then use sleep/wake to request a stock UI redraw;
  restart the Kindle only if that recovery fails. This recovery sequence still
  requires device evidence.
- **Trial exclusive input without X11 (15s)** is a separate opt-in input-
  ownership experiment. It requests `EVIOCGRAB`, performs no transport, and
  retains the same local Exit, internal deadline, framebuffer restoration, and
  30-second launcher watchdog. Normal and benchmark readers remain
  nonexclusive. During the trial, tap the blank remote area before using local
  Exit; after restoration, confirm that the underlying Kindle UI did not react
  and that normal Kindle touch works again. If it does not, wait for the
  watchdog before using sleep/wake and then restart only as a last resort.
- **Trial connected runtime without X11 (60s)** is the next opt-in checkpoint.
  It uses the real PaperSpoon transport, MXCFB display, and exclusive native
  input without opening X11. It stops itself after 60 seconds and has a
  90-second launcher watchdog. PaperSpoon must be reachable for connected
  rendering and remote-touch claims; otherwise the run proves only the
  disconnected path. Normal launch behavior remains unchanged.
- **Test connected no-X11 TERM cleanup (15s)** runs that same connected trial
  with a watchdog shorter than its internal deadline. The launcher must send
  `TERM`; PaperPad must release `EVIOCGRAB`, restore the framebuffer, and exit
  before escalation. It is a lifecycle test, not a benchmark.
- **Inspect framebuffer metadata** is a read-only KUAL diagnostic. It logs
  `/dev/fb0` metadata without using the normal launcher or drawing.
- **Inspect input devices** is a read-only KUAL diagnostic. It logs evdev names,
  capabilities, properties, and absolute-axis ranges without consuming events
  or requesting `EVIOCGRAB`.
- **Trace input events (15s; tap)** is a bounded, nonexclusive KUAL diagnostic.
  It logs raw touchscreen records for protocol analysis and does not request
  `EVIOCGRAB`.
- The four **Benchmark MXCFB** KUAL actions are bounded, local completion tests
  for GC16, DU, A2, and DU4. They bypass PaperSpoon, use native evdev for local
  Exit, retain X11 only for window ownership and lifecycle, and must not be
  described as visual FPS without camera/operator evidence.
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
  `docs/archive/mxcfb-benchmarks.md`. Put dated runs, checksums, failures, and historical
  device evidence in `docs/archive/mxcfb-device-evidence.md`.
- Keep retrieved device logs and status snapshots local and uncommitted. Summarize
  only the evidence needed to support a claim in the appropriate documentation.

## Architecture ownership

PaperSpoon owns application state, layout, rendering, hit testing, and the
mapping from application controls to semantic actions. PaperPad treats the
remote viewport as an opaque, negotiated Mono1 or Gray8 framebuffer: it may
interpret physical input coordinates only to map them into viewport-relative
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
pointer messages travel in the opposite direction from the native evdev reader
for both normal display modes. X11 pointer events are diagnostic-only while the
X11 window remains for screen ownership and lifecycle. Keep framebuffer
primitives independent of transport and display backend; X11-specific bitmap
conversion belongs in PaperPad's X11 adapter, not in `paper-protocol` or MXCFB.

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
- Native-input claims require `input reader: opened` plus evdev Press/Release
  records in the authoritative Kindle log and the matching deployed checksum.
  X11 `ButtonPress`/`ButtonRelease` records are expected to be logged as ignored
  during normal Linux runs and are not evidence that PaperPad dispatched twice.

# AGENTS.md

Workflow notes for the rust_x11_hello repository. The source of truth for
behavior is README.md and the KUAL scripts; this file describes the task-level
steps an agent should follow, especially around physical-device work.

## Project shape

- Rust binary (`src/`) builds for ARMv7 musl in the default Nix shell
  (`nix develop`, then `make check`, `make build`, and `make verify`). The optional
  `nix develop .#mbx` shell adds a local compilation cache. Docker and Apple
  Container remain available via `BUILD_BACKEND=container`.
- Host artifact: `kindle-extension/rust_x11_hello/bin/rust_x11_hello` (untracked, rebuilt by `make build`).
- Device extension root: `/extensions/rust_x11_hello` (MTP) = `/mnt/us/extensions/rust_x11_hello` (runtime).
- KUAL actions **Run Paperpad GC16/DU/A2/DU4 (5m)** use the connected native
  runtime with the selected remote-frame waveform, direct framebuffer output,
  and exclusive evdev input, without opening X11. Local system UI, startup
  clearing, and framebuffer restoration remain GC16 in every mode.
  Stop it from the device-local **Exit** button or let the watchdog end it. A
  first MXCFB
  historical first MXCFB trial on the Paperwhite 6 exercised direct display,
  the then-current X11 touch path, and local Exit;
  remaining visual and lifecycle checks are recorded in
  `docs/archive/mxcfb-device-evidence.md`.
- **Test MXCFB TERM cleanup (15s)** runs the normal native MXCFB path with a
  short watchdog so the launcher's `TERM` forwarding, exclusive-input release,
  orderly framebuffer restore, and process exit can be checked without waiting
  five minutes. It is not a benchmark.
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
  Exit, derive geometry from `/dev/fb0`, open no X11 connection, and must not
  be described as visual FPS without camera/operator evidence.
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
frames → PaperPad framebuffer receiver → MXCFB display backend. JPEG bytes and
paths never cross the protocol boundary. Viewport-relative pointer messages
travel in the opposite direction from the native evdev reader. Keep framebuffer
primitives independent of transport and display lifecycle. PaperPad has no X11
source modules or client dependencies. The historical `rust_x11_hello` package
and extension names remain for deployment compatibility; they are not runtime
dependencies.

## Device deployment (MTP)

Prerequisites:

- Kindle unlocked with USB accessory access allowed; `mtp-rs devices` lists it.
- PaperPad-affecting changes built and verified first from the default Nix shell:
  `make check && make build && make verify`. Use the recorded native build
  [device evidence](docs/archive/mxcfb-device-evidence.md#native-nix-build-2026-11-27)
  when checking the baseline. Experimental MBX artifacts require separate,
  manual physical validation.

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

After a successful deploy, `mtp-rs ls /extensions/rust_x11_hello --recursive` shows
`bin/{rust_x11_hello, run.sh, probe-fb.sh, probe-input.sh}`, `config.xml`, and
`menu.json`; an update also retains `bin/rust_x11_hello.previous`. Runtime files
such as `rust_x11_hello.status`, `rust_x11_hello.log`, and
`paperpad.latency.jsonl` may be absent after cleanup and are not deployment
payloads.

## Log handling

- Treat `rust_x11_hello.log` and `paperpad.latency.jsonl` as ephemeral evidence,
  not retained project artifacts. Neither file belongs in Git or the repository
  working tree.
- Before every agent-directed device run, preserve any still-needed evidence from
  the preceding run and then remove both files from the single connected Kindle
  without asking for confirmation. The launcher recreates them as needed:
  `mtp-rs rm /extensions/rust_x11_hello/rust_x11_hello.log --yes` and
  `mtp-rs rm /extensions/rust_x11_hello/paperpad.latency.jsonl --yes`. A missing
  file is already clean and is not an error.
- Retrieve a log only after the run has stopped and only when it is needed to
  support a specific claim. Store it in a task-specific directory under
  `/private/tmp`, never in the repository root.
- Inspect large logs with targeted searches or bounded tails. Do not dump an
  entire device log into tool output or copy raw per-event records into committed
  documentation.
- After extracting the required evidence and recording a compact summary, delete
  the temporary local copies and both append-style files from the Kindle. Do not
  leave them on the Kindle between agent tasks. Keep a raw log only when the user
  explicitly requests it, and keep it outside Git.
- `rust_x11_hello.status` is a small current-state snapshot, not an accumulating
  log. Preserve it unless it is stale or the user explicitly requests its removal.

## Verification discipline

- Kindle-side logs (`rust_x11_hello.log`, `rust_x11_hello.status`) are authoritative for
  input/transport behavior; host checks prove buildability only.
- Match the deployed binary's SHA-256 against the host artifact before trusting a run.
- Native-input claims require `input reader: opened` plus evdev Press/Release
  records in the authoritative Kindle log and the matching deployed checksum.
  Archived X11 `ButtonPress`/`ButtonRelease` records are not evidence that
  PaperPad dispatched twice; current normal and benchmark runs open no X11
  connection.

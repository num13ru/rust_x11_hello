# Kindle touch prototype: execution and manual-test plan (Phases 0-3)

## Scope and intended result

This plan turns the first three phases of the source plan into executable work and device-test checkpoints. It covers:

1. Phase 0: establish a reproducible, recoverable baseline;
2. Phase 1: replace the five-second lifetime with a persistent X11 event loop;
3. Phase 2: subscribe to core X11 pointer events;
4. Phase 3: log touchscreen/pointer events on a real Kindle.

It deliberately stops before hit testing, a button grid, XInput, evdev, networking, or a UI/module refactor. Phase 2 and Phase 3 should be separate commits but one device deployment, because an event subscription with no useful logging cannot be meaningfully validated by the person touching the device.

The physical Kindle is authoritative for touch support. Desktop checks can prove that the program builds and that an X11 event loop behaves sensibly, but cannot prove that Kindle touch input is translated into core X11 events.

## Verified repository facts and open assumptions

Verified on 2026-08-29:

- `cargo fmt --check`, `cargo check`, `cargo clippy --all-targets`, and `cargo test` pass on the macOS host;
- there are currently no Rust tests;
- the host has only the `aarch64-apple-darwin` Rust target installed, so no ARM/static build was verified during preparation of this plan;
- `Cargo.toml` uses `anyhow = "1"` and `x11rb = "0.13"`;
- the program still draws once, waits five seconds, and exits;
- the repository contains `kindle-extension/rust_x11_hello`, whose runner expects `/mnt/us/extensions/rust_x11_hello/bin/rust_x11_hello`;
- the ARM build and verification scripts expect the same `kindle-extension/rust_x11_hello/bin/rust_x11_hello` artifact;
- `mtp-rs ls /` succeeds against the connected Kindle;
- the MTP root `/` maps to Kindle shared storage `/mnt/us` at runtime;
- the installed KUAL extension is `/extensions/rust_hello` over MTP, with `bin/rust_hello`, `bin/run.sh`, `bin/show.sh`, `menu.json`, `config.xml`, and `hello.log`;
- the installed launcher runs `/mnt/us/extensions/rust_hello/bin/rust_hello` and writes `/mnt/us/extensions/rust_hello/hello.log`.

The repository/device mismatch is now verified rather than hypothetical. Until Phase 0 deliberately migrates and proves another layout, the existing `rust_hello` installation is the canonical device layout. The Cargo output may remain `rust_x11_hello`; deployment must explicitly rename/map it to `rust_hello` instead of assuming host and device names match.

## Conditions required for device testing

Before installing the persistent build, all of the following must be true:

- the Kindle is jailbroken and KUAL can launch the current extension;
- `mtp-rs devices`, `mtp-rs ls /extensions`, and a small upload/download readback have succeeded with the Kindle unlocked and USB accessory access allowed;
- the chosen MTP path, runtime path, and binary name are recorded explicitly;
- a bounded launcher watchdog has been tested independently of the X11 window; MTP alone cannot signal a running process or follow a growing log safely;
- the last known-good binary is retained on the host;
- the Kindle model, firmware version, orientation, and screen dimensions are recorded with the test result.

SSH through USBNetwork or Wi-Fi is optional and useful for live `tail`/manual stop, but it is not the deployment prerequisite. With MTP-only access, never launch an unbounded persistent test build: use a 90-second launcher watchdog until another stop mechanism has been proved while the X11 window is active. A KUAL “Stop” menu item alone is not sufficient because the override-redirect window may cover KUAL.

## Phase 0 - Reproducible and recoverable baseline

### Repository work

1. Inspect the actual Kindle over MTP:

```sh
mtp-rs devices
mtp-rs ls /extensions
mtp-rs ls /extensions/rust_hello --recursive
mtp-rs ls /extensions/rust_hello/bin
```

2. Record both sides of the verified deployment mapping. For the currently connected device, use:

```text
host build artifact: kindle-extension/rust_x11_hello/bin/rust_x11_hello
MTP extension: /extensions/rust_hello
MTP binary: /extensions/rust_hello/bin/rust_hello
runtime extension: /mnt/us/extensions/rust_hello
runtime binary: /mnt/us/extensions/rust_hello/bin/rust_hello
runtime log: /mnt/us/extensions/rust_hello/hello.log
runtime PID file: /mnt/us/extensions/rust_hello/rust_hello.pid
```

3. Align `scripts/build-kindle-armv7hf.sh`, `scripts/check-static.sh`, `Makefile`, `menu.json`, and the launcher with that explicit mapping. Keep the Cargo package/binary name `rust_x11_hello`; deploy-time rename to `rust_hello` is acceptable. Do not delete or rename the known-working device extension merely to make the names uniform.

4. Make the launcher safe for a persistent child process:

- reject a second launch when the PID file identifies a live expected instance;
- start the binary as a child, record its PID, and wait for it;
- remove a stale PID file only after confirming it does not identify the expected live binary;
- clean up the PID file when the child exits;
- retain the child exit status and error text in `hello.log`;
- for test builds, start a separate 90-second watchdog that sends `TERM` only to the recorded child and is cancelled/reaped if the child exits first;
- optionally provide a stop script that validates the PID belongs to this extension before sending `TERM`; do not treat it as MTP-callable unless an MTP stop-sentinel workflow has been implemented and proved.

5. Update “Show Last Result” or add a diagnostic action that displays useful final status. Do not attempt to render the live event stream with `eips`. Retrieve completed logs with `mtp-rs get`; if SSH happens to be available, `tail -F` is an optional convenience only.

6. Keep the current five-second program behavior for this phase. The purpose is to prove the build, MTP package transfer, KUAL launch, logging, and cleanup all refer to the same artifact before changing its lifetime.

### Host verification gate

Run in this order and stop at the first failure:

```sh
cargo fmt --check
cargo check
cargo clippy --all-targets
cargo test
make build
make verify
```

Record the produced binary's path and checksum. `make verify` must report no dynamic interpreter and no GLIBC symbols. If Docker or its package network is unavailable, report the ARM/static build as unverified; do not substitute the successful macOS build as evidence.

### Manual Kindle baseline guide

1. Back up the known-working device files to the host before replacing anything:

```sh
KINDLE_BACKUP_DIR=artifacts/kindle-backups/phase0
mkdir -p "$KINDLE_BACKUP_DIR"
mtp-rs get /extensions/rust_hello/bin/rust_hello "$KINDLE_BACKUP_DIR/rust_hello.before-phase0"
mtp-rs get /extensions/rust_hello/bin/run.sh "$KINDLE_BACKUP_DIR/run.sh.before-phase0"
mtp-rs get /extensions/rust_hello/bin/show.sh "$KINDLE_BACKUP_DIR/show.sh.before-phase0"
mtp-rs get /extensions/rust_hello/menu.json "$KINDLE_BACKUP_DIR/menu.json.before-phase0"
mtp-rs get /extensions/rust_hello/config.xml "$KINDLE_BACKUP_DIR/config.xml.before-phase0"
shasum -a 256 "$KINDLE_BACKUP_DIR/rust_hello.before-phase0"
```

If a backup destination already exists, choose a new run-specific directory; do not overwrite the only known-good copy.

2. Confirm the five-second baseline process has exited and KUAL is usable. Upload the new binary under a temporary remote name and request byte-for-byte readback verification:

```sh
mtp-rs put kindle-extension/rust_x11_hello/bin/rust_x11_hello \
  /extensions/rust_hello/bin/rust_hello.new --verify
mtp-rs ls /extensions/rust_hello/bin
```

3. Only after `--verify` succeeds, replace the stopped binary with recoverable in-place renames:

```sh
mtp-rs rename /extensions/rust_hello/bin/rust_hello rust_hello.before-phase0
mtp-rs rename /extensions/rust_hello/bin/rust_hello.new rust_hello
mtp-rs ls /extensions/rust_hello/bin
```

The backup name must not already exist. If it does, stop and choose a unique recorded name. MTP does not preserve Unix executable bits reliably; the KUAL launcher must run `chmod +x` before execution.

4. Upload any Phase 0 launcher/menu changes to their verified MTP paths:

```sh
mtp-rs put kindle-extension/rust_x11_hello/bin/run.sh \
  /extensions/rust_hello/bin/run.sh --replace --verify
mtp-rs put kindle-extension/rust_x11_hello/bin/show.sh \
  /extensions/rust_hello/bin/show.sh --replace --verify
mtp-rs put kindle-extension/rust_x11_hello/menu.json \
  /extensions/rust_hello/menu.json --replace --verify
mtp-rs put kindle-extension/rust_x11_hello/config.xml \
  /extensions/rust_hello/config.xml --replace --verify
```

5. Wait for every MTP command to finish. If USB/MTP mode prevents interaction with KUAL on this firmware, disconnect the cable before launching and reconnect only after the process exits.

6. On the Kindle, open KUAL, open **Rust Hello**, and select **Run Rust Hello**.

7. Confirm all of the following:

- the expected X11 window appears;
- it closes after five seconds;
- KUAL/device UI becomes usable again;
- the launcher records exit status `0`;
- no PID file remains after exit.

8. Run it a second time to prove stale state does not prevent relaunch.

9. Reconnect MTP if necessary and retrieve the completed log and deployed binary for evidence:

```sh
mtp-rs get /extensions/rust_hello/hello.log \
  "$KINDLE_BACKUP_DIR/hello.phase0.log" --replace
mtp-rs get /extensions/rust_hello/bin/rust_hello \
  "$KINDLE_BACKUP_DIR/rust_hello.deployed" --replace
shasum -a 256 \
  kindle-extension/rust_x11_hello/bin/rust_x11_hello \
  "$KINDLE_BACKUP_DIR/rust_hello.deployed"
```

The two binary checksums must match. Treat `mtp-rs get` as post-run evidence collection, not a live log follower.

### Failure and recovery paths

- **Kindle is not visible or macOS denies USB access**: unlock the Mac and Kindle, allow the accessory, close competing MTP clients, run `mtp-rs doctor`, and retry from an interactive Terminal if the process lacks macOS USB user-client access. Do not silently substitute SSH/SCP and claim the MTP workflow was tested.
- **Remote path not found**: rerun `mtp-rs ls /extensions` and use the path actually reported by the device. Remember that MTP `/extensions/...` is runtime `/mnt/us/extensions/...`.
- **Upload succeeds but rename/replacement fails**: inspect `/extensions/rust_hello/bin`. Do not launch a partial install. If the old binary was renamed, restore it with `mtp-rs rename`; retain the verified host backup.
- **“binary file does not exist”**: the build/deployment mapping is still inconsistent; do not begin Phase 1.
- **No window, X11 connection error**: retrieve `hello.log`, record `DISPLAY`, `XAUTHORITY`, and `/tmp/.X11-unix` if an optional shell is available, and compare the KUAL environment with the previously working launcher. Do not guess credentials or switch GUI libraries.
- **Window appears but KUAL never returns after five seconds**: wait for the process to exit before retrieving the log, then fix child wait/status/PID cleanup. Prove the watchdog separately before continuing.
- **Log changes while being downloaded**: discard that copy, wait for clean process exit, and download again. MTP retrieval is not a replacement for `tail -F`.

### Phase 0 exit criteria

- The host-build-to-MTP-to-runtime path mapping is explicit and used everywhere.
- macOS checks pass.
- ARM/static build and static verification pass, or are explicitly marked unverified with the missing prerequisite named.
- The MTP upload completes with `--verify`, and a downloaded deployed binary has the same checksum as the host artifact.
- The same packaged binary launches twice through KUAL, exits normally, and leaves no live process or PID file.
- Completed-log retrieval with `mtp-rs get` is tested.
- The 90-second watchdog is independently proved before an unbounded Rust event loop is deployed without another stop path.

Suggested commit: `chore: align Kindle build and KUAL launcher paths`

## Phase 1 - Persistent X11 event loop

### Implementation steps

1. Remove the five-second sleep and the now-unused `thread`/`Duration` imports. Also remove or update the on-screen “Auto-exit” text.

2. After the window is created, mapped, and the map request is flushed, enter a blocking loop around `RustConnection::wait_for_event()`; do not add async code or polling timers.

3. Match at least these events:

   - `Expose`: call the existing draw routine when the final expose in a batch is reached (`count == 0`), avoiding redundant e-ink drawing;
   - `ConfigureNotify`: update/log valid geometry and safely reject or ignore zero/invalid dimensions;
   - `ButtonPress` and `ButtonRelease`: recognize the variants but defer the full diagnostic line to Phase 3;
   - other events: ignore them safely or emit a concise event-name diagnostic without panicking.

4. Treat `wait_for_event()` failure as termination of the event loop. Preserve a contextual connection error as the primary result and attempt best-effort GC/window cleanup only when the connection still permits it.

5. Check server-side errors for important X11 setup requests where `x11rb` returns a checkable void cookie. A successfully queued request is not proof that the X server accepted it.

6. Flush only after mapping or drawing/state-changing requests. Do not flush after merely receiving an event.

7. Keep the existing `RustConnection`, fixed window geometry, `override_redirect`, dependencies, and one-file structure. Geometry and module refactors belong to later phases.

### Host verification

1. Run the four macOS checks from Phase 0.
2. Run the ARM/static build and verification.
3. If a desktop X server is available, launch the program there and verify it remains alive for at least 30 seconds, redraws after an expose/uncover action, and exits when explicitly terminated. Record this only as desktop event-loop evidence.

### Manual Kindle persistence guide

1. Confirm the Phase 0 MTP backup exists and the 90-second watchdog was proved. Without a proved watchdog or another independent stop path, do not deploy the persistent build.
2. Confirm no previous instance is running. Deploy the verified binary through the temporary-name/readback/rename procedure from Phase 0. Never replace the binary while the previous process may still be live.
3. Wait for MTP commands to finish. Disconnect USB if this firmware blocks KUAL while MTP is connected.
4. Launch **Run Rust Hello** from KUAL.
5. Observe the window for at least 30 seconds. It must remain visible and the process must remain live; the old five-second auto-exit must not occur. Finish observations before the 90-second watchdog deadline.
6. If practical, sleep/wake or briefly cover/uncover the X11 window through a known-safe device action. Confirm an `Expose` redraw only if the X server actually emits one. Absence of an `Expose` during that gesture is inconclusive.
7. Touching the window is not an input test yet. No touch output is expected before Phases 2-3.
8. For the MTP-only workflow, allow the 90-second watchdog to stop the child. A proved MTP stop sentinel or optional SSH stop may end the run earlier, but neither is required for this plan.
9. Confirm the window disappears and KUAL/device UI becomes usable. Reconnect MTP if necessary and retrieve evidence:

```sh
KINDLE_EVIDENCE_DIR=artifacts/kindle-runs/phase1
mkdir -p "$KINDLE_EVIDENCE_DIR"
mtp-rs get /extensions/rust_hello/hello.log \
  "$KINDLE_EVIDENCE_DIR/hello.phase1.log" --replace
mtp-rs ls /extensions/rust_hello
```

10. Confirm the launcher recorded why the child ended and that `rust_hello.pid` is absent. Launch and stop once more to prove restartability.

### Failure and recovery paths

- **Immediate exit**: after the run ends, retrieve `hello.log` with MTP and preserve the first connection/event-loop error; do not hide it behind cleanup errors.
- **Watchdog deadline passes but the window/process remains**: do not relaunch or overwrite the binary. Use the previously proved recovery method and mark the persistence gate failed. MTP file transfer by itself cannot kill the process.
- **Frozen-looking e-ink image but the process is gone**: an X11 resource should disappear when the connection closes, but pixels can remain visually stale. Return Home or sleep/wake to request a normal system redraw before considering reboot.
- **Optional `TERM` stop does not work**: verify the PID and executable path, retry `TERM`, then use `KILL` only on the verified PID as a last recovery step. Preserve the log.
- **MTP is unavailable after reconnect**: run `mtp-rs doctor`, resolve USB/accessory ownership, and retrieve evidence later. Do not infer cleanup or input behavior from a missing log download.
- **Device UI is inaccessible and the watchdog also failed**: use the previously tested device recovery method. Do not proceed to pointer testing.

### Phase 1 exit criteria

- The window remains open beyond 30 seconds.
- The event loop blocks rather than spins: no growing idle log and no obvious idle CPU problem.
- `Expose` is handled without a timed repaint loop.
- Connection failure is reported without panic.
- The proved watchdog or another verified stop path ends the process; the app can then be relaunched without stale state.
- The completed Phase 1 log is retrieved through MTP and tied to the tested binary checksum.
- The static ARM build still passes.

Suggested commit: `feat: add persistent X11 event loop`

## Phase 2 - Subscribe to core pointer events

### Implementation steps

1. Extend the window event mask with:

   ```text
   BUTTON_PRESS | BUTTON_RELEASE | POINTER_MOTION
   ```

   Preserve `EXPOSURE | STRUCTURE_NOTIFY`.

2. Keep motion-event handling deliberately quiet. It may be ignored or counted for diagnostics, but it must not print one line per motion event during normal use.

3. Ensure all pointer variants are handled without indexing arrays or assuming coordinates are inside the window. X11 can report a release outside the window after an implicit button grab.

4. Do not enable the `xinput` feature and do not read `/dev/input/event*` in this phase.

### Verification and deployment rule

Run the macOS and ARM/static gates and review the final mask. A standalone Kindle deployment is optional and should only check that touching or dragging does not crash the application. Do not claim that events arrived, because Phase 2 intentionally provides no adequate evidence. Commit Phase 2 separately, then implement Phase 3 and deploy the two together.

### Phase 2 exit criteria

- The window selects exposure, structure, press, release, and motion events.
- Normal motion does not flood the log.
- The host and static-build gates pass.
- No XInput/evdev or new dependency has been introduced.

Suggested commit: `feat: subscribe to X11 pointer events`

## Phase 3 - Structured touch diagnostics

### Implementation steps

1. Emit exactly one concise line for each `ButtonPress` and `ButtonRelease` containing:

   - event type;
   - `detail` (the core X11 button number; do not assume it is `1`);
   - window-relative `event_x` and `event_y`;
   - `root_x` and `root_y`;
   - X11 timestamp;
   - event window ID, preferably in hex;
   - `same_screen` and modifier/button state if useful for diagnosing surprises.

2. Use stable field names so logs from different device runs can be compared. Example shape, not a required exact format:

   ```text
   input type=ButtonPress detail=1 event_x=412 event_y=183 root_x=492 root_y=303 time=123456 window=0x2600001
   input type=ButtonRelease detail=1 event_x=413 event_y=183 root_x=493 root_y=303 time=123598 window=0x2600001
   ```

3. Keep coordinates signed. Log unexpected/out-of-window values rather than clamping or rejecting them.

4. Do not turn pointer motion into normal per-event output. If motion must be inspected after a failure, add a narrowly scoped diagnostic switch or a throttled summary, then record that it was enabled for that run.

5. Do not interpret a tap as a logical button and do not change rendering in response to input.

### Manual Kindle touch guide

#### Prepare the run

1. Record:

```text
Kindle model:
Firmware:
Orientation:
Reported X11 width x height:
Host binary checksum/commit:
MTP extension path: /extensions/rust_hello
Runtime extension path: /mnt/us/extensions/rust_hello
Date/time:
```

2. Confirm the device paths and preserve the previous completed log if one exists:

```sh
KINDLE_EVIDENCE_DIR=artifacts/kindle-runs/phase3
mkdir -p "$KINDLE_EVIDENCE_DIR"
mtp-rs ls /extensions/rust_hello
mtp-rs ls /extensions/rust_hello/bin
mtp-rs get /extensions/rust_hello/hello.log \
  "$KINDLE_EVIDENCE_DIR/hello.before-phase3.log"
```

If there is no prior log, record that fact instead of treating it as a deployment failure. Do not download or replace a log while the app is writing it.

3. Deploy the Phase 3 binary with the Phase 0 temporary-name/readback/rename procedure. Confirm its readback checksum, and confirm the previous instance is not running before replacement.
4. Wait for MTP commands to finish. Disconnect USB if necessary for KUAL interaction.
5. Launch it through KUAL. Use the visible mapped window as the start signal; without optional SSH, do not claim the log was observed live.
6. Complete the input sequence within the 90-second watchdog window. Keep test touches away from bezel/system gesture areas unless a test explicitly calls for them.

#### Perform the input matrix

Perform each action slowly with a short, consistent pause. Record the action order and approximate host time; after the clean stop and MTP log retrieval, annotate the corresponding lines with the test ID.

| ID | Manual action | Evidence to look for |
|---|---|---|
| T1 | One quick tap near the center | One press followed by one release; matching `detail`; close press/release coordinates |
| T2 | Tap near the inset top-left, top-right, bottom-left, and bottom-right of the visible app window | Coordinates vary in the expected direction and remain plausible for the reported geometry |
| T3 | Press and hold for about two seconds, then release without moving | A press appears promptly and one release appears later; no synthetic repeats are required |
| T4 | Press in the center, drag a short distance, then release | Press and release coordinates differ; no crash or log flood |
| T5 | Five distinct, moderately paced taps in the center | Five ordered press/release pairs, or a precisely recorded discrepancy |
| T6 | Tap outside the application window, if safely reachable with the current fixed geometry | No app event is the normal expectation; record any event that does arrive |
| T7 | Optional two-finger touch | Record actual behavior only; core X11 multi-touch support is not an acceptance requirement |

Repeat T1 and T2 once after stopping and relaunching the app.

#### Validate the evidence

1. Let the 90-second watchdog or another proved stop path end the run cleanly. Confirm the window disappears and KUAL/device UI returns before accessing the log.
2. Reconnect MTP if necessary and retrieve the complete post-run log plus a readback of the deployed binary:

```sh
mtp-rs get /extensions/rust_hello/hello.log \
  "$KINDLE_EVIDENCE_DIR/hello.phase3.log" --replace
mtp-rs get /extensions/rust_hello/bin/rust_hello \
  "$KINDLE_EVIDENCE_DIR/rust_hello.phase3.deployed" --replace
mtp-rs ls /extensions/rust_hello
shasum -a 256 \
  kindle-extension/rust_x11_hello/bin/rust_x11_hello \
  "$KINDLE_EVIDENCE_DIR/rust_hello.phase3.deployed"
```

3. Require matching binary checksums, a launcher session header for this run, and no remaining `rust_hello.pid`. If the log contains earlier appended sessions, preserve the whole file and identify the tested session by its launcher timestamp/header.
4. For each normal tap, check:

- press precedes release;
- `detail` is stable for primary touch, whatever its value actually is;
- `event_x/event_y` change consistently with finger position;
- `root_x/root_y` are consistent with the window's position and screen orientation;
- timestamps are nondecreasing;
- the event window ID matches the created application window;
- no panic, connection failure, or unexpected process exit occurs.

5. Relaunch and repeat T1/T2 once. After the second clean stop, retrieve the log again as `hello.phase3.relaunch.log` and tie both sessions to the same binary checksum.
6. Preserve the complete logs, checksum, commit, and device metadata before any next build. Do not keep only a screenshot or the last `eips` line.

### Interpreting failures without jumping to a fallback

- **No press or release events**: first prove the Phase 3 binary is the one running, the window was created, and `Expose`/other X11 events still arrive. Recheck the selected event mask and repeat one desktop pointer test if available.
- **Motion but no button events**: record that exact result; it is evidence for later XInput investigation, not permission to rewrite the renderer.
- **Only press or only release**: record action-by-action ordering and whether leaving the window changes the result.
- **Coordinates fixed, inverted, rotated, negative, or outside bounds**: retain raw event and root coordinates with orientation and geometry. Do not normalize them in this phase.
- **System UI reacts instead of the app**: confirm the touch was inside the app window and note whether Kindle chrome/window-manager behavior intercepted it.
- **Event flood**: disable normal motion logging while keeping the motion subscription; repeat the test with press/release lines visible.
- **Connection/process failure during touch**: preserve the first error and exact action. Treat this as a Phase 1/3 defect before evaluating the input backend.

Only after the mask, running binary, logs, and device conditions have been verified should the next investigation ask whether XInput is available. evdev remains a later fallback, not part of Phases 0-3.

### Phase 3 exit criteria

Pass the core-X11 touch proof only when a physical Kindle run shows:

- at least one reproducible `ButtonPress`/`ButtonRelease` pair per normal tap;
- position-dependent coordinates that are plausible for the window/screen geometry;
- stable behavior across a stop/relaunch cycle;
- complete device metadata and logs tied to the tested binary;
- successful ARM/static verification and no new GUI/runtime dependency.

If those events do not arrive, the phase result is still useful but must be written as **core X11 touch input unverified/unsupported on the tested configuration**, with the observed events and failed checks attached. Do not claim Kindle touch support from compilation or desktop testing.

Suggested commit: `feat: log X11 touch coordinates`

## Evidence record for the milestone

Use this at the end of each physical-device run:

```text
Commit / binary checksum:
Host checks: PASS / FAIL (details)
ARM static build: PASS / FAIL / NOT RUN (reason)
Kindle model and firmware:
Orientation and X11 geometry:
Launch through KUAL: PASS / FAIL
Persistent >30 seconds: PASS / FAIL
MTP upload/readback verification: PASS / FAIL
Watchdog/verified stop and relaunch: PASS / FAIL
Completed log retrieved with MTP: PASS / FAIL
ButtonPress observed: YES / NO
ButtonRelease observed: YES / NO
Coordinates position-dependent: YES / NO
Unexpected events/behavior:
Log file copied to:
Conclusion: VERIFIED ON PHYSICAL KINDLE / NOT VERIFIED
```

## Boundary after this plan

The next implementation phase should make rendering fully geometry-aware and event-driven, followed only then by simple logical hit testing. XInput investigation is conditional: it starts only if the Phase 3 evidence shows that core `ButtonPress`/`ButtonRelease` events do not arrive on the real device.

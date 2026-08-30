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

- `cargo fmt --check`, `cargo check`, `cargo clippy --all-targets -- -D warnings`, and `cargo test` pass on the macOS host;
- there are two Rust unit tests: required event-mask coverage and raw pointer-diagnostic formatting, including a non-primary button and outside-window coordinates;
- `Cargo.toml` uses `anyhow = "1"` and `x11rb = "0.13"` with no XInput or evdev dependency;
- the program now uses a blocking X11 event loop, redraws final `Expose` batches, tracks valid `ConfigureNotify` geometry, subscribes core press/release/motion, and logs structured press/release diagnostics without logging normal motion;
- the repository package, KUAL scripts, Cargo binary, and runtime paths consistently use `rust_x11_hello`;
- `make build` and `make verify` pass in Docker;
- the produced artifact is a statically linked 32-bit ARM EABI5 executable with SHA-256 `873550a9518940c308e392d85811c7d5ae54f1124c9c905bbd4df3760c858d5d`;
- the ARM artifact executes in the emulated ARM Linux build environment and reaches the expected contextual error when `DISPLAY` is absent;
- a Linux `/proc` smoke test passes watchdog `TERM`-to-`KILL` escalation, duplicate-launch refusal, validated manual stop, and PID-file cleanup;
- direct MTP inspection previously found the legacy `/extensions/rust_hello` installation; the user chose to delete it and use a fresh canonical installation instead.

The canonical mapping is now:

```text
host package: kindle-extension/rust_x11_hello
host binary: kindle-extension/rust_x11_hello/bin/rust_x11_hello
MTP extension: /extensions/rust_x11_hello
MTP binary: /extensions/rust_x11_hello/bin/rust_x11_hello
runtime extension: /mnt/us/extensions/rust_x11_hello
runtime binary: /mnt/us/extensions/rust_x11_hello/bin/rust_x11_hello
runtime log: /mnt/us/extensions/rust_x11_hello/rust_x11_hello.log
runtime PID file: /mnt/us/extensions/rust_x11_hello/rust_x11_hello.pid
```

No physical run of the new artifact has been performed yet. Compilation, static verification, and the earlier MTP inventory do not prove that this binary launches or receives touch events on the Kindle.

## Conditions required for device testing

Before installing or launching the build, all of the following must be true:

- the Kindle is jailbroken and KUAL is working;
- the legacy `/extensions/rust_hello` has been removed, and fresh-install mode confirms `/extensions/rust_x11_hello` does not already exist;
- `mtp-rs devices` and `mtp-rs ls /extensions` succeed with the Kindle unlocked and USB accessory access allowed;
- the ARMv7/EABI5 binary is compatible with the Kindle model;
- the KUAL environment supplies a working `DISPLAY` and `XAUTHORITY`;
- the launcher can inspect `/proc/<pid>/exe` or `/proc/<pid>/cmdline` and signal the verified child; otherwise PID validation fails closed and the watchdog/stop action must not be treated as proved;
- the Kindle model, firmware version, orientation, screen dimensions, commit, and binary checksum are recorded with the test result.

MTP is the primary install and post-run log-retrieval transport, but it cannot prove process state, signal a process, or safely follow a growing log. The initial launcher is therefore bounded to 90 seconds, then sends `TERM` and, after a five-second grace period, `KILL` only if the same PID still resolves to the expected executable. SSH is optional for live diagnostics and is not a deployment prerequisite. A KUAL stop item is useful when reachable but is not the sole recovery path because the override-redirect window may cover KUAL.

## Phase 0 - Reproducible and recoverable baseline

### Implemented repository work

1. One canonical name, `rust_x11_hello`, is used by the Cargo binary, host package, MTP destination, KUAL scripts, log, status file, and PID file.
2. The KUAL runner:

- refuses a second launch when the PID resolves to the expected executable;
- serializes launch attempts with an atomic lock directory, refuses live/ownerless locks, and recovers a lock only when its recorded launcher PID is provably dead;
- refuses an ambiguous live PID rather than deleting its PID file or launching a duplicate;
- removes a PID file only when the recorded process is no longer live;
- records child PID, status, end reason, and timestamps;
- forwards launcher signals only to a revalidated expected child;
- uses a 90-second watchdog, a five-second `TERM` grace, and revalidation before `KILL`;
- removes its PID/watchdog marker only when they still belong to its child.

3. `stop.sh` sends `TERM` only after validating the PID against the expected binary. `show.sh` displays the stable one-line status file rather than the separator at the end of the full log.
4. `scripts/deploy-kindle-mtp.sh install` refuses an existing canonical extension, verifies each transfer, and uploads `menu.json` last. Update mode requires `--confirm-stopped`, stages the binary, retains `rust_x11_hello.previous`, and attempts rollback if activation fails.

### Host verification gate

Run in order and stop at the first failure:

```sh
make check
make build
make verify
sh -n kindle-extension/rust_x11_hello/bin/run.sh
sh -n kindle-extension/rust_x11_hello/bin/show.sh
sh -n kindle-extension/rust_x11_hello/bin/stop.sh
jq empty kindle-extension/rust_x11_hello/menu.json
file kindle-extension/rust_x11_hello/bin/rust_x11_hello
shasum -a 256 kindle-extension/rust_x11_hello/bin/rust_x11_hello
```

The `file` result must identify a 32-bit ARM EABI5 statically linked executable. `make verify` must report no dynamic interpreter and no GLIBC symbols. A macOS build is not substitute evidence for these checks.

### Fresh MTP installation and baseline guide

1. After deleting the legacy extension, inspect shared storage:

```sh
mtp-rs ls /extensions
```

Both conditions must hold before continuing: `rust_hello` is absent, and `rust_x11_hello` is absent. If the canonical directory already exists, inspect it and choose deliberate recovery/update steps instead of running fresh-install mode.

2. Install the fully verified package:

```sh
scripts/deploy-kindle-mtp.sh install
```

3. Record the host checksum printed by the installer. Wait for all MTP operations to complete. If USB/MTP mode blocks KUAL interaction on this firmware, disconnect the cable.
4. In KUAL, confirm **Rust X11 Hello** contains **Run Rust X11 Hello (90s)**, **Stop Rust X11 Hello**, and **Show Last Result**.
5. Select **Show Last Result** before the first run. “no status yet” is expected and proves the menu/script path without starting the persistent program.
6. Select **Run Rust X11 Hello (90s)**. Confirm the expected X11 window appears. Do not overwrite package files while it is running.
7. Allow the watchdog to stop the first run. Confirm the window disappears and KUAL/device UI becomes usable.
8. Reconnect MTP if necessary and retrieve evidence:

```sh
KINDLE_EVIDENCE_DIR=artifacts/kindle-runs/phase0
mkdir -p "$KINDLE_EVIDENCE_DIR"
mtp-rs get /extensions/rust_x11_hello/rust_x11_hello.log \
  "$KINDLE_EVIDENCE_DIR/rust_x11_hello.phase0.log"
mtp-rs get /extensions/rust_x11_hello/rust_x11_hello.status \
  "$KINDLE_EVIDENCE_DIR/rust_x11_hello.phase0.status"
mtp-rs get /extensions/rust_x11_hello/bin/rust_x11_hello \
  "$KINDLE_EVIDENCE_DIR/rust_x11_hello.phase0.deployed"
mtp-rs ls /extensions/rust_x11_hello
shasum -a 256 \
  kindle-extension/rust_x11_hello/bin/rust_x11_hello \
  "$KINDLE_EVIDENCE_DIR/rust_x11_hello.phase0.deployed"
```

The binary checksums must match; `rust_x11_hello.pid`, `rust_x11_hello.watchdog`, and `rust_x11_hello.lock` must be absent; and the log must contain one complete launcher session with an end reason.

### Failure and recovery paths

- **macOS denies USB access**: unlock the Mac and Kindle, allow the accessory, close competing MTP clients, run `mtp-rs doctor`, and retry from an interactive Terminal. Do not silently substitute SCP and claim the MTP workflow passed.
- **Fresh install finds an existing canonical directory**: stop and inspect it. The installer intentionally refuses to merge with unknown or partial state.
- **Install fails before `menu.json`**: KUAL should not expose the partial package. Preserve command output, inspect the remote directory, and repair or remove it deliberately before retrying.
- **No window or X11 connection error**: retrieve the completed log and compare the KUAL `DISPLAY`/`XAUTHORITY` environment with the previously working launcher if an optional shell is available.
- **Watchdog cannot validate the PID**: it fails closed and does not signal. Use a previously tested recovery method and do not claim the bounded-run safety gate passed.
- **Log changes while being downloaded**: discard the copy, wait for clean process exit, and download again. MTP retrieval is not a live follower.

### Phase 0 exit criteria

- The canonical package installs through MTP with readback verification and no legacy duplicate.
- The downloaded deployed binary checksum matches the verified host artifact.
- KUAL finds all three actions at the canonical path.
- One bounded launcher session completes, KUAL returns, status/log evidence is retrieved, and no PID/watchdog/launcher-lock state remains.
- The watchdog's PID-validation and termination behavior is either proved on device or explicitly marked unverified; compilation alone is insufficient.

Suggested commit: `chore: add safe KUAL and MTP deployment baseline`

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

1. Confirm Phase 0 recorded the installed checksum and proved the bounded launcher path. If the watchdog could not validate/signal the child, do not repeat a persistent run until another independent recovery path is available.
2. No redeployment is needed when testing the same installed checksum. If the binary changed, first confirm the prior window is gone and no PID file remains, then use:

```sh
scripts/deploy-kindle-mtp.sh update --confirm-stopped
```

Never replace package files while the previous process may still be live.

3. Wait for MTP commands to finish. Disconnect USB if this firmware blocks KUAL while MTP is connected.
4. Launch **Run Rust X11 Hello (90s)** from KUAL.
5. Observe the window for at least 30 seconds. It must remain visible; the removed five-second auto-exit must not occur. Finish observations before the watchdog deadline.
6. If practical, sleep/wake or briefly cover/uncover the X11 window through a known-safe device action. Confirm an `Expose` redraw only if the X server emits one; absence of an `Expose` during that gesture is inconclusive.
7. Touching the window is not interpreted as logical UI input yet. For this phase, only stability matters.
8. If KUAL remains reachable, test **Stop Rust X11 Hello** once. Otherwise allow the 90-second watchdog to stop the child.
9. Confirm the window disappears and KUAL/device UI becomes usable. Reconnect MTP and retrieve the completed log/status:

```sh
KINDLE_EVIDENCE_DIR=artifacts/kindle-runs/phase1
mkdir -p "$KINDLE_EVIDENCE_DIR"
mtp-rs get /extensions/rust_x11_hello/rust_x11_hello.log \
  "$KINDLE_EVIDENCE_DIR/rust_x11_hello.phase1.log" --replace
mtp-rs get /extensions/rust_x11_hello/rust_x11_hello.status \
  "$KINDLE_EVIDENCE_DIR/rust_x11_hello.phase1.status" --replace
mtp-rs ls /extensions/rust_x11_hello
```

10. Confirm the launcher recorded the end reason and `rust_x11_hello.pid` is absent. Launch and stop once more to prove restartability.

### Failure and recovery paths

- **Immediate exit**: after the run ends, retrieve `rust_x11_hello.log` with MTP and preserve the first connection/event-loop error; do not hide it behind cleanup errors.
- **Watchdog deadline passes but the window/process remains**: do not relaunch or overwrite the binary. Use the previously proved recovery method and mark the persistence gate failed. MTP file transfer by itself cannot kill the process.
- **Frozen-looking e-ink image but the process is gone**: an X11 resource should disappear when the connection closes, but pixels can remain visually stale. Return Home or sleep/wake to request a normal system redraw before considering reboot.
- **Optional `TERM` stop does not work**: verify the PID and executable path, retry `TERM`, then use `KILL` only on the verified PID as a last recovery step. Preserve the log.
- **MTP is unavailable after reconnect**: run `mtp-rs doctor`, resolve USB/accessory ownership, and retrieve evidence later. Do not infer cleanup or input behavior from a missing log download.
- **MTP update fails before activation**: the old binary remains selected, but support files are not transactional and may already be new. Do not launch; inspect the remote listing and rerun or repair the package first.
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
MTP extension path: /extensions/rust_x11_hello
Runtime extension path: /mnt/us/extensions/rust_x11_hello
Date/time:
```

2. Confirm the device paths and preserve the previous completed log if one exists:

```sh
KINDLE_EVIDENCE_DIR=artifacts/kindle-runs/phase3
mkdir -p "$KINDLE_EVIDENCE_DIR"
mtp-rs ls /extensions/rust_x11_hello
mtp-rs ls /extensions/rust_x11_hello/bin
mtp-rs get /extensions/rust_x11_hello/rust_x11_hello.log \
  "$KINDLE_EVIDENCE_DIR/rust_x11_hello.before-phase3.log"
```

If there is no prior log, record that fact instead of treating it as a deployment failure. Do not download or replace a log while the app is writing it.

3. If Phase 3 uses the same checksum installed in Phase 0, do not redeploy it. Otherwise confirm the previous process is stopped and run `scripts/deploy-kindle-mtp.sh update --confirm-stopped`; record the new readback checksum.
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
mtp-rs get /extensions/rust_x11_hello/rust_x11_hello.log \
  "$KINDLE_EVIDENCE_DIR/rust_x11_hello.phase3.log" --replace
mtp-rs get /extensions/rust_x11_hello/bin/rust_x11_hello \
  "$KINDLE_EVIDENCE_DIR/rust_x11_hello.phase3.deployed" --replace
mtp-rs ls /extensions/rust_x11_hello
shasum -a 256 \
  kindle-extension/rust_x11_hello/bin/rust_x11_hello \
  "$KINDLE_EVIDENCE_DIR/rust_x11_hello.phase3.deployed"
```

3. Require matching binary checksums, a launcher session header for this run, and no remaining `rust_x11_hello.pid`. If the log contains earlier appended sessions, preserve the whole file and identify the tested session by its launcher timestamp/header.
4. For each normal tap, check:

- press precedes release;
- `detail` is stable for primary touch, whatever its value actually is;
- `event_x/event_y` change consistently with finger position;
- `root_x/root_y` are consistent with the window's position and screen orientation;
- timestamps are nondecreasing;
- the event window ID matches the created application window;
- no panic, connection failure, or unexpected process exit occurs.

5. Relaunch and repeat T1/T2 once. After the second clean stop, retrieve the log again as `rust_x11_hello.phase3.relaunch.log` and tie both sessions to the same binary checksum.
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

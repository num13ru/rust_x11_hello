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
- the ARM build and verification scripts expect the same `kindle-extension/rust_x11_hello/bin/rust_x11_hello` artifact.

The attached source plan says the existing KUAL path works, but that claim has not been verified on a device here. The directory/binary mismatch must be resolved before touch work, otherwise a successful build may not be the binary KUAL launches.

Naming decision: use `rust_x11_hello` consistently for the KUAL extension directory, Cargo output, and deployed binary. The canonical installed path is `/mnt/us/extensions/rust_x11_hello/bin/rust_x11_hello`. If the Kindle still has an installation under a noncanonical path, remove or rename it before testing; do not support both paths during the experiment.

## Conditions required for device testing

Before installing the persistent build, all of the following must be true:

- the Kindle is jailbroken and KUAL can launch the current extension;
- the chosen extension path and binary name are known from the device, not assumed from the repository;
- the tester has a working recovery/control path independent of the X11 window, preferably SSH through USBNetwork or Wi-Fi;
- a second terminal can remain connected to stop the process and follow the log;
- the last known-good binary is retained on the host;
- the Kindle model, firmware version, orientation, and screen dimensions are recorded with the test result.

If SSH or another tested out-of-band stop path is unavailable, do not launch the persistent build. First add and prove a bounded watchdog or another recovery mechanism. A KUAL “Stop” menu item alone is not sufficient because the override-redirect window may cover KUAL.

## Phase 0 - Reproducible and recoverable baseline

### Repository work

1. Inspect the actual Kindle over SSH:

   ```sh
   ls -la /mnt/us/extensions/rust_x11_hello
   find /mnt/us/extensions/rust_x11_hello -maxdepth 2 -type f 2>/dev/null
   ```

2. Choose exactly one canonical extension directory, installed binary name, log path, and PID-file path. Unless the device contradicts the repository, use:

   ```text
   extension: /mnt/us/extensions/rust_x11_hello
   binary:    /mnt/us/extensions/rust_x11_hello/bin/rust_x11_hello
   log:       /mnt/us/extensions/rust_x11_hello/rust_x11_hello.log
   PID file:  /mnt/us/extensions/rust_x11_hello/rust_x11_hello.pid
   ```

3. Align `scripts/build-kindle-armv7hf.sh`, `scripts/check-static.sh`, `Makefile`, the extension directory, `menu.json`, and the launcher with that decision. Keep the Cargo package/binary name `rust_x11_hello`; a deploy-time rename is acceptable.

4. Make the launcher safe for a persistent child process:

   - reject a second launch when the PID file identifies a live instance;
   - start the binary as a child, record its PID, and wait for it;
   - remove a stale PID file only after confirming that it does not identify the expected live binary;
   - clean up the PID file when the child exits;
   - retain the child exit status and error text in `rust_x11_hello.log`;
   - provide a small stop script that validates the PID belongs to this extension before sending `TERM`.

5. Update “Show Last Result” or add a diagnostic action that displays a useful final status. Do not attempt to render a live event stream with `eips`; use SSH and `tail` for live input diagnostics.

6. Keep the current five-second program behavior for this phase. The purpose is to prove that build, package, KUAL launch, logging, and cleanup all refer to the same artifact before changing its lifetime.

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

1. Keep the known-good device binary on the host. Install the new binary as a temporary name first:

   ```sh
   scp kindle-extension/rust_x11_hello/bin/rust_x11_hello \
     root@<kindle-ip>:/mnt/us/extensions/rust_x11_hello/bin/rust_x11_hello.new
   ssh root@<kindle-ip> \
     'chmod +x /mnt/us/extensions/rust_x11_hello/bin/rust_x11_hello.new && mv /mnt/us/extensions/rust_x11_hello/bin/rust_x11_hello.new /mnt/us/extensions/rust_x11_hello/bin/rust_x11_hello'
   ```

   Copy updated launcher/menu files as well when Phase 0 changes them. USB mass-storage copying is acceptable, but cleanly eject the Kindle before opening KUAL. A persistent build still must not be launched without a separate stop path.

2. In terminal A, confirm SSH remains usable and follow the log:

   ```sh
   ssh root@<kindle-ip> 'tail -n 50 -F /mnt/us/extensions/rust_x11_hello/rust_x11_hello.log'
   ```

3. On the Kindle, open KUAL, open **Rust X11 Hello**, and select **Run Rust X11 Hello**.

4. Confirm all of the following:

   - the expected X11 window appears;
   - it closes after approximately five seconds;
   - KUAL/device UI becomes usable again;
   - the log names the expected binary and contains the X11 connection/screen data;
   - the launcher records exit status `0`;
   - the PID file is gone after exit.

5. Run it a second time to prove stale state does not prevent relaunch.

### Failure and recovery paths

- **“binary file does not exist”**: the build/deployment naming is still inconsistent; do not begin Phase 1.
- **No window, X11 connection error**: record `DISPLAY`, `XAUTHORITY`, and `/tmp/.X11-unix`; compare the KUAL environment with the previously working launcher. Do not guess credentials or switch GUI libraries.
- **Window appears but KUAL never returns after five seconds**: inspect the child PID and launcher log; fix wait/status/PID cleanup before continuing.
- **SSH drops when the window opens**: do not install the persistent version until a separate recovery method is proven.

### Phase 0 exit criteria

- One canonical extension path is used everywhere.
- The macOS checks pass.
- The ARM/static build and static verification pass, or are explicitly marked unverified with the missing prerequisite named.
- The same packaged binary launches twice through KUAL, exits normally, and leaves no live process or PID file.
- Remote log following and remote process termination have both been tested.

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

1. Confirm terminal A is already following `rust_x11_hello.log` and open terminal B with a working SSH prompt.
2. Deploy the verified binary using the temporary-name/atomic-move procedure from Phase 0. Never replace a binary while its previous process is live.
3. Launch **Run Rust X11 Hello** from KUAL.
4. Observe the window for at least 30 seconds. It must remain visible and the process must remain live; the old five-second auto-exit must not occur.
5. If practical, sleep/wake or briefly cover/uncover the X11 window through a known-safe device action. Confirm an `Expose` redraw when the X server actually emits one. Absence of an `Expose` during this gesture is inconclusive; do not force repeated refreshes solely for this test.
6. Touching the window is not an input test yet. No touch output is expected before Phases 2-3.
7. From terminal B, invoke the validated stop script (or send `TERM` only to the PID recorded by this extension).
8. Confirm that:

   - the process exits;
   - the X11 window disappears;
   - KUAL/device UI becomes usable;
   - the launcher records the child result;
   - the PID file is removed.

9. Launch and stop once more to prove restartability.

### Failure and recovery paths

- **Immediate exit**: read the first connection/event-loop error in the log; do not hide it with a cleanup error.
- **Frozen-looking e-ink image but process is gone**: the X11 resource should disappear when the connection closes, but pixels can remain visually stale. Return to Home or sleep/wake to request a normal system redraw before considering a reboot.
- **`TERM` does not stop it**: verify the PID and executable path, retry `TERM`, then use `KILL` only for that verified PID as a last recovery step. Preserve the log.
- **Device UI inaccessible and SSH unavailable**: use the previously tested recovery method. Mark the persistence gate failed; do not proceed to pointer testing.

### Phase 1 exit criteria

- The window remains open beyond 30 seconds.
- The event loop blocks rather than spins (no growing idle log and no obvious idle CPU problem).
- `Expose` is handled without a timed repaint loop.
- Connection failure is reported without a panic.
- The process can be remotely stopped and relaunched without stale state.
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
   Binary checksum/commit:
   Extension path:
   Date/time:
   ```

2. Start `tail -F` in terminal A and keep the stop-capable SSH session in terminal B.
3. Deploy the Phase 3 binary atomically and launch it through KUAL.
4. Wait until the log confirms that the window is mapped. Keep test touches away from the bezel/system gesture area unless a test explicitly calls for it.

#### Perform the input matrix

Perform each action slowly, pause to identify its log lines, and annotate the captured log with the test ID afterward.

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

For each normal tap, check:

- press precedes release;
- `detail` is stable for primary touch, whatever its value actually is;
- `event_x/event_y` change consistently with finger position;
- `root_x/root_y` are consistent with the window's position and screen orientation;
- timestamps are nondecreasing;
- the event window ID matches the created application window;
- no panic, connection failure, or unexpected process exit occurs.

Copy the complete log back to the host before the next build. Do not keep only a screenshot or the last `eips` line.

#### End the run

1. Stop only the PID validated by the extension stop mechanism.
2. Confirm process exit, window removal, PID-file cleanup, and device UI recovery.
3. Preserve the log with the commit/checksum and device metadata.

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
Remote stop and relaunch: PASS / FAIL
ButtonPress observed: YES / NO
ButtonRelease observed: YES / NO
Coordinates position-dependent: YES / NO
Unexpected events/behavior:
Log file copied to:
Conclusion: VERIFIED ON PHYSICAL KINDLE / NOT VERIFIED
```

## Boundary after this plan

The next implementation phase should make rendering fully geometry-aware and event-driven, followed only then by simple logical hit testing. XInput investigation is conditional: it starts only if the Phase 3 evidence shows that core `ButtonPress`/`ButtonRelease` events do not arrive on the real device.

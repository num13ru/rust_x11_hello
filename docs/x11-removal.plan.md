# Codex Implementation Plan: Remove X11 from PaperPad

Repository: https://github.com/num13ru/rust_x11_hello

## Objective

Migrate PaperPad from its hybrid X11/MXCFB architecture to a minimal native Linux implementation.

**Target architecture:**

- Display: MXCFB (`/dev/fb0`, HWTCON)
- Touch input: Linux evdev (`/dev/input/event*`)
- Runtime: X11-independent event loop
- Transport: existing PPFB v2
- Local UI: existing Exit button and connection indicator

**Final result:** one display implementation, one input implementation, no X11 client dependencies, and a smaller codebase.

This is a replacement, not an effort to maintain multiple backends.

## Assumptions and constraints

- Target device: Kindle Paperwhite 6, as documented in `AGENTS.md`.
- The existing MXCFB implementation is the starting point.
- PaperSpoon and PPFB v2 behavior must remain unchanged.
- Preserve Mono1, Gray8, remote touch forwarding, local Exit, connection state, and framebuffer restoration.
- Preserve existing KUAL watchdog and deployment safety mechanisms.
- Do not modify HWTCON waveforms or introduce unrelated FPS optimizations.
- Do not create additional architectural abstractions unless they are justified by actual reuse.
- Do not introduce Tokio or another asynchronous runtime without a demonstrated need.
- Do not permanently retain the X11 backend as a fallback.
- Physical-device behavior cannot be established by compilation or unit tests alone.

Read `AGENTS.md`, `README.md`, and the affected source files before making changes.

## Phase 1 — Inventory dependencies and verify feasibility

**Tasks**

1. Identify every X11 dependency across Rust modules, configuration, scripts, tests, and documentation.
2. Identify which X11 responsibilities must be replaced:
   - Touch events
   - Screen dimensions
   - Fullscreen window and screen ownership
   - Window lifecycle
   - Event polling
   - Benchmark-local Exit
3. Identify the existing MXCFB APIs that can be reused without modification.
4. Inspect the device's Linux input devices and determine the touchscreen event protocol.
5. Investigate whether the Kindle UI can update `/dev/fb0` while PaperPad is running without its fullscreen X11 window.
6. Determine a safe, reversible method to prevent conflicting screen updates.

**Deliverables**

- Dependency/removal inventory
- Verified or explicitly unverified touchscreen assumptions
- Screen-ownership strategy
- Short risk report

**Acceptance criteria**

The proposed design accounts for every current X11 responsibility. No unverified device-specific assumptions are presented as facts.

**Stop condition:** If exclusive screen ownership cannot be achieved safely, document the blocker before permanently removing X11.

## Phase 2 — Extract the application event loop

**Primary file:** `src/x11/events.rs`

**Tasks**

1. Move transport polling, frame presentation, connection-indicator updates, and application event dispatch into `src/runtime.rs` or a small `src/runtime/` module.
2. Keep X11 event decoding outside the new runtime during this intermediate stage.
3. Preserve existing `AppState` behavior and viewport-relative coordinates.
4. Preserve the latest-frame handling and current failure semantics.
5. Replace window-destruction-based termination with an explicit application shutdown outcome.
6. Preserve meaningful latency measurements without coupling them to X11 names.
7. Avoid unnecessary intermediate traits or generic backend interfaces.

**Acceptance criteria**

- Shared application behavior no longer depends on X11 event types.
- Existing execution remains functional during this intermediate stage.
- Existing tests pass.
- No transport protocol changes.

**Commit:** `refactor: separate PaperPad runtime from X11 events`

## Phase 3 — Implement native evdev touch input

**Suggested location:** `src/input/evdev.rs`

**Tasks**

1. Detect the correct touchscreen event device rather than assuming a fixed `/dev/input/eventN` path.
2. Inspect supported event capabilities and absolute-axis ranges.
3. Handle the event protocol actually exposed by the PW6:
   - Touch press and release
   - Absolute X/Y coordinates
   - `SYN_REPORT` event grouping
   - Multitouch slot/tracking events, if applicable
4. Normalize raw coordinates into existing `PhysicalPoint` values.
5. Handle rotation, axis inversion, and coordinate scaling as required.
6. Feed normalized events into the existing `AppState::handle_pointer()` logic.
7. Ignore unsupported or secondary contacts safely.
8. Handle incomplete event sequences, `SYN_DROPPED`, read errors, and device disappearance.
9. Investigate `EVIOCGRAB` for exclusive input access. Enable it only after verifying its interaction with Kindle.
10. Ensure device handles and exclusive grabs are released during shutdown.

**Testing**

- Coordinate boundaries
- Press/release pairing
- Exit-button activation
- Remote viewport versus local system strip
- Input cancellation
- Malformed/incomplete event sequences
- Device errors

Use synthetic evdev event sequences for automated tests.

**Acceptance criteria**

- Touch events are normalized without X11.
- Remote pointer behavior remains unchanged.
- Local Exit works without PaperSpoon.
- Device access failures produce actionable errors.
- No permanently stuck touch contact.

**Commit:** `feat: implement native evdev touch input`

## Phase 4 — Replace X11 screen ownership and lifecycle

**Tasks**

1. Obtain display dimensions from validated MXCFB framebuffer metadata.
2. Preserve the existing 72-pixel device-local system strip.
3. Implement the verified screen-ownership strategy from Phase 1.
4. Ensure the Kindle interface cannot overwrite PaperPad's active content.
5. Avoid terminating or permanently modifying the Kindle's X server.
6. Restore any temporarily changed system state on normal shutdown.
7. Handle startup failures and unexpected termination as safely as possible.
8. Preserve existing framebuffer snapshot and GC16 restoration.
9. Define an explicit recovery procedure for forced termination, including `SIGKILL` and watchdog escalation.
10. Reject unsupported framebuffer geometry instead of making assumptions.

**Acceptance criteria**

On the physical Kindle:

- PaperPad displays directly through MXCFB.
- Stock Kindle UI does not interfere during operation.
- Touch input reaches the correct recipient.
- Local Exit restores the previous screen.
- The device remains usable after errors or timeout.

**Commit:** `feat: manage display lifecycle without X11`

**Required checkpoint:** Device validation before removing the old implementation.

## Phase 5 — Switch the main runtime to MXCFB only

**Primary files:** `src/main.rs`, `src/config.rs`

**Tasks**

1. Replace X11 initialization with direct MXCFB initialization.
2. Initialize evdev input.
3. Start PaperSpoon networking using MXCFB-derived viewport geometry.
4. Run the X11-independent runtime.
5. Shut down input, networking, and display in a safe order.
6. Preserve the original error when cleanup also fails.
7. Add graceful handling of `SIGINT`, `SIGTERM`, and `SIGHUP` to normal PaperPad execution.
8. Remove backend selection from normal startup.
9. Ensure startup never attempts an X11 connection.

**Acceptance criteria**

- Normal operation requires no `DISPLAY` or `XAUTHORITY`.
- MXCFB is the only supported display path.
- Mono1 and Gray8 remain functional.
- Disconnect/reconnect behavior is preserved.
- Invalid remote frames do not replace the last successfully presented frame.
- Exit and timeout restore device state.

**Commit:** `refactor: use MXCFB as the sole PaperPad display`

## Phase 6 — Migrate benchmarks and delete X11

**Tasks**

1. Replace benchmark X11 input polling with the native input implementation.
2. Preserve local Exit and watchdog cancellation.
3. Preserve existing benchmark waveforms, bounded frame counts, update-marker semantics, and restoration.
4. Delete `src/x11/`.
5. Remove `x11rb` from `Cargo.toml` and update `Cargo.lock`.
6. Remove `DisplayBackendKind` and `PAPERPAD_DISPLAY_BACKEND`.
7. Remove X11-specific window creation, graphics contexts, diagnostics, event types, and tests.
8. Simplify or remove the `DisplayBackend` trait if only one implementation remains.
9. Keep reusable frame validation and caching structures; do not delete them merely because they live under `src/display/`.
10. Remove X11-only redraw and geometry-change behavior where no longer applicable.
11. Remove obsolete constants and comments tied to X11 button semantics or coordinates.
12. Update KUAL `menu.json` and `bin/run.sh` to expose one normal launch path.
13. Preserve read-only framebuffer diagnostics and experimental MXCFB benchmarks.

**Acceptance criteria**

- No `x11rb` dependency in the PaperPad crate.
- No X11 source modules in the final architecture.
- No backend selector or legacy X11 launch action.
- All normal and benchmark entry points work without X11.
- No new compatibility layer preserving the old architecture.

**Commit:** `refactor: remove X11 backend and obsolete code`

## Phase 7 — Verification, documentation, and cleanup

**Automated verification**

Run the repository's standard checks:

```sh
make check
make build
make verify
git diff --check
```

Update relevant verification rules to reflect the native-only architecture.

**Physical-device verification**

Test on the PW6:

- Startup from KUAL
- Startup with `DISPLAY` and `XAUTHORITY` unset
- Touch press/release accuracy
- Remote viewport edge coordinates
- Local Exit with PaperSpoon connected and disconnected
- Connection-indicator behavior
- Mono1 and Gray8 presentation
- Disconnect and reconnect
- Repeated launch and exit
- Framebuffer restoration
- Watchdog timeout and graceful termination
- Failure during initialization
- Benchmark cancellation and cleanup
- Stock Kindle UI usability after exit

Record which checks were actually executed on hardware.

**Documentation**

Update:

- `README.md`
- `AGENTS.md`
- KUAL usage instructions
- `docs/mxcfb-benchmarks.md` where relevant

Preserve useful historical device evidence in the archive rather than rewriting history.

**Complexity report**

Provide before/after measurements of:

- Rust source lines added and removed
- Number of modules
- Direct and transitive dependencies
- Final ARM binary size
- Remaining platform-specific code

Do not claim that the migration improved FPS unless separate measurements establish that.

**Commit:** `docs: finalize native PaperPad migration`

---

## Implementation rules

💡 **Prefer deletion over compatibility.** Do not keep an obsolete implementation merely to preserve old APIs.

💡 **Avoid speculative abstractions.** Do not introduce generic `DisplayBackend`, `InputBackend`, or platform frameworks for a single implementation.

💡 **Preserve protocol boundaries.** Do not move PaperSpoon application logic or hit testing onto Kindle.

💡 **Treat hardware behavior as unverified until measured.** Compiler success cannot establish correct touch calibration, screen ownership, or E-Ink restoration.

💡 **Keep this migration separate from performance optimization.** Do not mix A2 waveform tuning, framebuffer caching changes, or dirty-region optimizations into the same work.

## Execution instructions for Codex

Implement phases sequentially.

For each phase:

1. Inspect the actual relevant source and repository guidance.
2. Make the smallest coherent change.
3. Add or update tests for changed behavior.
4. Run applicable verification commands.
5. Summarize changed files and removed code.
6. Report unresolved risks and physical-device requirements.
7. Create the phase commit only after verification succeeds.

Do not silently skip failures or claim physical-device verification that was not performed.

Do not proceed past a hardware-dependent safety checkpoint on assumptions alone. If hardware access is unavailable, report the exact manual test procedure and remaining blocker.

## Definition of Done

The migration is complete when:

- PaperPad uses MXCFB exclusively for display.
- PaperPad uses native Linux input without X11.
- No X11 runtime connection is needed.
- No X11 source modules or direct dependencies remain.
- Normal operation and MXCFB benchmarks remain functional.
- Exit, cleanup, and watchdog behavior are verified on PW6.
- PaperSpoon and PPFB v2 remain compatible.
- The resulting implementation is demonstrably simpler, with obsolete code removed rather than replaced by equivalent complexity.

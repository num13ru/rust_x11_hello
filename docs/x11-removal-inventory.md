# X11 removal inventory and feasibility checkpoint

Status: Phase 1 evidence review, 2026-11-13.

This document records what the repository and existing Paperwhite 6 evidence
establish before the X11 removal work changes runtime behavior. It is not a
claim that native input or X11-free screen ownership works on the device.

## Current dependency and responsibility inventory

| Area | Current X11 responsibility | Required native replacement |
| --- | --- | --- |
| Build | `Cargo.toml` and `Cargo.lock` include `x11rb` and `x11rb-protocol`. | Remove both after the final X11 caller is gone. |
| Startup and geometry | `src/main.rs` always connects to X11, reads the selected screen dimensions, maps a full-screen window, and checks MXCFB dimensions against that window. | Open `/dev/fb0` first and use validated `FramebufferInfo`/`ScreenLayout` geometry. Reject unsupported metadata before networking or drawing. |
| Backend selection | `src/config.rs` selects the default X11 backend or experimental MXCFB backend with `PAPERPAD_DISPLAY_BACKEND`. | Remove the selector after MXCFB-only startup is device-validated. |
| Window ownership | `src/x11/display.rs` creates, maps, and destroys the full-screen window. In MXCFB mode it uses a backgroundless `InputOutput` window with no graphics context. | A verified, reversible native lifecycle must stop stock UI updates from racing PaperPad and restore normal Kindle operation on exit. This is unresolved. |
| Input | `src/x11/display.rs` selects core pointer events. `src/x11/events.rs` converts primary `ButtonPress`/`ButtonRelease` coordinates to `PointerEvent`. | Discover the touchscreen evdev node by capabilities; validate axes and orientation; handle reports, cancellation, errors, and device loss; release any optional exclusive grab. |
| Event loop | `src/x11/events.rs` combines X11 polling with transport polling, frame presentation, connection-indicator updates, display maintenance, application dispatch, and termination. | Move transport/display/application work to an X11-independent runtime before replacing the event source. |
| Redraw and geometry events | `Expose` redraws retained content and local UI. `ConfigureNotify` updates `AppState`, announces a viewport change, and invalidates incompatible retained frames. | Fixed MXCFB geometry removes ordinary window resize handling. Startup and resume/reassertion must still restore retained content and local UI without asking PaperSpoon for a new frame. |
| Termination | Local Exit destroys the X11 window; `DestroyNotify` is the successful event-loop outcome. X11 cleanup follows display cleanup. | Local Exit and signals need an explicit shutdown outcome, followed by input, networking, framebuffer restoration, and platform lifecycle cleanup in a defined order. |
| X11 display path | `src/x11/backend.rs`, `framebuffer.rs`, and `render.rs` implement Mono1 conversion, X11 upload, and X11 local UI drawing. | Delete after the native path passes the required hardware checkpoint. The transport-independent cache and frame validation remain useful. |
| MXCFB benchmark | `src/main.rs` maps an X11 window and `LocalExitPoll` supplies benchmark-local Exit and window lifecycle. | Reuse native input and explicit shutdown while preserving bounded samples, markers, waveform choices, and snapshot restoration. |
| Launcher and KUAL menu | `menu.json` exposes X11 and experimental MXCFB normal launches. The watchdog is backend-independent. | Keep the watchdog and one normal native launch; keep framebuffer diagnostics and explicit benchmarks. |
| Diagnostics and analysis | Logs and `scripts/analyze-latency.py` use X11-specific event names such as `x11_received`. | Rename newly emitted latency stages without breaking interpretation of retained historical traces. |
| Documentation | `README.md` and `AGENTS.md` describe the hybrid architecture. Archived plans and evidence preserve historical X11 results. | Update current operator documentation at cutover; do not rewrite archived evidence. |

The package, binary, extension directory, and KUAL identifier still contain
`rust_x11_hello`. Those names are not an X11 runtime dependency. Renaming them
would expand deployment and rollback risk and is not required by the removal
plan.

## Existing MXCFB work that can be reused

- `OpenedFramebuffer::open()` opens `/dev/fb0`, obtains fixed and variable
  metadata, validates the supported PW6 layout, and maps only the checked
  visible span.
- `FramebufferInfo` owns the validated `ScreenLayout`, including the 72-pixel
  device-local strip and remote viewport.
- `MxcfbDisplayBackend` already snapshots the visible framebuffer, presents
  Mono1 and Gray8 content, draws the local system UI, performs the bounded
  startup reassertion, and restores the original snapshot with a full-screen
  GC16 update during `shutdown()` or `Drop`.
- `MxcfbBenchmark` already uses the same validated framebuffer metadata and
  full-snapshot restoration while preserving marker-specific completion
  semantics.
- `AppState`, `ScreenLayout`, frame validation, and the remote-frame cache are
  independent of raw X11 protocol types. Their X11-specific comments and
  coordinate type limits still need cleanup at the final cutover.

This reuse is sufficient for framebuffer geometry, presentation, local UI, and
normal-process restoration. It does not establish touchscreen access, prevent
stock UI framebuffer writes, or guarantee cleanup after `SIGKILL` or power
loss.

## Device evidence and unresolved assumptions

The archived PW6 evidence establishes the following on Sangria/Bellatrix4,
firmware 5.17.1.0.4:

- MXCFB presentation, X11-delivered touch, local Exit, and snapshot restoration
  have each been exercised on the physical device.
- An `InputOnly` X11 window did not prevent the Kindle shelf from repainting
  over direct framebuffer content.
- Replacing it with a backgroundless full-screen `InputOutput` X11 window
  prevented that observed repaint while retaining touch and local Exit. The
  window's stacking/clipping footprint is therefore part of the only currently
  verified screen-ownership strategy.

The read-only capability probe later identified `/dev/input/event1` (`pt_mt`)
as a direct multitouch device. It exposes two slots, tracking IDs, and X/Y
ranges of 0..1271 and 0..1695, numerically matching the 1272×1696 framebuffer.
The device exposes no `BTN_TOUCH` or `BTN_TOOL_FINGER`. These results establish
device selection and coordinate extents, but not input report sequencing,
rotation/inversion, release encoding, secondary-contact behavior, or
`EVIOCGRAB` safety.

The repository still contains no bounded physical trial showing that PaperPad
can keep the display exclusively while no X11 window is mapped. Native input
protocol behavior and X11-free screen ownership therefore remain unverified.

## Screen-ownership checkpoint

There is currently no verified X11-free screen-ownership strategy. In
particular:

- an evdev exclusive grab, if supported, can redirect input but cannot by
  itself prevent stock UI drawing;
- periodic framebuffer reassertion can repair some damage but does not provide
  exclusive ownership or eliminate update races;
- stopping, pausing, or reconfiguring Kindle UI services has not been
  investigated in this repository and must not be attempted without a known
  reversible command sequence and forced-termination recovery procedure.

Consequently, Phases 2 and 3 may prepare an independent runtime and native
input, but the old X11 lifecycle must not be removed and MXCFB-only startup
must not become the normal KUAL path until a bounded PW6 trial verifies a safe,
reversible ownership method.

## Risk report

| Risk | Impact | Required evidence or mitigation |
| --- | --- | --- |
| Stock UI overwrites PaperPad without the X11 occlusion window | High: corrupt or unusable active display | Bounded native-only run with deliberate delayed stock UI activity, operator-visible result, watchdog recovery, and post-run usability check. |
| Wrong evdev device or axis transform | High: incorrect actions or inaccessible local Exit | Capability-based discovery plus captured axis ranges and synthetic normalization tests, followed by edge/corner and system-strip device tests. |
| Lost or malformed input report leaves a contact armed | High: unintended release/action later | Handle `SYN_DROPPED`, incomplete reports, read errors, disconnect, and shutdown as explicit contact cancellation. |
| Exclusive input grab survives abnormal flow | High: Kindle becomes temporarily unusable | Enable a grab only after device testing; close the descriptor on every normal/error path and document watchdog/forced-stop recovery. |
| `SIGKILL` bypasses framebuffer restoration | High: stale PaperPad image remains | Keep launcher TERM grace, verify graceful signal cleanup, and define a device-local recovery action that can restore stock UI after forced termination. |
| Runtime extraction changes latest-frame or failure semantics | Medium: stale/invalid content becomes authoritative | Preserve mailbox polling, successful-presentation cache policy, reconnect behavior, and tests before changing input. |
| Benchmark migration changes measurement meaning | Medium: invalid comparisons | Preserve bounded frames, region, waveform, marker waits, and full-snapshot cleanup; continue to avoid visual-FPS claims without camera evidence. |

## Next smallest evidence steps

The read-only `--inspect-input` diagnostic and **Inspect input devices** KUAL
action passed host checks, the ARMv7 musl build, static verification, deployed
binary readback verification, and a physical PW6 capability run. The compact
result is recorded in `docs/archive/mxcfb-device-evidence.md`.

1. Deploy the bounded, non-grabbing event-trace diagnostic and record controlled
   single-contact taps to establish report ordering, `SYN_REPORT` grouping,
   release encoding, slot behavior, and physical orientation.
2. Use the observed protocol and ranges to implement synthetic evdev decoder
   tests before opening a live input descriptor in the normal runtime.
3. Design a separately bounded no-X11 ownership trial with signal cleanup and a
   documented operator recovery path. Do not combine its first device run with
   normal-launch cutover.

The target architecture works only if the PW6 exposes a readable, identifiable
touchscreen event device with stable coordinate metadata, PaperPad can obtain
the needed input ownership without breaking stock UI recovery, and a verified
screen-lifecycle mechanism prevents competing framebuffer updates. Those
conditions have not all been established yet.

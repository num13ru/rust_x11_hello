# X11 removal inventory and feasibility checkpoint

Status: Phase 4 connected X11-free runtime, local Exit, and graceful signal
cleanup device-validated; normal-launch cutover is next, 2026-11-14.

This document records what the repository and existing Paperwhite 6 evidence
establish as the X11 removal work proceeds. It is not a claim that the complete
X11-free normal runtime works on the device.

## Current dependency and responsibility inventory

| Area | Current X11 responsibility | Required native replacement |
| --- | --- | --- |
| Build | `Cargo.toml` and `Cargo.lock` include `x11rb` and `x11rb-protocol`. | Remove both after the final X11 caller is gone. |
| Startup and geometry | MXCFB mode reads and validates `/dev/fb0` metadata before connecting to X11, derives PaperPad and evdev geometry from it, and rejects disagreement with both the X11 ownership window and the later read-write backend. Device evidence confirms the 1272 x 1696 metadata-before-X11 order, native touch, local Exit, and restoration. X11 mode still uses X11 geometry. | Remove the ownership-window comparison when the connected no-X11 runtime, exclusive input, and cleanup paths are device-validated. |
| Backend selection | `src/config.rs` selects the default X11 backend or experimental MXCFB backend with `PAPERPAD_DISPLAY_BACKEND`. | Remove the selector after MXCFB-only startup is device-validated. |
| Window ownership | Normal MXCFB still uses a backgroundless X11 `InputOutput` window with no graphics context. Device-validated no-X11 trials were not overwritten by the stock UI, restored the framebuffer snapshot, left the Kindle usable, and covered connected local-Exit and launcher-forwarded signal cleanup. | Promote the validated native lifecycle to normal MXCFB startup. Forced-recovery cases remain unverified. |
| Input | Normal Linux startup opens the single capability-selected direct touchscreen without `EVIOCGRAB`, drains bounded nonblocking Type-B records, and sends primary pointer/cancellation outcomes into the runtime. Core X11 button events are logged but ignored. Connected no-X11 device evidence confirms exclusive remote Press/Release forwarding, underlying UI isolation, and explicit grab release on local Exit. Normal and benchmark paths remain nonexclusive. | Deadline, signal, input-failure, and descriptor-close-only release still need evidence, as do viewport edges, secondary-contact cancellation, and device-loss behavior. |
| Event loop | `src/x11/events.rs` services native input and polls X11 only for expose, geometry, ownership, and lifecycle events. `src/runtime.rs` owns transport polling, frame presentation, connection-indicator updates, display maintenance, application dispatch, and explicit shutdown requests. A device-validated fixed-geometry native loop drives that runtime directly for the opt-in connected trial. | Promote the native loop to normal startup without changing service cadence or cleanup order before deleting the X11 lifecycle/event-polling boundary. |
| Redraw and geometry events | `Expose` redraws retained content and local UI. `ConfigureNotify` updates `AppState`, announces a viewport change, and invalidates incompatible retained frames. | Fixed MXCFB geometry removes ordinary window resize handling. Startup and resume/reassertion must still restore retained content and local UI without asking PaperSpoon for a new frame. |
| Termination | Local Exit returns an explicit shutdown outcome; external `DestroyNotify` remains distinct. Linux paths install atomic `HUP`/`INT`/`TERM` handlers. Device evidence confirms the connected no-X11 loop receives launcher-forwarded `TERM`, stops for signal, releases the grab, restores the framebuffer, exits status 0, and avoids `KILL`. | Preserve the validated input, transport, display cleanup order at cutover. `SIGKILL`, kernel hangs, and power loss still bypass userspace restoration. |
| X11 display path | `src/x11/backend.rs`, `framebuffer.rs`, and `render.rs` implement Mono1 conversion, X11 upload, and X11 local UI drawing. | Delete after the native path passes the required hardware checkpoint. The transport-independent cache and frame validation remain useful. |
| MXCFB benchmark | `src/main.rs` maps an X11 ownership window, uses the native evdev reader for benchmark-local Exit, and polls X11 only for fixed geometry and lifecycle. | Remove the remaining X11 lifecycle dependency while preserving bounded samples, markers, waveform choices, and snapshot restoration. |
| Launcher and KUAL menu | `menu.json` exposes X11 and experimental MXCFB normal launches. The watchdog is backend-independent. | Keep the watchdog and one normal native launch; keep framebuffer diagnostics and explicit benchmarks. |
| Diagnostics and analysis | New runtime traces use backend-neutral `input_received`; `scripts/analyze-latency.py` accepts historical `x11_received` traces as a fallback. | Preserve the compatibility fallback while native input adopts `input_received`; do not rewrite retained historical evidence. |
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
The device exposes no `BTN_TOUCH` or `BTN_TOOL_FINGER`.

A subsequent nonexclusive trace established type-B primary-contact sequencing:
tracking ID 0 starts a contact, X/Y and optional touch-major updates are grouped
by `SYN_REPORT`, and tracking ID -1 followed by `SYN_REPORT` releases it. Ordered
top-left, center, and bottom-right taps established identity portrait mapping:
X increases left-to-right and Y increases top-to-bottom. Slot 0 remained
implicit. Secondary-slot behavior and physical `SYN_DROPPED` recovery remain
unverified. The same nonexclusive touches reached the underlying Kindle UI,
and the trace did not retain its normal KUAL completion lifecycle. A later
bounded trial established `EVIOCGRAB` isolation and release on local Exit.

The bounded 15-second no-X11 trial showed that the stock UI did not overwrite
PaperPad's direct framebuffer content and that deadline cleanup restored the
screen without leaving the Kindle unusable. That trial did not establish input
ownership: prior nonexclusive tracing showed the same touches reaching the
underlying Kindle UI.

## Screen and input ownership checkpoint

Connected X11-free display, transport, remote touch, and local-Exit cleanup are
now device-verified, but the strategy is incomplete for normal launch. In
particular:

- the tested evdev exclusive grab prevented underlying UI input and released
  after local Exit, but deadline, signal, and failure cleanup remain untested;
- periodic framebuffer reassertion can repair some damage but does not provide
  exclusive ownership or eliminate update races;
- stopping, pausing, or reconfiguring Kindle UI services has not been
  investigated in this repository and must not be attempted without a known
  reversible command sequence and forced-termination recovery procedure.

The required pre-cutover checkpoint has passed. The next slice can promote the
validated connected native lifecycle to normal MXCFB startup while retaining
the old X11 path until the cutover itself is device-checked.

## Risk report

| Risk | Impact | Required evidence or mitigation |
| --- | --- | --- |
| Stock UI overwrites PaperPad without the X11 occlusion window | High: corrupt or unusable active display | Bounded native-only run with deliberate delayed stock UI activity, operator-visible result, watchdog recovery, and post-run usability check. |
| Wrong evdev device or axis transform | High: incorrect actions or inaccessible local Exit | Capability-based discovery plus captured axis ranges and synthetic normalization tests, followed by edge/corner and system-strip device tests. |
| Lost or malformed input report leaves a contact armed | High: unintended release/action later | Handle `SYN_DROPPED`, incomplete reports, read errors, disconnect, and shutdown as explicit contact cancellation. |
| Exclusive input grab survives abnormal flow | High: Kindle becomes temporarily unusable | Local-Exit and graceful-signal release passed on device. Keep descriptor-close fallback; `SIGKILL` and input-failure recovery remain unverified. |
| `SIGKILL` bypasses framebuffer restoration | High: stale PaperPad image remains | Keep launcher TERM grace, verify graceful signal cleanup, and define a device-local recovery action that can restore stock UI after forced termination. |
| Runtime extraction changes latest-frame or failure semantics | Medium: stale/invalid content becomes authoritative | Preserve mailbox polling, successful-presentation cache policy, reconnect behavior, and tests before changing input. |
| Benchmark migration changes measurement meaning | Medium: invalid comparisons | Preserve bounded frames, region, waveform, marker waits, and full-snapshot cleanup; continue to avoid visual-FPS claims without camera evidence. |

## Next smallest evidence steps

The read-only `--inspect-input` diagnostic and **Inspect input devices** KUAL
action passed host checks, the ARMv7 musl build, static verification, deployed
binary readback verification, and a physical PW6 capability run. The compact
result is recorded in `docs/archive/mxcfb-device-evidence.md`.

1. Complete the remaining native-input edge trials: viewport boundaries,
   secondary-contact cancellation, and descriptor failure. The primary touch,
   single-dispatch, and local Exit path has passed its first device trial.
2. Optionally complete the deferred interrupted-benchmark trial. All seven
   native-reader completion paths and restorations passed, but benchmark-local
   Exit, ignored X11 duplicates during a contact, and signal stop remain
   unverified on device.
3. Promote the validated connected native loop to normal MXCFB startup without
   removing the old X11 implementation in the same slice. Perform one
   consolidated device verification before deleting X11 code.

The target architecture works only if the PW6 exposes a readable, identifiable
touchscreen event device with stable coordinate metadata, PaperPad can obtain
the needed input ownership without breaking stock UI recovery, and a verified
screen-lifecycle mechanism prevents competing framebuffer updates. Those
conditions have not all been established yet.

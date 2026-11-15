# X11 removal inventory and feasibility checkpoint

Status: Phase 6 native benchmark lifecycle device-validated; dormant X11
source/dependency deletion is next, 2026-11-15.

This document records what the repository and existing Paperwhite 6 evidence
establish as the X11 removal work proceeds. It is not a claim that the complete
X11-free normal runtime works on the device.

## Current dependency and responsibility inventory

| Area | Current X11 responsibility | Required native replacement |
| --- | --- | --- |
| Build | `Cargo.toml` and `Cargo.lock` include `x11rb` and `x11rb-protocol`. | Remove both after the final X11 caller is gone. |
| Startup and geometry | Native normal startup opens and validates `/dev/fb0`, derives PaperPad and evdev geometry from it, and never connects to X11. | Preserve the validated geometry checks while migrating benchmarks. |
| Backend selection | Normal startup selects MXCFB directly; `PAPERPAD_DISPLAY_BACKEND` is no longer read. | None for the normal runtime. |
| Window ownership | Normal and benchmark startup use native MXCFB lifecycles without an X11 window. | Delete the now-dormant X11 ownership source. Forced-recovery cases remain unverified. |
| Input | Normal startup uses the validated exclusive reader and forwards viewport-relative Press/Release pairs into the shared runtime. Benchmarks use the same capability-selected reader nonexclusively for local Exit; native initialization passed the GC16 lifecycle check. | Delete dormant X11 input diagnostics. Benchmark-local Exit, input-failure, and descriptor-close-only release remain unverified. |
| Event loop | `src/runtime.rs` owns normal transport, presentation, local UI, input dispatch, and shutdown. Benchmarks poll native evdev and the shared signal flag between bounded phases. | Delete the dormant X11 event loop without changing native service cadence or cleanup order. |
| Redraw and geometry events | `Expose` redraws retained content and local UI. `ConfigureNotify` updates `AppState`, announces a viewport change, and invalidates incompatible retained frames. | Fixed MXCFB geometry removes ordinary window resize handling. Startup and resume/reassertion must still restore retained content and local UI without asking PaperSpoon for a new frame. |
| Termination | Local Exit returns an explicit shutdown outcome; external `DestroyNotify` remains distinct. Linux paths install atomic `HUP`/`INT`/`TERM` handlers. Device evidence confirms the connected no-X11 loop receives launcher-forwarded `TERM`, stops for signal, releases the grab, restores the framebuffer, exits status 0, and avoids `KILL`. | Preserve the validated input, transport, display cleanup order at cutover. `SIGKILL`, kernel hangs, and power loss still bypass userspace restoration. |
| X11 display path | The drawable X11 backend modules are detached from compilation and have no runtime caller. | Delete their source files with the remaining benchmark X11 code. The transport-independent cache and frame validation remain useful. |
| MXCFB benchmark | `src/main.rs` opens the benchmark framebuffer, derives evdev geometry from it, polls native local Exit and signals, and restores the full snapshot without X11. A 12-sample GC16 run passed on device. | Preserve bounded samples, markers, waveform choices, and snapshot restoration while deleting dormant X11 code. |
| Launcher and KUAL menu | `menu.json` exposes one normal native launch. The watchdog is backend-independent. | Keep framebuffer diagnostics and explicit benchmarks. |
| Diagnostics and analysis | New runtime traces use backend-neutral `input_received`; `scripts/analyze-latency.py` accepts historical `x11_received` traces as a fallback. | Preserve the compatibility fallback while native input adopts `input_received`; do not rewrite retained historical evidence. |
| Documentation | `README.md` and `AGENTS.md` describe native normal and benchmark lifecycles. Archived plans and evidence preserve historical X11 results. | Remove dormant-source wording at final deletion; do not rewrite archived evidence. |

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

The required cutover checkpoint passed: the normal MXCFB action exercised
connected Mono1 and Gray8 presentation, remote touch, local Exit, and watchdog
cleanup without an X11 connection. Normal startup is MXCFB-only, and a complete
GC16 run validated the framebuffer-derived, X11-free benchmark lifecycle.

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
3. Remove the dormant X11 dependency and source files. Do not add more one-off
   KUAL test actions.

The target architecture works only if the PW6 exposes a readable, identifiable
touchscreen event device with stable coordinate metadata, PaperPad can obtain
the needed input ownership without breaking stock UI recovery, and a verified
screen-lifecycle mechanism prevents competing framebuffer updates. Those
conditions have not all been established yet.

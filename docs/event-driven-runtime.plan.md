# PaperPad: Event-Driven Native Runtime Migration

> **Codex implementation plan** — `num13ru/rust_x11_hello`
> Scope: Kindle-side PaperPad native MXCFB runtime (`master` branch).
> Status: **Milestones A–G complete for the scoped event-driven cutover; unmeasured performance and fault-injection limitations remain listed below.**

## 0. Assumptions and constraints

- Target the **current repository**, not legacy X11 designs or archived documentation. Review `README.md` and the source before editing; `AGENTS.md` includes historical X11 instructions that may be stale.
- Preserve PPFB v2 wire format, Mono1/Gray8 support, viewport negotiation, and PaperSpoon's ownership of UI/layout/actions.
- Keep **evdev** input and **MXCFB** output; no X11 fallback or compatibility layer.
- Retain the existing synchronous runtime and background transport workers unless evidence shows a need to change them. **Do not introduce Tokio, async/await, or a general-purpose executor.**
- Preserve the latest-frame mailbox (bounded memory), outbound pointer ordering, reconnect behavior, local Exit, and pre-PaperPad framebuffer restoration.
- Use `libc` already in the crate when Linux system calls are needed; favor a small, well-encapsulated primitive over dependencies.
- Hardware-dependent behavior cannot be claimed verified until tested on a Paperwhite 6. Host-only tests establish logic and buildability, not visual completion or power consumption.

## 1. Verified baseline and problem statement

Inspect these concrete paths first:

| File | Current role / observed behavior |
| --- | --- |
| `src/native_runtime.rs` | Calls `runtime.service()`, drains evdev, then unconditionally sleeps for `POLL_INTERVAL = 50 ms`. |
| `src/input/evdev.rs` | Opens the selected touchscreen with `O_NONBLOCK`, optionally `EVIOCGRAB`; `drain()` processes at most 256 records per invocation. No public FD accessor yet. |
| `src/runtime.rs` | `service()` checks connection state, takes at most one latest frame via `poll_frame()`, renders, and calls display `maintain()`. |
| `src/net/mod.rs` | Reader thread publishes frames to a latest-frame mailbox **without waking the main runtime**. Existing `wake_tx` notifies the **reconnector** on reader termination; it is not a main-thread wake mechanism. Startup results are consumed via `try_recv()`. |
| `src/net/mod.rs` | `wait_for_startup_retry()` checks the stop flag with sleeps capped at `STARTUP_STOP_POLL_INTERVAL = 50 ms`; reconnect attempts use intentional retry delays. |
| `src/mxcfb/backend.rs` | `maintain()` checks `startup_refresh_at`, a one-shot ~750 ms reassertion deadline. |
| `src/main.rs` | Signal handlers set an atomic shutdown flag; the native runtime currently notices it during the periodic loop. |

**Goal:** In steady idle state, PaperPad's main thread should block until evdev input, transport/state notification, shutdown, or the nearest real display deadline. No periodic 50 ms wakeup. Do not confuse the Linux `poll(2)` readiness call with periodic application polling.

**Expected, not guaranteed:** Remove up to one 50 ms cycle of avoidable scheduling latency and reduce main-loop idle wakeups. MXCFB waveform/panel refresh latency is independent and must be measured separately.

## 2. Target architecture

```text
Touchscreen FD (evdev) -----------+
                                  |
Transport notifications ----------+--> RuntimeWait::wait(deadline)
  - validated frame published     |           |
  - startup result available      |           v
  - disconnected / reconnected    |     PaperpadRuntime
                                  |       - process input
Shutdown notification -----------+       - update connection UI
                                  |       - take latest frame
Display maintenance deadline ----+       - display/maintain
                                          |
                                          +--> MXCFB
                                          +--> outbound pointer queue
```

Use **Linux `poll(2)`** for this small, stable FD set, with a coalescing wake descriptor (`eventfd` is a reasonable first choice; a nonblocking self-pipe is an acceptable fallback). A `poll` timeout is derived from the nearest real deadline, or `-1` when no deadline exists.

Define explicit ownership:

- `RuntimeWait`: owns/borrows registered FDs; handles readiness, deadlines, and errors; no application state.
- `RuntimeNotifier`: clonable producer-side handle used by background workers; exposes a nonblocking `notify()` that does not allocate per event.
- `PaperpadRuntime`: continues to own app/contact/connection-indicator state and dispatch.
- `Paperspoon`: continues to own transport lifecycle, frame mailbox, and outbound pointer ordering.
- `MxcfbDisplayBackend`: continues to own display submissions, cached frame, restoration, and next display-maintenance deadline.

**Important:** The existing `wake_tx` in `net/mod.rs` is an internal reconnector channel. Do not repurpose it for the runtime; add a separate, clearly named notifier.

## 3. Implementation milestones

Work through the preparation and tests in order. **Safety gate:** Milestones D, E, and F form one deployable cutover: do not merge or ship a main loop that blocks indefinitely until signal wakeups and display deadlines are wired. Keep the implementation reviewable in logical commits, but test D–F together before deploying.

### Milestone A — Instrument and establish baseline

- [x] Confirm the named files and entry points still match the checked-out revision; update this plan if the repository changed.
- [x] Audit every sleep, timeout, channel receive, socket block, and display deadline relevant to **normal** PaperPad operation. Separate periodic checking from intentional reconnect/benchmark pacing.
- [x] Capture existing `input_received`, `pointer_queued`, `frame_present_start`, and `frame_present_return` traces where available; record what each measures.
- [x] Record test conditions: connected idle, disconnected idle, startup offline, touch bursts, incoming frame bursts, and graceful exit.
- [x] Define baseline measurements for main-thread wakeups/CPU and touch/frame dispatch delay. If unavailable, mark as **not measured**, not zero.

**Acceptance:** A short baseline table and an inventory of all possible main-thread wake sources exist before code changes.


#### Milestone A baseline (2026-11-15)

The checkout was `event-loop` at `a171d0494e6efb26e3d99c17c9a57aa93df5c209`.
`master` pointed to the same commit, so the plan's target and every named entry point still
matched the source being audited.

| Main-thread work source | Current observation point | Baseline behavior |
| --- | --- | --- |
| Process signal (`SIGHUP`, `SIGINT`, `SIGTERM`) | Atomic flag checked at the top of `native_runtime`'s loop | Observed after the current pass or 50 ms sleep; the atomic alone will not safely wake a future infinite wait. |
| Touchscreen input and local Exit | Nonblocking `EvdevReader::drain()` once per loop, bounded to 256 records | Readiness is not used yet; input waits for the next loop pass and a still-readable FD is revisited only after another 50 ms sleep. |
| Startup result | `Paperspoon::promote_startup()` uses `try_recv()` from `is_connected()` and `poll_frame()` | A queued result is observed on the next 50 ms service pass. |
| Connection transition | Shared `ConnectionState` read by `is_connected()` | Reader EOF wakes the reconnector, not the main thread; the UI observes disconnect/reconnect on a later service pass. |
| Validated inbound frame | Latest-only `FrameMailbox`, taken by `poll_frame()` | Publication does not wake the main thread; the newest frame is taken on the next service pass. |
| Startup display reassertion | `MxcfbDisplayBackend::maintain()` checks `startup_refresh_at` | One-shot deadline is about 750 ms after open and runs on the first 50 ms service pass at or after the deadline. |

Normal-runtime waits and blocks were classified as follows:

| Owner | Wait or deadline | Classification and event-driven constraint |
| --- | --- | --- |
| Native main thread | Unconditional `POLL_INTERVAL` sleep (50 ms) | Periodic idle polling; this is the primary wait to remove. |
| Startup worker | Retry delay of 2 s, split into 50 ms stop-flag sleeps | Retry deadline is intentional; 50 ms shutdown polling is not and must become interruptible. |
| Startup worker | Explicit-host `ToSocketAddrs` resolution | Potentially blocking OS resolver with no repository-controlled timeout or cancellation. Preserve and document this limitation unless separately redesigned. |
| Startup worker | UDP discovery, three 500 ms probe windows | Intentional bounded discovery receive timeouts (about 1.5 s total per discovery attempt). |
| Startup/reconnector | TCP connect timeout of 150 ms | Intentional bounded connection attempt. Reconnector also uses 150 ms retry pacing. |
| Startup/reconnector/writer | TCP writes with a 150 ms write timeout | Intentional I/O bound; includes Hello and queued pointer writes. A writer error can change connection state and therefore needs a runtime notification. |
| Reader worker | Blocking TCP `fill_buf`/frame reads | Intentional steady-state block. Socket shutdown during disconnect/drop is relied on to release it. Successful frame publication and terminal read outcomes need runtime notifications. |
| Writer worker | Blocking outbound-channel `recv()` | Intentional idle block. Dropping the sender releases it; the main thread uses bounded `try_send()` and does not block on this queue. |
| Latency logger worker | Blocking log-channel `recv()` when tracing is enabled | Intentional optional logger wait. Main-thread trace publication uses an unbounded sender and is not a runtime wake source. |
| Reconnector worker | Blocking internal wake-channel `recv()` and reader `join()` | Intentional wait for reader termination or shutdown. This internal channel remains separate from the planned runtime notifier. |
| Startup worker | Capacity-one startup update `send()` | May block until the main thread drains or drops the receiver; notification ordering must publish the update before waking the runtime. |
| Main thread | Mutex acquisition for connection/mailbox state | Expected to be short: network reads/writes occur outside these locks. No such lock may be held across the future blocking wait or MXCFB work. |
| Main thread | MXCFB update-submission ioctls | Driver calls may block; they are real display work, not idle polling. Remote-frame presentation currently records return from submission, not visual completion. |
| Display backend | One-shot 750 ms startup reassertion deadline | Real monotonic deadline that must determine the readiness wait timeout. |
| Shutdown cleanup | Marker-specific MXCFB completion wait while restoring the saved screen | Intentional cleanup block with driver-defined timing; it must remain reachable on local Exit, signals, and recoverable errors. |
| Transport shutdown | Socket disconnect, channel closure/wake, then writer and reconnector `join()` | Intentional cleanup wait. TCP write/connect paths are bounded, but the detached startup worker can remain in uncancellable OS resolution or bounded discovery until that call returns. |

Existing PaperPad latency stages are `input_received`, `pointer_queued`,
`socket_write_start`, `socket_write_done`, `socket_write_error`, `frame_received`,
`frame_present_start`, and `frame_present_return`. `input_received` is stamped for each
decoded pointer after `EvdevReader::drain()` returns and immediately before pointer
logging/dispatch; it is not the kernel event timestamp. `frame_received` is recorded
after protocol decode immediately before mailbox publication; there is no distinct
publication stage. `frame_present_return` measures backend submission return, not kernel
update completion or visible e-ink settling.

No baseline device run was performed: `mtp-rs devices` reported no accessible MTP
device (and an IOKit plug-in creation error). Connected idle, disconnected idle, startup
offline, touch bursts, frame bursts, local Exit, signal exit, and framebuffer restoration
therefore remain **not measured**. Idle wakeups/s, idle CPU, touch p50/p95/p99,
frame-publication-to-present p50/p95/p99, Exit latency, and restoration reliability are all
**not measured**, not zero. With negligible work, the fixed sleep permits at most roughly
20 idle service passes per second and adds up to one 50 ms sleep before dispatch. These
are source-level bounds, not measurements; end-to-end latency also includes work already
in progress, OS scheduling, network activity, and MXCFB submission.

### Milestone B — Add a minimal Linux readiness/wakeup primitive

Target: new small module, e.g. `src/runtime_wait.rs` (actual naming may follow local conventions), and `src/main.rs` module registration.

- [x] Expose the evdev file descriptor without transferring ownership, e.g. an accessor returning `BorrowedFd` or `RawFd` with documented validity.
- [x] Implement `RuntimeWait` to wait on evdev readiness plus one notifier FD using `libc::poll` (no fixed tick).
- [x] Return explicit readiness categories: input, notified, deadline elapsed, and shutdown (or an equally small representation).
- [x] Support infinite wait when no deadline exists, and monotonic `Instant` deadlines when one does.
- [x] Retry `EINTR` safely, recomputing the remaining deadline; do **not** treat every interruption as shutdown.
- [x] Make `POLLERR`, `POLLHUP`, `POLLNVAL`, EOF/bad-descriptor conditions, and simultaneous readiness observable without discarding other ready flags.
- [x] Implement a nonblocking, coalescing notifier; `EAGAIN` must not spin or enqueue unbounded wakeups. Always publish shared state *before* notifying.
- [x] Ensure waiter destruction/FD closure is safe even if initialization fails mid-way.

**Acceptance:** Linux tests prove an idle waiter blocks, a producer wakes it, multiple notifications coalesce, deadlines fire, and errors are observable. Keep Linux-specific tests behind appropriate target gates; do not silently skip them and declare the behavior proven.

#### Milestone B implementation (2026-11-15)

- `src/runtime_wait.rs` adds a Linux-only `poll(2)` waiter over borrowed evdev and a
  shared nonblocking `eventfd`, plus a cloneable coalescing notifier.
- `src/input/evdev.rs` exposes a documented borrowed raw descriptor; `src/main.rs`
  registers the Linux-only module. The production loop does not construct it yet.
- The waiter returns input flags, notification readiness, and deadline expiry together.
  It retries `EINTR`, rounds sub-millisecond timeouts upward, clamps large timeouts, and
  treats notifier saturation as an already-pending wake rather than a retry loop.
- Linux container tests executed all 13 waiter tests: idle blocking, producer wake,
  coalescing and saturation, deadline, simultaneous input/notification and error flags,
  notifier failure, hangup, invalid descriptor, injected `EINTR`, timeout conversion,
  and shared descriptor lifetime.
  resulting artifact is a static ARM EABI5 musl binary with no interpreter or GLIBC
  symbols. The default proxy-enabled build first failed before compilation because its
  configured Apple-container gateway address was unavailable.
- Linux Clippy was not executed because the builder image lacks `cargo-clippy`. A Linux
  production `cargo check` passed with warnings denied except for the repository's two
  pre-existing dead-code warnings; host Clippy passed with all warnings denied.
- No Kindle was accessible over MTP, so deployment did not run. Actual Paperwhite 6
  `eventfd` behavior, input teardown, signal wakeup, and framebuffer restoration remain
  unverified.

Next step: Milestone C must publish shared transport state before notifying this waiter
while retaining the existing 50 ms native-loop fallback.

### Milestone C — Wire transport-to-runtime notifications

Target: `src/net/mod.rs` and any narrow transport helpers required.

- [x] Supply a clone of `RuntimeNotifier` to the startup worker, reader, writer/disconnect path, and reconnector as appropriate.
- [x] Notify after a **validated frame is successfully published** in `FrameMailbox::publish()`. Do not notify for rejected frames unless a visible error/state change requires dispatch.
- [x] Notify after startup `Connected` or `Failed` update becomes available (including terminal worker failure), so `promote_startup()` runs without a timer.
- [x] Notify on observable connection transitions: disconnect, reconnect success, and relevant writer-triggered disconnect. Ensure the indicator changes even if no new frame is received.
- [x] Preserve the existing bounded outbound queue, connection-token checks, and latest-only frame semantics. Do not queue full frames in the notifier.
- [x] Avoid lost wakeups: a notification arriving before `poll()` must remain observable; drain wake tokens only after readiness and re-check shared state before sleeping again.
- [x] Ensure shutdown does not wait indefinitely for worker joins or a blocked socket. Retain the existing intentional socket timeouts and document any uncancellable resolver wait.

**Acceptance:** Tests demonstrate frame delivery, offline startup, connect/reconnect, read EOF, writer failure, and connection-indicator updates without periodic `service()` calls. Notification order must not cause stale state or a missed latest frame.

#### Milestone C implementation (2026-11-16)

- `src/net/mod.rs` carries a separate `RuntimeWake` alongside the existing internal
  reader-to-reconnector channel. It wraps the Linux `RuntimeNotifier` in production and
  remains a no-op for cross-platform transport tests.
- Validated mailbox publication, queued startup results, current writer disconnect,
  reader-driven disconnect, and reconnect success publish authoritative state first,
  release their locks, and then notify. Rejected frames and stale connection tokens do
  not generate a runtime wake.
- The mailbox remains latest-only, the outbound pointer queue remains capacity 16, and
  reconnect delays plus 150 ms connect/write timeouts are unchanged. The detached OS
  resolver limitation and existing shutdown joins are unchanged.
- `src/native_runtime.rs` creates the waiter/notifier pair and passes the notifier into
  transport, but deliberately retains the 50 ms loop. The waiter is not consumed until
  the D–F cutover, so this milestone does not claim reduced wakeups or dispatch latency.
- Six Linux notification tests cover latest-frame coalescing, rejected-frame silence,
  terminal offline startup, startup success, reader EOF/disconnect/reconnect, and an
  actual failed socket write with current-token disconnect. They wait on eventfd before
  reading transport state, rather than calling periodic `service()`.
- `make check` passed. A broad Linux run passed 164 tests with two unrelated existing
  timeout assertions skipped; when run unskipped, those two read Linux `SO_SNDTIMEO` as
  152 ms while expecting exactly 150 ms. All new notification tests passed.
- `make build PROXY=false` and `make verify PROXY=false` passed. The static ARM EABI5
  artifact has SHA-256 `9fc08f3cb20bf7286508e282fc72fa67a5dca2607584456a84528284561db37d`.
- The single connected Kindle was updated with the canonical MTP script. Independent
  readback matched the host hash; the prior binary is retained as `.previous`.
  Ephemeral runtime/latency logs were removed and `rust_x11_hello.status` was preserved.
- The user subsequently verified the deployed version on device on 2026-11-16. This
  operator confirmation does not extend to Paperwhite eventfd behavior, prompt wakeup,
  signal handling, framebuffer restoration, or performance; those remain unverified.

Next step is the D–F safety-gated cutover: main-loop readiness, race-free signal wake,
display deadlines, and interruptible startup retry must be implemented and tested
together before another deployable runtime is produced.

### Milestone D — Convert the native main loop

Target: `src/native_runtime.rs`, `src/input/evdev.rs`, possibly `src/runtime.rs`.

- [x] Replace unconditional `thread::sleep(POLL_INTERVAL)` with `RuntimeWait::wait(next_deadline)`.
- [x] After waking, service pending transport state/frame(s), process ready evdev input, and run due display maintenance; ensure startup state and initial UI draw are serviced once *before* first blocking wait.
- [x] On terminal evdev readiness or a drain EOF/error, cancel active contact and propagate the failure through the existing grab-release and framebuffer-cleanup path.
- [x] Preserve **event ordering within each decoded evdev batch** and local Exit handling. With `MAX_EVENTS_PER_DRAIN = 256`, a still-readable FD must be handled on the next cycle without waiting for unrelated notifications.
- [x] Avoid starvation under continuous frames or continuous touch. Use bounded processing per pass and allow the other ready source to progress.
- [x] Do not hold mailbox or connection mutexes across MXCFB display operations or a blocking wait.
- [x] Keep `PaperpadRuntime` as the sole main-thread owner of UI/contact state.
- [x] Remove the main-loop `POLL_INTERVAL` constant after tests pass. Prefer renaming `poll_frame()` only if doing so materially clarifies semantics; it currently means a nonblocking mailbox take, not a timed network poll.

**Acceptance:** Main-thread idle has no recurring 50 ms timeout; touch, latest frame, and connection status each cause an independent wake and immediate dispatch.

### Milestone E — Make shutdown race-free

Target: `src/main.rs`, `src/native_runtime.rs`, waiter initialization/teardown.

- [x] Do **not** rely solely on the existing atomic shutdown flag: a signal can arrive between checking the flag and entering an infinite wait.
- [x] Choose one rigorously safe strategy: **(preferred for minimal change)** register a nonblocking signal-safe wake FD and write it from the signal handler with `libc::write`, plus the atomic flag; or use `signalfd` with signals masked before spawning workers. Document the choice and thread/signal assumptions.
- [x] The signal handler must perform only async-signal-safe operations—no allocation, locking, logging, or Rust channel send. Handle notification saturation as already-woken, not as a busy loop.
- [x] Ensure a shutdown event cannot be missed before, during, or after `poll()` setup. Preserve existing signal semantics for the separate MXCFB benchmark mode.
- [x] Preserve cancellation of active contact, explicit `EVIOCGRAB` release, worker teardown, and framebuffer snapshot restoration on normal Exit, signals, and recoverable errors.

**Acceptance:** An indefinitely idle, disconnected PaperPad exits promptly on `SIGTERM` and `SIGINT` and restores the framebuffer; repeated signals and startup failures do not hang or skip cleanup.

### Milestone F — Convert maintenance and remaining idle retry polling

Target: `src/mxcfb/backend.rs`, `src/runtime.rs`, `src/net/mod.rs`.

- [x] Expose the next display deadline (e.g. `next_maintenance_deadline() -> Option<Instant>`); do not duplicate its timer state in the event loop.
- [x] Compute `poll()` timeout from the deadline using monotonic time. Handle expired deadlines immediately, sub-millisecond rounding correctly, and large timeouts safely.
- [x] Preserve the startup ~750 ms refresh/reassert behavior and its error/restore semantics; after consuming the deadline, allow infinite idle wait again.
- [x] Replace the startup worker's `STARTUP_STOP_POLL_INTERVAL` 50 ms stop-check sleeps with an interruptible timed wait (e.g. `Condvar` or a shutdown channel with timeout). This is **separate** from the main-loop migration.
- [x] Retain genuine reconnect retry backoff and socket operation timeouts. Do not replace intentional retry deadlines with busy retrying.

**Acceptance:** No recurring 50 ms idle check remains in the normal native runtime or startup worker. Display maintenance runs at its intended one-shot deadline; retries remain controlled when disconnected.

#### Milestones D–F implementation (2026-11-16)

- `src/native_runtime.rs` performs one initial service pass, then blocks in `RuntimeWait::wait()` until evdev, a coalesced transport/shutdown notification, or the display backend's monotonic deadline is ready. Each pass services at most the latest frame and one bounded evdev drain; level readiness schedules remaining input without a periodic timeout.
- The process signal handler publishes the atomic shutdown flag and performs only one nonblocking `write(2)` to a process-lifetime eventfd. Registration rechecks the flag and notifies after publication, closing the pre-registration race. The separate MXCFB benchmark continues using its existing bounded polling lifecycle.
- `MxcfbDisplayBackend` owns and exposes its one-shot startup reassertion deadline. The startup connection worker now uses an interruptible `Condvar` deadline; intentional discovery, reconnect, connect, and write timeouts remain unchanged.
- `make check`, Clippy with warnings denied, and `git diff --check` passed on macOS. All 224 host tests passed. In the Linux builder, 168 tests passed when skipping only the two pre-existing exact `SO_SNDTIMEO` assertions; the unfiltered run failed only because Linux returned 152 ms for the requested 150 ms. All new event-loop, level-readiness, deadline, stop-wake, and early-signal tests passed.
- `make build PROXY=false` and `make verify PROXY=false` passed. The static ARM EABI5 artifact has SHA-256 `7732a4dfa03fe1b991515fe4fd715607b0bdab0a0312153f3cb2d4fef683776b`.
- The single connected Kindle was updated with the canonical MTP script. Independent readback matched the host hash; the retained `.previous` binary matched the A–C hash `9fc08f3cb20bf7286508e282fc72fa67a5dca2607584456a84528284561db37d`. Append-style logs were removed and `rust_x11_hello.status` was preserved before physical validation.
- The connected **Test MXCFB TERM cleanup (15s)** run stopped through the signal path, released `EVIOCGRAB`, restored the full framebuffer snapshot, and exited 0; the operator confirmed the stock UI returned. The log also contains native evdev Press/Release pairs for the matching deployment. See `docs/archive/mxcfb-device-evidence.md`.
- A second run with PaperSpoon stopped remained disconnected through repeated discovery failures. TERM produced `stopped reason=signal` before the in-flight discovery attempt completed, followed by explicit grab release, full-snapshot restoration, exit 0, and operator-confirmed stock UI return.
- A disconnected **Run Paperpad (5m)** run recorded a bottom-strip Press/Release pair, `ui action=activate system=exit`, and `stopped reason=local_exit`, followed by explicit grab release, full-snapshot restoration, exit 0, and operator-confirmed stock UI return.
- Still unverified on device: numeric signal-wake latency, repeated-signal behavior, active-contact cancellation, input-failure cleanup, idle wakeup rate, and any latency, driver visual-completion, CPU, or battery improvement.
- Follow-up verification replaced the two exact Linux `SO_SNDTIMEO` observation assertions with a 150–170 ms bound that still rejects missing, shortened, or materially weakened write timeouts. `make check` passed and the unfiltered Linux PaperPad suite passed 170/170. ARM build/verification passed; artifact SHA-256 `75e33a9a46f3139262f8fce3a3be240321f768fa453767cedaa994b3a2b01934` was deployed through the canonical updater and matched independent MTP readback. The retained `.previous` binary matches the physically validated `7732a4dfa03fe1b991515fe4fd715607b0bdab0a0312153f3cb2d4fef683776b` build. The source change is test-only and adds no new runtime-behavior claim.

### Milestone G — Verification, regression review, and documentation

- [x] Run from repository root:

  ```sh
  make check
  make build
  make verify
  git diff --check
  ```

- [x] If `make check` runs on macOS, also execute the new Linux-specific waiter tests in the Linux build environment; confirm they actually ran.
- [x] Inspect the diff for accidental changes to `paper-protocol`, PaperSpoon UI, packet formats, X11 code, KUAL launcher, and benchmark paths.
- [x] Update architecture notes to describe event-driven wakeups. Correct only directly affected stale statements in `AGENTS.md`/comments; avoid unrelated documentation refactors.
- [x] Deploy to the Kindle using the repository's documented MTP procedure, verify the deployed binary checksum, and collect device logs.
- [x] Report measurements and limitations; **do not claim 24 FPS, visual refresh completion, or battery savings without independent evidence**.

**Acceptance:** All automated checks pass, hardware observations are recorded, and any unverified behavior is explicitly listed.

## 4. Test matrix

| Scenario | Required result |
| --- | --- |
| Connected idle for several minutes | Main runtime blocks; no periodic 50 ms wake. |
| Offline startup | Exit remains responsive; startup attempts continue with backoff, not busy spin. |
| Startup success after multiple failures | Startup update wakes runtime and UI reflects connection. |
| Single touch press/move/release | Same decoded sequence and remote pointer phases; no 50 ms service wait. |
| Dense touch events (>256 records) | Remaining records drain over subsequent readiness cycles; no lost release/starvation. |
| Rapid frame burst | Latest frame wins; bounded memory; no unbounded wake queue. |
| Frame received while idle | Wake and render without another input event. |
| Reader EOF/network loss | Connection indicator updates; retained displayed frame is not cleared. |
| Reconnect without input | Runtime sees reconnect; matching authoritative frame behavior remains unchanged. |
| Writer error / stale token | Correct disconnect notification and no pointer replay. |
| Local Exit while disconnected | Clean and prompt exit without network dependency. |
| Signal during indefinite wait | Prompt shutdown; no lost-signal race. |
| Evdev `POLLHUP`, EOF, or read failure | Contact cancelled, error reported, grab released. |
| Simultaneous input + frame | Both serviced fairly; no deadlock/starvation. |
| Startup reassert deadline | Runs once on time and preserves display state. |
| Normal/error cleanup | Preexisting framebuffer snapshot restored, including GC16 restoration path. |

## 5. Performance reporting

Record before/after **under the same conditions**, preferably with multiple samples:

| Metric | Before | After | Method / caveat |
| --- | --- | --- | --- |
| Idle main-loop wakeups per second | Not measured | Not measured | `strace`/`perf` if available, otherwise explicit instrumentation. |
| Idle CPU usage | Not measured | Not measured | Same duration and connection state. |
| Touch decode-to-queue p50/p95/p99 | Not measured | Not measured | Existing latency trace points. |
| Frame publication-to-present-start p50/p95/p99 | Not measured | Not measured | Add narrow timestamps if needed; distinguish from network decode. |
| Local Exit latency from idle | Not measured | Not measured | Include disconnected case. |
| Framebuffer restoration reliability | Not measured | Not measured | Hardware check after normal and abnormal shutdown. |

**Interpretation:** Faster software dispatch does not imply faster e-ink panel settling. Kernel update submission, waiting for an ioctl marker, and visual completion are distinct events.

## 6. Risks and engineering decisions to document

- 💡 **Single-reactor simplicity:** `poll(2)` plus a few worker threads is enough for the current FD count; revisit `epoll` or Tokio only if measured scaling pressure emerges.
- 💡 **Lost-wakeup correctness:** event delivery must be level/coalescing-safe across races; the mailbox remains the authoritative data source, not the number of wake notifications.
- 💡 **Backpressure:** rendering may be slower than the sender. Keep latest-only frame replacement, preserve bounded pointer queue, and consider explicit display pacing only after measurements.
- 💡 **Signal routing:** Linux signals can be delivered to different threads. Choose signal handling accordingly; never assume an interrupted `poll()` alone is reliable.
- 💡 **Hardware compatibility:** validate `eventfd`, `poll`, ioctl, and libc behavior with the actual ARMv7 musl build and Paperwhite 6 kernel.
- 💡 **Deadlines versus polling:** reconnect backoff, display reassertion, and socket timeouts are intentional; removing all timers would be incorrect.
- 💡 **Observability:** distinguish wake-to-dispatch latency from panel response time; retain trace event meanings to compare before and after.

## 7. Codex execution contract

1. Implement **one milestone at a time**, preferring a small, reviewable diff.
2. Before editing, report any discrepancy between this plan and the checkout. Do not invent APIs or file paths.
3. Write tests for newly introduced wakeup/race paths, including failure cases, before claiming a milestone complete.
4. After each milestone, summarize changed files, expected behavior, tests actually executed, known limitations, and the next step.
5. Preserve all existing public behavior, cleanup invariants, and PPFB v2 compatibility unless the user explicitly approves a deviation.
6. Do not claim device or performance verification from successful compilation alone.

### Suggested PR boundaries

- **PR 1:** A–B — baseline, waiter/notifier primitives, Linux tests.
- **PR 2:** C — transport notifications; retain the existing 50 ms loop as a temporary fallback.
- **PR 3:** D–F together — switch the native loop only with signal-safe shutdown and deadline-based display maintenance; eliminate worker stop-check polling.
- **PR 4 (or device-validation commit):** G — measurements, hardware evidence, and focused documentation.

> **Definition of done:** When PaperPad is idle, its main thread blocks indefinitely unless a real deadline exists; touch, frames, connection transitions, and shutdown all wake it independently; every previous lifecycle and cleanup guarantee remains verifiable.

# PaperSpoon — Ratatui operator console

## Goal

Replace the interactive line-oriented console with a fullscreen Ratatui TUI. Keep the upper portion as a continuously updating, scrollable log and place grouped action buttons in the lower portion. PaperSpoon must remain fully operational while the operator navigates menus, edits parameters, or reads old logs.

The fullscreen console is the sole operator mode. Regular launches require terminal stdin and stdout. Internal Now Playing helpers run independently of the terminal.

This is a host-only change. It must not change the Kindle UI, PaperView, PPFB v2/v3, compression selection, framebuffer rendering, waveform policy, or Now Playing sampling cadence.

## 1. Inspect the current checkout

- [ ] Read `AGENTS.md`, `README.md`, `tools/paperspoon/Cargo.toml`, and the operator, terminal, server, logging, and provider modules. Follow local repository instructions.
- [ ] Inventory operator actions, startup options, log writers, subprocess streams, signal handlers, and worker lifecycles. Use `rg` for discovery.
- [ ] Confirm the exact command variants and current connection/provider lifecycle on the checked-out branch before editing. Public master snapshots are not a pinned revision.
- [ ] Run the existing host tests as a baseline. Note existing failures separately.

## 2. Target UI

```text
┌ PaperSpoon ─ TCP :5581 ─ UDP :5580 ─ Connected ─ PPFB v3 ────────────┐
│ Peer: 192.168.0.3:56724 | Viewport: 1272x1624 | Compression: Auto   │
├ Realtime log ──────────────────────────────────────── FOLLOW ─────┤
│ discovery request from=192.168.0.3:5582 nonce=...                   │
│ discovery response sent to=192.168.0.3:5582 tcp_port=5581           │
│ nowplaying sample provider=Apple Music elapsed_us=219982 ...       │
│ connected: 192.168.0.3:56724                                        │
│ received ... hello protocol=3 viewport=1272x1624 ...                │
│ sent live nowplaying frame id=514 width=1272 height=1624 ...        │
│ sent live nowplaying frame id=515 width=1272 height=1624 ...        │
│ ...                                                                │
├ Application ────────────────────┬ Now Playing ────────────────────┤
│ [Show UI] [Viewport: Auto ▾]     │ [Start] [Stop] [Refresh]        │
│                                 │ [Status] [Snapshot]             │
├ Diagnostic frames ──────────────┼ Gray8 / Images ─────────────────┤
│ [White] [Black] [Horizontal]     │ [Gradient] [Bars] [Gray…]       │
│ [Checkerboard] [Border] [Corners]│ [JPEG…]                         │
├ Test provider (when enabled) ────┴─────────────────────────────────┤
│ [Play] [Pause] [Next] [Seek…] [Fail] [Recover] [Slow] [Normal]      │
├───────────────────────────────────────────────────────────────────┤
│ Tab: group  Arrows: select  Enter: run  F: follow  Q: quit         │
└───────────────────────────────────────────────────────────────────┘
```

The mockup is illustrative, not a fixed terminal size. Allocate remaining vertical space to the log; action groups have compact content-driven height. All actions must remain reachable in small terminals by switching to a compact group selector or scrolling the action panel. At extremely small sizes, show a safe minimal resize prompt while keeping background services alive. Do not truncate away the only way to exit.

### Action mapping

| Group | Button | Existing command / behavior |
|---|---|---|
| Application | Show UI | `ui WIDTHxHEIGHT` |
| Application | Viewport | Local parameter: negotiated viewport by default; optional explicit dimensions |
| Diagnostic | White, Black, Horizontal, Checkerboard, Border, Corners | `frame PATTERN WIDTHxHEIGHT` |
| Gray8 | Gradient, Bars | `frame gray-gradient WIDTHxHEIGHT`, `frame gray-bars WIDTHxHEIGHT` |
| Gray8 | Gray… | `frame gray VALUE WIDTHxHEIGHT` |
| Images | JPEG… | `frame jpeg PATH` |
| Now Playing | Snapshot | `frame nowplaying` |
| Now Playing | Start, Stop, Status, Refresh | `nowplaying start/stop/status/refresh` |
| Test provider | Play, Pause, Next, Fail, Recover, Slow, Normal | `nowplaying fake ACTION` |
| Test provider | Seek… | `nowplaying fake seek SECONDS` |

Keep every action in the table reachable through the console. Enable test-provider buttons only when the selected provider supports them. Keep startup options on the command line.

### Parameters and interaction

- Default dimensions come from the active negotiated remote viewport, not the terminal dimensions. Do not hardcode 1272x1624. Explicit dimensions remain available for diagnostics; retain existing validation and rejection behavior. Automatic mode without a negotiated viewport cannot dispatch dimension-dependent actions.
- `Gray…` opens a value editor (integer 0–255). `JPEG…` opens a local path editor supporting spaces and Unicode. `Seek…` accepts finite nonnegative seconds. Validate via existing rules; do not shell-expand paths or invoke a shell.
- `Tab` / `Shift+Tab`: move focus between groups and log; arrows move among buttons; `Enter` activates; `Esc` cancels an editor. Ignore repeat activation from held keys. Provide mouse click support for visible enabled buttons, mouse wheel over the log, and keyboard equivalents for everything.
- `PageUp` / `PageDown`, `Home` / `End` when log focused; `F` toggles follow-tail outside text editors. Scrolling up pauses following; End resumes it. Show FOLLOW/PAUSED and unseen-record count. Log scrolling must not steal action focus.
- Show pending, disabled, focused and active states distinctly, not by color alone. Derive button availability from the actual runtime and existing command semantics. Do not disable valid disconnected actions (e.g. live start if currently supported).
- Display command errors in the log and a compact status line; do not quit on invalid input. Do not claim a frame was sent or displayed merely because a button was pressed.

## 3. Architecture

```text
Ratatui actions ─────────> typed operator command ─> dispatcher
                                                     │
                                                     ├─ FrameSender / CurrentConnection
                                                     ├─ LiveNowPlaying
                                                     └─ application viewport notifications
                                                              │
Existing TCP / UDP / provider workers ──> log + runtime state ─┤
                                                              v
                         bounded log store / latest status -> Ratatui
```

Keep the current synchronous/threaded architecture and standard-library channels where appropriate. Do not introduce Tokio, an actor framework, or a second network server just for this UI.

Proposed layout (adapt to the checkout, do not move unrelated modules):

```text
tools/paperspoon/src/
  main.rs                 # startup and internal helper entry point
  operator.rs             # typed actions and validation
  dispatch.rs             # command execution and outcomes
  operator_log.rs         # console logging abstraction, bounded TUI store
  terminal.rs             # terminal lifecycle, event loop, and worker wiring
  tui/
    app.rs                # focus, modal inputs, log viewport, action selection
    view.rs               # pure Ratatui rendering and button hit rectangles
```

Keep typed commands and their validation in `operator.rs`. Ratatui constructs `OperatorCommand` values directly and sends them through one dispatcher.

### Shared dispatcher

- [ ] Preserve command ordering, errors, viewport notifications, connection tokens, retained frames, and log records in the dispatcher.
- [ ] Preserve the behavior in which a manual frame, JPEG, snapshot or application frame stops live Now Playing before sending; preserve Stop's return-to-application behavior where implemented. Do not introduce a second display-mode state machine in the TUI.
- [ ] In TUI mode, execute commands on one background command worker with a bounded queue. Keep input and rendering off this worker. Never call a blocking socket write, JPEG decoder, media acquisition, or worker join from the UI drawing/input path.
- [ ] Serialize operator commands, identify pending operations, handle full/closed queues explicitly, and avoid silently retrying a command against a replacement connection. Preserve existing connection/viewport validation and single-client behavior. Do not enqueue frame sends indefinitely.
- [ ] Obtain an authoritative lightweight status snapshot/event stream from existing runtime state (connection, negotiation, viewport, compression, live state, provider, last command outcome). Avoid locking frame/render state on the UI thread if that lock spans encoding or socket I/O. Old-session events must not overwrite current-session status.
- [ ] Keep `source=tui` on operator frame records. Preserve other log fields and their measurement meaning.

## 4. Logging: separate terminal ownership from diagnostics

- [ ] Introduce one explicit console-log abstraction shared by main, discovery, sessions, provider/live workers and error paths. Use the project's existing logging approach where possible; no logging-framework migration is required.
- [ ] Route operational messages to the TUI log store, including startup, discovery, connection/hello/disconnect, pointer/actions, Now Playing samples and sends, frame diagnostics, errors, and shutdown. Audit direct printing and subprocess inheritance.
- [ ] Preserve existing persistent log-file and latency JSONL semantics, paths, timestamps, fields and flush behavior. Avoid duplicate file writes. The TUI is not the authoritative benchmark log and does not replace it.
- [ ] Use a bounded in-memory ring buffer (proposed: 5,000 records with a bounded per-record size). Track evictions and any dropped UI records; surface them in the log/status panel. Never block frame transport on TUI log backpressure. Do not silently drop persistent records because the UI buffer is full.
- [ ] Preserve full diagnostic fields in normal-size records. Wrap long lines or provide horizontal scrolling; sanitize terminal control characters and escape sequences before displaying untrusted log strings. Correctly handle Unicode, empty lines and resize. A paused viewport should remain anchored until its records are evicted; indicate when this happens.
- [ ] Keep a bounded work budget when draining log events. Continuous log traffic must not starve keyboard input or terminal redraws. Do not clone all history on each frame; render only the visible window.
- [ ] Do not globally redirect file descriptors or capture the process's stdout using a pipe. The internal Now Playing helper uses stdout for binary responses; its early dispatch and protocol must remain untouched. Audit newer helper modes too. Child processes must not write raw text to the active terminal; route/capture their diagnostics while preserving their established functional output and cleanup.
- [ ] Errors opening/writing log files must remain visible without recursive logging or deadlocking. Initialize sinks before starting producers; startup errors before terminal initialization can use ordinary stderr.

## 5. Terminal lifecycle and compatibility

- [ ] Add Ratatui with the Crossterm backend only to `tools/paperspoon/Cargo.toml`. Select versions compatible with the pinned Rust 1.97.1 toolchain and the actual workspace dependency policy. Use Ratatui's compatible Crossterm re-export or an explicitly matched dependency; update `Cargo.lock`. Verify on macOS and the existing supported host check targets.
- [ ] Require terminal stdin and stdout (`std::io::IsTerminal`) before starting the console or listeners. Report a clear error for redirected launches. Keep startup options for ports, logs, forwarding, compression, and Now Playing.
- [ ] Handle internal helper arguments before logging/TUI setup or terminal detection. Never enter raw mode or alternate screen in a helper.
- [ ] Give the TUI event loop sole ownership of terminal input and output.
- [ ] Use alternate screen, raw mode, cursor management and mouse capture only while TUI is active. Restore all terminal settings on normal exit, errors, panic and handled SIGINT/SIGTERM. Ratatui's terminal restore helpers cover terminal basics; explicitly undo extra modes enabled by this application.
- [ ] Handle Ctrl+C as a raw-mode key event as well as process signals; coordinate with existing live shutdown/signal handling rather than adding competing exit paths. Avoid a direct `process::exit` that bypasses terminal restoration. Ensure a signal received before the UI starts is also handled correctly.
- [ ] Stop accepting commands, signal workers, interrupt blocked sockets, stop the live provider and reap its helper processes, then join workers with bounded shutdown behavior. Restore terminal settings even if cleanup fails. Document the SIGKILL limitation.
- [ ] Event loop: use bounded input polling (e.g. 25–50 ms maximum idle interval) or wake-driven events, with dirty-state redraws capped around 20 Hz. Redraw on log arrival, input, resize, and status changes. Terminal redraws must not drive remote framebuffer updates or media polling.

## 6. Implementation milestones

1. Extract command dispatch while keeping the old console behavior. Add regression tests and run host checks.
2. Introduce log routing and lightweight state reporting. Audit all print and subprocess output sites.
3. Add Ratatui layout, log pane, bounded log history, action groups, keyboard/mouse navigation and parameter editors. Use a test backend for rendering.
4. Wire TUI to the dispatcher and runtime. Require a terminal, remove the line-oriented console, and verify lifecycle cleanup and responsiveness while live frames are sent.
5. Complete regression and integration tests, update concise operator documentation, and stop once the acceptance criteria pass. No new transport benchmark campaign or broad codebase cleanup is part of this task.

## 7. Tests and verification

### Automated tests

- [ ] Typed dispatch semantics for all patterns, explicit dimensions, JPEG paths with spaces, snapshot and fake commands.
- [ ] Valid/invalid inputs, cancellation without dispatch, disabled actions, exactly one dispatch per activation, and queue-full/closed behavior.
- [ ] Ratatui `TestBackend`: regular and narrow terminal sizes, visible action groups, focus movement, long Unicode log lines, log follow/scroll/eviction, resize, and modal overlays. No panics on tiny or zero-size layouts.
- [ ] Log routing to the TUI, bounded storage under sustained traffic, persistent log record counts, and terminal escape sanitization.
- [ ] Runtime state updates during connection replacement, disconnect/reconnect, live start/stop, provider failure/recovery, and unsupported pixel formats.
- [ ] Simulated slow command execution and sustained logging: UI still handles navigation, cancellation of local input and log scrolling. No stale success status, no duplicate sends.
- [ ] Redirected launches fail before starting listeners or writing terminal escapes. Preserve binary helper response framing. Test shutdown while disconnected, connected, and with a live provider.

### Run on the pinned development environment

```sh
nix develop -c cargo test -p paperspoon
nix develop -c make check
```

Use existing scripts and flags as discovered in the checkout. Run the host interactively in Kitty/tmux or another compatible terminal:

```sh
nix develop -c cargo run --release -p paperspoon -- 5581 /tmp/paperspoon.log
```

Verify that a redirected launch reports the terminal requirement:

```sh
nix develop -c cargo run --release -p paperspoon -- 5581 /tmp/paperspoon.log </dev/null
```

### Manual end-to-end smoke test

- Start PaperSpoon in TUI mode; confirm idle console, listener and discovery status.
- Connect PaperView with the existing command (`cargo run -p paperview -- --host 127.0.0.1 --port 5581 --viewport 1272x1624`). PaperSpoon serves a single display client; do not unintentionally replace a live Kindle session.
- Exercise application, Mono1, Gray8, JPEG and Now Playing actions; test live mode with existing fake-provider setup and then real Apple Music if available.
- While live updates are running, scroll the log, edit parameters, resize the terminal, click/navigate buttons, and switch back to Follow. Verify no raw console text corrupts the screen.
- Verify reconnects, invalid inputs, unavailable provider, unsupported format and slow writes. Confirm existing command semantics and frame timing fields remain intact. Local socket-write timing is not proof of remote display completion.
- Quit using Q (outside editors), Ctrl+C and external SIGTERM in separate runs. Confirm terminal restoration, listener port release and no leaked helper processes. Repeat launch/quit within tmux.
- No Kindle deployment is required for this host-only change. If a real Kindle is available, an optional connection smoke test is useful; report it as unverified if not performed. Follow `AGENTS.md` if any unexpected device-affecting change is necessary.

## 8. Acceptance criteria / definition of done

- [ ] Interactive PaperSpoon starts a fullscreen TUI without a competing stdin reader; normal logs are contained in the upper pane.
- [ ] Every existing operator command is available through grouped actions; parameterized actions use local editors instead of a global command prompt.
- [ ] Live log updates continue independently of input; follow-tail, history navigation, resizing, keyboard and mouse controls work.
- [ ] Operator actions execute through the validated dispatcher and preserve frame and Now Playing semantics.
- [ ] Background processing continues while the UI is busy; log memory is bounded, and frame writes do not wait on the UI.
- [ ] Regular launches require a terminal. Internal helper output remains binary-clean.
- [ ] Terminal state is restored on normal exit and handled failure paths; no persistent listener/provider/helper leak in tested shutdown cases.
- [ ] Existing host checks and new targeted tests pass; host-only dependencies do not enter the Kindle binary's dependency graph. No protocol or device changes are needed.
- [ ] README documents the terminal requirement, controls, and logging. Report actual check results, remaining limitations, and any unperformed device tests. Verify physical-device behavior with device evidence.

## References

- https://github.com/num13ru/rust_x11_hello
- https://github.com/num13ru/rust_x11_hello/blob/master/tools/paperspoon/src/main.rs
- https://ratatui.rs/how-to/develop-apps/terminal-and-event-handler/
- https://docs.rs/ratatui/latest/ratatui/init/index.html

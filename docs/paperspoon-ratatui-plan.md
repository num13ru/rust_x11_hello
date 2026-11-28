# PaperSpoon — Ratatui operator console

## Goal

Replace the interactive line-oriented console with a fullscreen Ratatui TUI. Keep the upper portion as a continuously updating, scrollable log and place grouped action buttons in the lower portion. PaperSpoon must remain fully operational while the operator navigates menus, edits parameters, or reads old logs.

This is a host-only change. It must not change the Kindle UI, PaperView, PPFB v2/v3, compression selection, framebuffer rendering, waveform policy, or Now Playing sampling cadence.

## 1. Inspect the current checkout

- [ ] Read `AGENTS.md`, `README.md`, `tools/paperspoon/Cargo.toml`, `tools/paperspoon/src/main.rs`, `diagnostic.rs`, `live_now_playing.rs`, `discovery.rs`, `server.rs`, `latency.rs`, and the current provider/helper modules. Follow local repository instructions.
- [ ] Inventory all operator commands, startup flags, stdout/stderr writers, log-file writers, child-process stdio, signal handlers, worker lifecycles, and scripts that pipe commands into PaperSpoon. Use `rg` for discovery.
- [ ] Confirm the exact command variants and current connection/provider lifecycle on the checked-out branch before editing. Public master snapshots are not a pinned revision.
- [ ] Run the existing host tests as a baseline. Note existing failures separately.

Known integration points from public master: `StdinCommand`, `LiveCommand`, `parse_stdin_command`, `FrameSender`, `CurrentConnection`, `LiveNowPlaying`, and the application viewport notification channel. The stdin worker currently performs command dispatch as well as console input. Extract that dispatch, not the rendering or transport implementation.

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

Reconcile this table with the local parser. Every existing operator command must remain reachable without typing a command. Fake-provider buttons are diagnostics, not actual Apple Music playback controls; hide or disable them unless supported by the selected provider. Preserve existing startup flags rather than adding controls for every flag in this iteration.

### Parameters and interaction

- Default dimensions come from the active negotiated remote viewport, not the terminal dimensions. Do not hardcode 1272x1624. Explicit dimensions remain available for diagnostics; retain existing validation and rejection behavior. Automatic mode without a negotiated viewport cannot dispatch dimension-dependent actions.
- `Gray…` opens a value editor (integer 0–255). `JPEG…` opens a local path editor supporting spaces and Unicode. `Seek…` accepts finite nonnegative seconds. Validate via existing rules; do not shell-expand paths or invoke a shell.
- `Tab` / `Shift+Tab`: move focus between groups and log; arrows move among buttons; `Enter` activates; `Esc` cancels an editor. Ignore repeat activation from held keys. Provide mouse click support for visible enabled buttons, mouse wheel over the log, and keyboard equivalents for everything.
- `PageUp` / `PageDown`, `Home` / `End` when log focused; `F` toggles follow-tail outside text editors. Scrolling up pauses following; End resumes it. Show FOLLOW/PAUSED and unseen-record count. Log scrolling must not steal action focus.
- Show pending, disabled, focused and active states distinctly, not by color alone. Derive button availability from the actual runtime and existing command semantics. Do not disable valid disconnected actions (e.g. live start if currently supported).
- Display command errors in the log and a compact status line; do not quit on invalid input. Do not claim a frame was sent or displayed merely because a button was pressed.

## 3. Architecture

```text
Legacy stdin parser ───┐
                      ├─> typed operator command ─> shared dispatcher
Ratatui actions ───────┘                              │
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
  main.rs                 # options, startup, wiring, lifecycle
  operator.rs             # shared typed command dispatcher and outcomes
  operator_log.rs         # console logging abstraction, bounded TUI store
  tui/
    mod.rs                # terminal setup, event loop and teardown
    app.rs                # focus, modal inputs, log viewport, action selection
    view.rs               # pure Ratatui rendering and button hit rectangles
```

Keep `diagnostic.rs` as the source of command parsing and validation unless a small rename/extraction is needed. Use the existing command types (or a neutral `OperatorCommand` name); construct typed actions directly from TUI rather than feeding text into stdin. Expose validated constructors as needed. Both frontends must use one command execution path.

### Shared dispatcher

- [ ] Move the existing stdin match and its side effects into a reusable dispatcher. Preserve ordering, errors, viewport notifications, connection tokens, retained frames, and existing log records.
- [ ] Preserve the behavior in which a manual frame, JPEG, snapshot or application frame stops live Now Playing before sending; preserve Stop's return-to-application behavior where implemented. Do not introduce a second display-mode state machine in the TUI.
- [ ] In TUI mode, execute commands on one background command worker with a bounded queue. Keep input and rendering off this worker. Never call a blocking socket write, JPEG decoder, media acquisition, or worker join from the UI drawing/input path.
- [ ] Serialize operator commands, identify pending operations, handle full/closed queues explicitly, and avoid silently retrying a command against a replacement connection. Preserve existing connection/viewport validation and single-client behavior. Do not enqueue frame sends indefinitely.
- [ ] Obtain an authoritative lightweight status snapshot/event stream from existing runtime state (connection, negotiation, viewport, compression, live state, provider, last command outcome). Avoid locking frame/render state on the UI thread if that lock spans encoding or socket I/O. Old-session events must not overwrite current-session status.
- [ ] Keep `source=stdin` for legacy records. If adding `source=tui`, check existing consumers first; preserve other log fields and measurement meaning.

## 4. Logging: separate terminal ownership from diagnostics

- [ ] Introduce one explicit console-log abstraction shared by main, discovery, sessions, provider/live workers and error paths. Use the project's existing logging approach where possible; no logging-framework migration is required.
- [ ] Route normal operational stdout/stderr messages to the TUI log store in TUI mode and to their original streams in legacy mode. Include startup, discovery, connection/hello/disconnect, pointer/actions, Now Playing samples and sends, frame diagnostics, errors, and shutdown. Audit direct printing and subprocess inheritance, not only `main.rs`.
- [ ] Preserve existing persistent log-file and latency JSONL semantics, paths, timestamps, fields and flush behavior. Avoid duplicate file writes. The TUI is not the authoritative benchmark log and does not replace it.
- [ ] Use a bounded in-memory ring buffer (proposed: 5,000 records with a bounded per-record size). Track evictions and any dropped UI records; surface them in the log/status panel. Never block frame transport on TUI log backpressure. Do not silently drop persistent records because the UI buffer is full.
- [ ] Preserve full diagnostic fields in normal-size records. Wrap long lines or provide horizontal scrolling; sanitize terminal control characters and escape sequences before displaying untrusted log strings. Correctly handle Unicode, empty lines and resize. A paused viewport should remain anchored until its records are evicted; indicate when this happens.
- [ ] Keep a bounded work budget when draining log events. Continuous log traffic must not starve keyboard input or terminal redraws. Do not clone all history on each frame; render only the visible window.
- [ ] Do not globally redirect file descriptors or capture the process's stdout using a pipe. The internal Now Playing helper uses stdout for binary responses; its early dispatch and protocol must remain untouched. Audit newer helper modes too. Child processes must not write raw text to the active terminal; route/capture their diagnostics while preserving their established functional output and cleanup.
- [ ] Errors opening/writing log files must remain visible without recursive logging or deadlocking. Initialize sinks before starting producers; startup errors before terminal initialization can use ordinary stderr.

## 5. Terminal lifecycle and compatibility

- [ ] Add Ratatui with the Crossterm backend only to `tools/paperspoon/Cargo.toml`. Select versions compatible with the pinned Rust 1.97.1 toolchain and the actual workspace dependency policy. Use Ratatui's compatible Crossterm re-export or an explicitly matched dependency; update `Cargo.lock`. Verify on macOS and the existing supported host check targets.
- [ ] Default to TUI only when both stdin and stdout are terminals (`std::io::IsTerminal`). Add `--no-tui` for explicit legacy console mode, including piped scripts and output redirection. Optional `--tui` may force a TUI only on a suitable terminal; otherwise report a clear error. Reject contradictory flags. Keep all existing positional arguments and flags.
- [ ] Handle internal helper arguments before logging/TUI setup or terminal detection. Never enter raw mode or alternate screen in a helper.
- [ ] Exactly one owner of terminal input/output: TUI event loop in interactive mode; existing line reader and console output in legacy mode. Never spawn `paperspoon-stdin` in TUI mode.
- [ ] Use alternate screen, raw mode, cursor management and mouse capture only while TUI is active. Restore all terminal settings on normal exit, errors, panic and handled SIGINT/SIGTERM. Ratatui's terminal restore helpers cover terminal basics; explicitly undo extra modes enabled by this application.
- [ ] Handle Ctrl+C as a raw-mode key event as well as process signals; coordinate with existing live shutdown/signal handling rather than adding competing exit paths. Avoid a direct `process::exit` that bypasses terminal restoration. Ensure a signal received before the UI starts is also handled correctly.
- [ ] Introduce cooperative shutdown only where needed: stop accepting commands, signal workers, interrupt blocked listener/socket operations, stop the live provider and reap its helper processes, then join workers with bounded shutdown behavior. Do not let a blocked stdin reader or socket prevent exit. Ensure terminal restoration even if cleanup fails. SIGKILL cannot be handled; document this limitation.
- [ ] Event loop: use bounded input polling (e.g. 25–50 ms maximum idle interval) or wake-driven events, with dirty-state redraws capped around 20 Hz. Redraw on log arrival, input, resize, and status changes. Terminal redraws must not drive remote framebuffer updates or media polling.

## 6. Implementation milestones

1. Extract command dispatch while keeping the old console behavior. Add regression tests and run host checks.
2. Introduce log routing and lightweight state reporting. Keep legacy mode functional. Audit all print/child output sites.
3. Add Ratatui layout, log pane, bounded log history, action groups, keyboard/mouse navigation and parameter editors. Use a test backend for rendering.
4. Wire TUI to the real dispatcher and runtime, implement mode selection and lifecycle cleanup. Verify responsiveness while live frames are sent.
5. Complete regression and integration tests, update concise operator documentation, and stop once the acceptance criteria pass. No new transport benchmark campaign or broad codebase cleanup is part of this task.

## 7. Tests and verification

### Automated tests

- [ ] Existing parser and dispatch semantics, including all patterns, explicit dimensions, JPEG paths with spaces, snapshot and fake commands. Test TUI and CLI command equivalence.
- [ ] Valid/invalid inputs, cancellation without dispatch, disabled actions, exactly one dispatch per activation, and queue-full/closed behavior.
- [ ] Ratatui `TestBackend`: regular and narrow terminal sizes, visible action groups, focus movement, long Unicode log lines, log follow/scroll/eviction, resize, and modal overlays. No panics on tiny or zero-size layouts.
- [ ] Log routing for both modes; bounded store under sustained synthetic traffic; no duplicate records in persistent logs; no terminal escape injection.
- [ ] Runtime state updates during connection replacement, disconnect/reconnect, live start/stop, provider failure/recovery, and unsupported pixel formats.
- [ ] Simulated slow command execution and sustained logging: UI still handles navigation, cancellation of local input and log scrolling. No stale success status, no duplicate sends.
- [ ] Headless CLI and internal helper paths: no terminal escapes in redirected output; preserve existing stdout/stderr and binary helper response framing. Test shutdown while disconnected, connected and with a live provider.

### Run on the pinned development environment

```sh
nix develop -c cargo test -p paperspoon
nix develop -c make check
```

Use existing scripts and flags as discovered in the checkout. Run the host interactively in Kitty/tmux or another compatible terminal:

```sh
nix develop -c cargo run --release -p paperspoon -- 5581 /tmp/paperspoon.log
```

Verify the legacy path separately (example command stream, stop the server using the existing lifecycle; EOF may not terminate it):

```sh
printf 'nowplaying status\n' | nix develop -c cargo run --release -p paperspoon -- --no-tui 5581 /tmp/paperspoon.log
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
- [ ] Operator actions execute through the same validated dispatcher as the legacy CLI, with existing frame/Now Playing semantics intact.
- [ ] Background processing continues while the UI is busy; log memory is bounded, and frame writes do not wait on the UI.
- [ ] Non-TTY launches and `--no-tui` retain the previous command-line workflow; internal helper output remains binary-clean.
- [ ] Terminal state is restored on normal exit and handled failure paths; no persistent listener/provider/helper leak in tested shutdown cases.
- [ ] Existing host checks and new targeted tests pass; host-only dependencies do not enter the Kindle binary's dependency graph. No protocol or device changes are needed.
- [ ] README documents launch mode, controls, legacy mode and logging. Report actual commands/results, remaining limitations and any unperformed device tests. Do not mark physical-device behavior as verified without evidence.

## References

- https://github.com/num13ru/rust_x11_hello
- https://github.com/num13ru/rust_x11_hello/blob/master/tools/paperspoon/src/main.rs
- https://github.com/num13ru/rust_x11_hello/blob/master/tools/paperspoon/src/diagnostic.rs
- https://ratatui.rs/how-to/develop-apps/terminal-and-event-handler/
- https://docs.rs/ratatui/latest/ratatui/init/index.html

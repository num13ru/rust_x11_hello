# Plan: Turn `rust_x11_hello` into a Kindle touch-input prototype

## Goal

Extend the existing Kindle X11 Rust prototype so it can detect touchscreen taps through X11 and report touch coordinates reliably.

This phase is only about proving the input path on the Kindle. Do not add networking, macOS integration, configurable layouts, or Stream Deck behavior yet.

Repository:
`https://github.com/num13ru/rust_x11_hello`

## Current state

The project already:

- builds as a static ARMv7 musl Rust binary;
- runs on the Kindle through KUAL;
- connects to the Kindle X server using `x11rb`;
- uses `x11rb::rust_connection::RustConnection`;
- creates an `override_redirect` X11 window;
- draws rectangles and text;
- exits automatically after five seconds.

Current dependency:

```toml
x11rb = "0.13"
```

Keep `x11rb` and keep using `RustConnection`.

Do not switch to Xlib, libxcb, GTK, SDL, Qt, winit, or another GUI framework.

## Assumptions

1. The Kindle touchscreen may already be translated by the Kindle X server into ordinary X11 pointer events.
2. If so, `ButtonPress` / `ButtonRelease` should expose touch coordinates through `event_x` and `event_y`.
3. Core X11 events should be tested before enabling the XInput extension.
4. Existing ARMv7 musl cross-compilation must continue working.
5. The real Kindle is the authoritative environment; success on desktop X11 alone is insufficient.

## Phase 1: Replace the temporary lifetime with an event loop

Remove the current:

```rust
thread::sleep(Duration::from_secs(5));
```

behavior.

After the window has been created and mapped, enter a persistent X11 event loop using:

```rust
conn.wait_for_event()
```

The program should keep running until explicitly terminated.

Handle at minimum:

- `Expose`
- `ConfigureNotify`
- `ButtonPress`
- `ButtonRelease`
- X11 errors where applicable

Do not introduce async Rust for this phase.

## Phase 2: Subscribe to pointer events

Extend the window event mask.

Currently it subscribes to:

```rust
EventMask::EXPOSURE | EventMask::STRUCTURE_NOTIFY
```

Add:

```rust
EventMask::BUTTON_PRESS
EventMask::BUTTON_RELEASE
EventMask::POINTER_MOTION
```

`POINTER_MOTION` is useful diagnostically but should not produce excessive normal logging.

Do not enable the `xinput` crate feature yet.

## Phase 3: Log touchscreen events

For `ButtonPress` and `ButtonRelease`, log useful diagnostic information including:

- event type;
- button/detail value;
- `event_x`;
- `event_y`;
- root coordinates if available;
- timestamp if available;
- window ID.

Example conceptual output:

```text
ButtonPress button=1 x=412 y=683
ButtonRelease button=1 x=412 y=683
```

The exact log format is not important, but it should make real-device testing easy.

Do not interpret the events as application actions yet.

## Phase 4: Make rendering event-driven

Move drawing into a reusable rendering function.

Redraw when:

- an `Expose` event requires it;
- window geometry changes when necessary.

Do not repeatedly repaint in a timed loop.

The e-ink display should remain static when nothing changes.

Call `flush()` only when required after drawing or state-changing requests.

## Phase 5: Use actual screen geometry

Remove the fixed application geometry where practical:

```rust
width = 760
height = 360
x = 80
y = 120
```

Use the X11 screen dimensions already available through:

```rust
screen.width_in_pixels
screen.height_in_pixels
```

For this prototype, create a near-fullscreen or fullscreen control surface suitable for touchscreen testing.

Preserve `override_redirect` unless testing shows that the Kindle window manager requires another approach.

Keep screen geometry logic isolated so it can later support different Kindle models.

## Phase 6: Add a simple hit-test proof

Once touch events are confirmed, divide the window into a simple grid, for example 2×3.

Represent buttons as logical rectangles independent from X11 event structures.

Example conceptual model:

```text
┌───────────────┬───────────────┬───────────────┐
│       1       │       2       │       3       │
├───────────────┼───────────────┼───────────────┤
│       4       │       5       │       6       │
└───────────────┴───────────────┴───────────────┘
```

When a button is released, log:

```text
button=4
```

Do not send anything to the Mac yet.

Prefer activation on `ButtonRelease`, not `ButtonPress`.

If possible, remember where the press started and only activate the button if the release is still inside the same logical button. This allows a user to cancel an accidental press by moving the finger away.

## Phase 7: Separate X11 from UI concepts

Do not keep all logic in `main.rs`.

Introduce a small structure approximately like:

```text
src/
├── main.rs
├── x11/
│   ├── mod.rs
│   ├── display.rs
│   └── events.rs
└── ui/
    ├── mod.rs
    ├── geometry.rs
    └── button.rs
```

This is a guideline rather than a mandatory exact structure.

The important boundary is:

```text
X11 events
    ↓
generic pointer/touch event
    ↓
UI hit testing
```

UI code should not need to know about `x11rb::protocol::Event`.

This will make it possible to replace or supplement the X11 input backend later if Kindle touch events must be read from `/dev/input/event*`.

## Phase 8: Preserve the fallback path

If core X11 `ButtonPress` / `ButtonRelease` events do not arrive on the Kindle:

1. Do not immediately rewrite the application.
2. Record what events are received.
3. Check whether the XInput extension is available.
4. Investigate enabling only the `xinput` feature in `x11rb`.
5. If XInput also does not expose touchscreen events, investigate the Kindle evdev devices under `/dev/input/event*`.

The rendering layer should remain on `x11rb` even if input eventually comes from evdev.

Do not implement the evdev fallback unless core X11 input has actually been shown not to work or the repository contains enough device evidence to justify it.

## Phase 9: Send semantic activations over USBNetwork

Once the logical button grid, release-based activation, and semantic action ids
are proven on the physical Kindle, add the first transport: send each
activation to the macOS companion over the USB network.

Keep the current file layout (the input/UI boundary from Phase 7). The new code
is a small `net` layer:

```text
src/
├── main.rs
├── net/
│   └── mod.rs        one-shot TCP sender
├── proto/
│   └── mod.rs        wire format (`event action=<id>;`)
├── ui/
└── x11/
```

Protocol:

```text
event action=<semantic-id>;
```

One newline-terminated line per activation, from the Kindle to the companion
listener on `192.168.15.201:5581` (the USBNetwork static host). The Kindle
connects, writes the line, and disconnects; a client that is down costs at most
the short connect timeout and is logged (`transport error: ...`) without
breaking the X11 event loop. No listening socket opens on the Kindle, and no
device, button, or coordinate state leaves the device -- only the action id.

Conditions:

- The Kindle runs USBNetwork (static: device at `192.168.15.200`, host at
  `192.168.15.201`) instead of MTP over the same USB link.
- The Mac brings up the USB network interface at `192.168.15.201` and runs the
  companion listener (`tools/companion_listen.py`), which prints and validates
  each received action id.
- Deploy the binary over MTP first, then switch the device to USBNetwork for
  the run.

Acceptance:

1. Each on-device activation produces a matching `event action=<id>;` line at
   the companion listener.
2. A stopped or unreachable companion never breaks the on-device event loop;
   activation logging continues.
3. The transport is std-only TCP (no new dependencies).
4. Physical-device logs and companion output together count as the transport
   proof; host-only testing does not.

Do not add retries, queues, acknowledgements, heartbeats, or a bidirectional
protocol in this phase. The next milestone (Phase 10) can add a persistent
connection and Mac-to-Kindle control once the single-shot direction is proven.

## Dependency policy

Keep dependencies minimal.

Preferred:

```text
anyhow
x11rb
std
```

Do not add:

- Tokio;
- async runtimes;
- GUI toolkits;
- HTTP libraries;
- serialization frameworks;
- logging frameworks;

unless they become justified by a later phase.

For the current diagnostic prototype, `println!` / `eprintln!` are sufficient.

Do not upgrade `x11rb` from `0.13` to `0.14` as part of the same change unless necessary.

The existing `0.13` version is already proven to work on the Kindle, so changing input handling and upgrading the library simultaneously would make failures harder to diagnose.

## Build requirements

The existing ARMv7 musl build must remain functional.

Verify the existing build path used by the repository.

At minimum run all checks available on the development machine, such as:

```text
cargo fmt --check
cargo check
cargo clippy
cargo test
```

Run the ARMv7 musl build used by the project if the target/toolchain is available.

Do not claim Kindle touchscreen functionality has been verified unless it was actually tested on the physical Kindle.

Desktop X11 testing may verify event-loop correctness but does not prove Kindle touchscreen compatibility.

## Error handling

Handle at least these cases cleanly:

- cannot connect to the X server;
- cannot create the window;
- cannot create the graphics context;
- event loop terminates because the X connection fails;
- invalid or unexpected geometry;
- pointer event outside the expected window/grid area.

Avoid panics for ordinary runtime failures.

## Non-goals

Do not implement yet:

- communication with macOS;
- USBNetwork;
- Wi-Fi transport;
- Hammerspoon integration;
- WebSockets;
- HTTP;
- configurable actions;
- application-specific profiles;
- icons;
- custom fonts;
- animations;
- e-ink refresh optimization;
- Mac → Kindle state synchronization.

Those belong to later milestones.

## Acceptance criteria

The change is complete when:

1. The program launches from the existing Kindle/KUAL workflow.
2. The X11 window remains open until terminated.
3. The UI is redrawn correctly after `Expose`.
4. Touching the Kindle produces logged pointer/touch coordinates if core X11 input is supported.
5. Pressing one of the test grid areas logs its logical button ID.
6. Touch input logic is separated from button hit-testing logic.
7. Existing static ARMv7 musl compilation still succeeds.
8. No unnecessary runtime or native GUI dependency is introduced.
9. README documents how to launch the test and what output to look for.
10. The final report explicitly states whether real Kindle touch input was actually verified or remains unverified.

## Suggested commits

Prefer small commits that keep individual experiments easy to bisect:

```text
feat: add persistent X11 event loop
feat: capture X11 pointer events
feat: add touch hit testing
refactor: separate X11 and UI layers
docs: add Kindle touch testing instructions
```

Do not combine unrelated cleanup or dependency upgrades with the touch-input experiment.

## Follow-up milestone

After this milestone proves:

```text
Kindle touch
    ↓
coordinates
    ↓
logical button
```

the next milestone will be:

```text
logical button
    ↓
small semantic protocol
    ↓
TCP over Wi-Fi or USBNetwork
    ↓
macOS companion
```

The Kindle should eventually send semantic action IDs such as:

```text
media.play_pause
terminal.new_window
tmux.work
zoom.toggle_mute
```

rather than hard-coded keyboard shortcuts.

That future architecture should not influence the scope of the current touch-input milestone beyond keeping the UI/input boundary clean.

## Phase 10 - Persistent connection and Mac-to-Kindle display commands

Once the single-shot direction (Phase 9) is proven on the physical Kindle, add
the first bidirectional step: a **persistent TCP connection** from the Kindle
to the companion, over which the companion can send **display commands** back
to the Kindle.

### Scope

Do not add retries, queues, heartbeats, acknowledgements, or a bidirectional
protocol in this phase beyond what is needed to prove the control direction.
The next milestone can add richer Mac-to-Kindle control once this direction is
proven.

### Protocol

One newline-terminated text line per message, in both directions:

```text
Kindle -> companion: event action=<semantic-id>;
companion -> Kindle: display <text>
```

`display <text>` renders the text on the Kindle's X11 window (in a fixed
status area), proving Mac-to-Kindle control. Companion sends only display
commands in this phase.

### Transport

- Kindle opens one TCP connection to the companion address on launch
  (same `RUST_X11_HELLO_COMPANION` override; default USBNetwork static host).
- The connection stays open for the run; each activation writes one
  `event action=<id>;` line over it.
- The companion reads with a short nonblocking poll so the X11 event loop is
  never blocked for long; a disconnected companion (or one that never sends)
  must not stall the X11 event loop or drain performance.
- If the connection drops, the Kindle logs it and keeps running; the next
  activation retries the connect. No queueing of lost messages.

### Kindle-side rendering of display commands

- `display <text>` sets a status line drawn in the window (below the button
  grid), visible to the operator, and logs the received command.
- The status area is a bounded region; long text truncates or wraps safely.
- The rendering layer stays in `x11/events.rs`/`x11/display.rs`; the network
  layer never calls X11 directly.

### Realm of the persistent socket

- The Kindle still opens no listening socket; it connects out. The companion
  keeps listening on `0.0.0.0:5581`.
- The device sends only `event action=<id>;`. Only the companion's display
  command enters the Kindle, and only into the status area.

### Acceptance

1. A persistent connection is established at launch and remains for the run.
2. Each activation produces an `event action=<id>;` line on the connection
   (regression of Phase 9).
3. A `display hello` line sent from the companion renders on the Kindle
   window and is logged as `display: hello`.
4. The X11 event loop keeps processing presses while the connection is idle
   and while a display command arrives.
5. A dropped connection logs `transport` state and never breaks the loop.
6. Static ARM build and host checks still pass.

### Suggested commits

```text
feat: persistent companion session with display command
test: add Kindle phase 10 display-command evidence
```
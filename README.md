# PaperPad

PaperPad turns a Kindle into a Wi-Fi-connected remote display and touch surface
for a macOS companion called PaperSpoon.

- PaperSpoon owns the application UI, layout, hit testing, and Mac actions.
- PaperPad displays negotiated Mono1 or Gray8 frames and forwards
  viewport-relative touch events.

## Architecture

```text
macOS                                      Kindle

PaperSpoon -- PPFB v2 framebuffer ------> PaperPad -- X11 or MXCFB
           <----- pointer events ---------          -- local Exit
```

PaperSpoon renders only the negotiated remote viewport. On the tested
1272×1696 portrait screen, that viewport is 1272×1624; PaperPad reserves the
bottom 72 pixels for its local system UI.

PPFB v2 uses typed binary messages for capability negotiation, viewport
changes, pointer phases, and frames. Mono1 is row-major and MSB-first (`0` is
white); Gray8 uses one byte per pixel (`0` is black and `255` is white).
Payloads are capped at 16 MiB and validated before becoming authoritative.

PaperPad retains only a successfully displayed frame. Invalid dimensions,
unsupported formats, decode errors, or failed display updates do not replace
it. A matching reconnect receives PaperSpoon's last authoritative frame;
pointer events that occur while disconnected are not replayed.

## Build and run

### 1. Verify and build the Kindle package

From the repository root:

```sh
make check
make build
make verify
git diff --check
```

`make check` needs Rust, Bash, `jq`, and local loopback socket access. The ARM
build and static verification use Docker. The resulting untracked binary is:

```text
kindle-extension/rust_x11_hello/bin/rust_x11_hello
```

### 2. Start PaperSpoon on the Mac

```sh
cargo build --release --package paperspoon
./target/release/paperspoon 5581 /tmp/paperspoon.log
```

PaperSpoon listens on TCP 5581 and answers discovery on UDP 5580. Passing TCP
port `0` selects and advertises an OS-assigned port.

### 3. Install or update the Kindle extension

For a fresh install, after confirming the canonical extension does not exist:

```sh
scripts/deploy-kindle-mtp.sh install
```

For an update, first stop PaperPad with its in-window **Exit** button or let the
watchdog finish, confirm the window is gone, then run:

```sh
scripts/deploy-kindle-mtp.sh update --confirm-stopped
```

MTP cannot inspect running processes; `--confirm-stopped` is the operator's
assertion. The update keeps the previous verified binary as
`bin/rust_x11_hello.previous` and refuses to overwrite an existing backup. To
clear that backup explicitly before a later retry:

```sh
mtp-rs rm /extensions/rust_x11_hello/bin/rust_x11_hello.previous --yes
```

### 4. Run from KUAL

KUAL provides three actions:

- **Run Paperpad (90s)** — reference X11 display backend.
- **Run Paperpad MXCFB (90s, experimental)** — direct framebuffer display with
  X11 still providing touch and lifecycle events.
- **Inspect framebuffer metadata** — read-only diagnostics; it does not draw or
  submit an e-ink refresh.

The full-screen window covers KUAL. Stop it with PaperPad's **Exit** button or
wait for the watchdog. The launcher serializes runs and escalates from `TERM`
to `KILL` only after revalidating the recorded PaperPad process.

## Connection configuration

PaperPad normally discovers PaperSpoon automatically:

```text
Kindle UDP 5582 -- DISCOVER --> broadcast UDP 5580
Kindle UDP 5582 <-- HERE ----- PaperSpoon
Kindle          -- PPFB v2 --> advertised TCP port
```

Discovery accepts exactly one distinct responder, uses three bounded 500 ms
probe windows, and retries the full connection path after two seconds. The
launcher installs and removes a narrow temporary firewall rule for the UDP
reply; it does not flush unrelated firewall state.

Use an explicit host for debugging or networks that block broadcast:

```sh
PAPERPAD_COMPANION=192.168.0.12
PAPERPAD_COMPANION_PORT=5581
```

`PAPERPAD_COMPANION_PORT` is valid only with an explicit host and must be in
`1..=65535`. Invalid configuration fails before the X11 window opens. There is
no hard-coded IP fallback.

Other runtime overrides are:

| Variable | Meaning |
| --- | --- |
| `PAPERPAD_DISPLAY_BACKEND=x11` | Default X11 display path |
| `PAPERPAD_DISPLAY_BACKEND=mxcfb` | Experimental direct framebuffer path |
| `PAPERPAD_EXT_DIR` | KUAL extension directory |
| `PAPERPAD_WATCHDOG_SECONDS` | Maximum run duration |
| `PAPERPAD_WATCHDOG_TERM_GRACE_SECONDS` | Grace period before forced stop |

Legacy environment variable names are not read.

## Display backends

| Backend | Mono1 | Gray8 and JPEG | Notes |
| --- | --- | --- | --- |
| X11 | Yes | No | Reference display path |
| MXCFB | Yes | Yes | Experimental; uses X11 for input |

MXCFB startup requires the tested framebuffer layout and HWTCON capabilities;
it fails rather than silently falling back to X11. Both formats currently use
the GC16 update path. Direct framebuffer output, X11 touch, local Exit, Gray8,
JPEG, and Apple Music Now Playing have been observed on the documented
Paperwhite 6. Update-completion timing, exact pixel fidelity, sleep/wake
interaction, and every failure fallback have not all been physically verified.

See [MXCFB device evidence](docs/archive/mxcfb-device-evidence.md) for recorded
claims and [the manual validation sequence](docs/archive/mxcfb-manual-validation.md)
for repeatable device checks.

## PaperSpoon frame commands

Commands are entered on PaperSpoon's stdin. Explicit diagnostic dimensions
must match the active remote viewport.

```text
frame corners 1272x1624
frame border 1272x1624
frame checkerboard 1272x1624
frame horizontal 1272x1624
frame black 1272x1624
frame white 1272x1624

frame gray-gradient 1272x1624
frame gray-bars 1272x1624
frame gray 128 1272x1624

frame jpeg /absolute/or/relative/path.jpg
frame nowplaying
ui 1272x1624
```

Gray8, JPEG, and Now Playing require a connected backend that advertises
Gray8. JPEGs are decoded on the host, fitted without cropping, centered, and
white-letterboxed. Relative paths use PaperSpoon's working directory; spaces
are allowed because everything after `frame jpeg` is treated as the path.

On macOS, `frame nowplaying` takes a one-shot system Now Playing snapshot using
the exact crates.io `media-remote` 0.5.2 `NowPlayingPerl` backend. It works with
SIP enabled and does not access `MediaRemote.framework` directly. Missing
artwork uses a placeholder; absent optional metadata uses an `Unknown` label.
A missing player, timeout, adapter failure, Mono1 connection, or viewport race
leaves the previous frame unchanged. PaperSpoon terminates and reaps the helper
and Perl adapter after each invocation.

Successful JPEG and Now Playing results retain their rendered pixels for a
matching reconnect instead of reopening the file or reacquiring system state.
See [the Now Playing design and validation record](docs/frame-nowplaying-via-MediaRemote.md)
for acquisition limits, lifecycle details, tests, and the exact host/device
evidence.

## Touch and Mac actions

PaperPad forwards only primary core-X11 contacts (`detail=1`) inside the remote
viewport. PaperSpoon activates a button only when a matching release remains
inside the button that was pressed. Gaps, unmatched releases, viewport changes,
and releases elsewhere cancel or do nothing. The local **Exit** control never
sends an application action across the protocol.

| Button | PaperSpoon action |
| --- | --- |
| 1 | `media.play_pause` |
| 2 | `media.next` |
| 3 | `media.previous` |
| 4 | `terminal.new_window` |
| 5 | `tmux.work` |
| 6 | `zoom.toggle_mute` |
| 7–9 | Reserved stub actions |

By default, PaperSpoon dispatches resolved actions with:

```text
open -g hammerspoon://paperpad?action=<id>
```

Use `--no-forward-url` to disable forwarding. A sample Hammerspoon handler is
available at `tools/hammerspoon/init.example.lua`. Dispatch success does not
prove that the target Mac application accepted the action.

## Logs and device evidence

Retrieve the device log after PaperPad stops:

```sh
mtp-rs get /extensions/rust_x11_hello/rust_x11_hello.log \
  rust_x11_hello.device.log --replace
```

The log grows across runs. Delete it before a clean evidence run if necessary;
the launcher recreates it:

```sh
mtp-rs rm /extensions/rust_x11_hello/rust_x11_hello.log --yes
```

Match the deployed binary's SHA-256 with the host artifact before trusting a
run. A successful host build or accepted framebuffer ioctl is not evidence of
visible panel output. Likewise, missing `ButtonPress`/`ButtonRelease` records
after checksum, event-mask, window, and geometry checks means touch remains
unverified on that configuration; it does not mean the Rust build failed.

## Repository map

| Path | Responsibility |
| --- | --- |
| `src/` | PaperPad lifecycle, X11 input, networking, and display backends |
| `crates/paper-protocol/` | Transport-independent PPFB types and codecs |
| `tools/paperspoon/` | Host rendering, input resolution, discovery, and actions |
| `kindle-extension/` | KUAL package and launch scripts |
| `scripts/deploy-kindle-mtp.sh` | Verified MTP install/update workflow |
| `docs/` | Feature designs and physical-validation records |

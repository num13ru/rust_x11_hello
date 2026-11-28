# PaperPad

PaperPad turns a Kindle into a Wi-Fi-connected remote display and touch surface
for a macOS companion called PaperSpoon.

- PaperSpoon owns the application UI, layout, hit testing, and Mac actions.
- PaperPad displays negotiated Mono1 or Gray8 frames and forwards
  viewport-relative touch events.

## Architecture

```text
macOS                                      Kindle

PaperSpoon -- PPFB framebuffer ------> PaperPad -- MXCFB
           <----- pointer events ---------          -- local Exit
```

PaperSpoon renders only the negotiated remote viewport. On the tested
1272×1696 portrait screen, that viewport is 1272×1624; PaperPad reserves the
bottom 72 pixels for its local system UI. An outlined circle at the left of
the **Exit** button means PaperSpoon is disconnected; the circle is filled
solid black while the PPFB transport session is connected. The indicator is
device-local and is not a touch target.

PPFB v2 uses typed binary messages for capability negotiation, viewport
changes, pointer phases, and frames. Mono1 is row-major and MSB-first (`0` is
white); Gray8 uses one byte per pixel (`0` is black and `255` is white).
Payloads are capped at 16 MiB and validated before becoming authoritative.
Compatible peers negotiate PPFB v3 by default for independent Zstandard
level-1 frames with Raw fallback. Automatic mode retries a v2-only host
on a fresh connection. See the [PPFB v3 setup and wire format](docs/ppfb-v3.md).

PaperSpoon embeds pinned IBM Plex Mono and Inter font assets, so host UI
rendering does not depend on installed macOS fonts or network access. Mono1
controls use thresholded IBM Plex Mono. Gray8 Now Playing uses binary
text alongside grayscale artwork. PaperPad's device-local Exit label is
a baked one-bit IBM Plex Mono bitmap. The Kindle binary does not parse or
bundle OpenType fonts.
Pixel format does not select the Kindle update waveform.

PaperPad retains only a successfully displayed frame. Invalid dimensions,
unsupported formats, decode errors, or failed display updates do not replace
it. A matching reconnect receives PaperSpoon's last authoritative frame;
pointer events that occur while disconnected are not replayed. Disconnecting
does not clear the retained application frame; only the local connection
indicator changes state.

The normal PaperPad runtime is event-driven: Linux `poll(2)` blocks on native
touch input and a coalescing `eventfd` for transport or shutdown work, using a
timeout only while a real display-maintenance deadline is pending.

## Build and run

### 1. Verify and build the Kindle package

#### Native Nix build (default)

On Apple Silicon macOS, install Nix with `nix-command` and `flakes` enabled.
The first activation and an uncached Cargo build need network access. Run these
commands from the repository root.

```sh
nix develop -c make check
nix develop -c make build
nix develop -c make verify
```

The shell supplies Rust 1.97.1, the ARMv7 musl target, and LLVM ELF tools. For an
interactive session, run `nix develop` once and use the same `make` commands.
Host workspace tests on macOS require loopback networking.

#### Optional Mr Boxington cache

To use the same toolchain with Mr Boxington's Cargo wrapper, run:

```sh
nix develop .#mbx -c make build
```

See the [build guide](docs/containerless-kindle-build.md) for cache inspection,
troubleshooting, and validation results.

#### Docker and Apple Container builds

```sh
make build BUILD_BACKEND=container CONTAINER_CLI=docker
make verify BUILD_BACKEND=container CONTAINER_CLI=docker
```

On Apple Silicon with [Apple Container](https://github.com/apple/container) installed:

```sh
container system start
make build BUILD_BACKEND=container CONTAINER_CLI=container
make verify BUILD_BACKEND=container CONTAINER_CLI=container
```

Set `BUILD_BACKEND=container` and your preferred `CONTAINER_CLI` in the ignored
`.env` file to persist the selection. Command-line assignments take precedence.
Docker is the default CLI when you select the container backend.

`make build` creates the builder image when it is missing or `Dockerfile` has
changed. `make image` and `make shell` operate on the selected container image
even when `BUILD_BACKEND=native`. Cargo dependencies persist in the ignored,
backend-specific `.cache/cargo/` directory. `make clean-cache` removes those
caches. `make clean-target` removes the repository's `target/`. `make clean`
also removes the packaged binary and `logs/`.

Apple Container runs the builder natively as AArch64 while Rust still targets
the Kindle's `armv7-unknown-linux-musleabihf`. Both backends limit build and
verification containers to 8 CPUs and 2 GiB by default; override or clear
`CONTAINER_RUN_ARGS` if the host has fewer resources or different limits. The
Apple Container path requires macOS 26 on Apple silicon, a running Container
system service, host network access to Debian and crates.io, and a CLI version
that provides both `build` and `run`. Docker remains the portable fallback.
For guest connectivity failures caused by a host VPN, see
[Apple Container network issues](#apple-container-network-issues).

#### Verified artifact

Both backends verify the newly built ARMv7 hard-float static ELF before
publishing it. `make verify` rechecks the published artifact with the selected
backend. The shared build script uses the executable path reported by Cargo,
including when Cargo uses a custom target directory.

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

For an update, run:

```sh
scripts/deploy-kindle-mtp.sh update
```

When practical, stop PaperPad with its device-local **Exit** button or let the
watchdog finish before updating; MTP cannot inspect running processes. The
update keeps the previous verified binary as
`bin/rust_x11_hello.previous` and refuses to overwrite an existing backup. To
clear that backup explicitly before a later retry:

```sh
mtp-rs rm /extensions/rust_x11_hello/bin/rust_x11_hello.previous --yes
```

### 4. Run from KUAL

Open the **PaperPad** group in KUAL for all PaperPad actions.

- **Run Paperpad DU (5m)** uses DU for binary pixel transitions and GC16
  for grayscale. **Run Paperpad GC16 (5m)** uses GC16 for all remote
  updates. The experimental **A2** and **DU4** runs use those waveforms
  where pixel levels qualify and fall back to GC16 elsewhere. All runs
  use direct framebuffer output, exclusive evdev input, and a five-minute
  watchdog. The local system strip, startup clearing, and framebuffer
  restoration use GC16.
- **Run Paperpad DU with PPFB v2 (5m)** forces legacy Raw transport.
- **Test MXCFB TERM cleanup (15s)** — normal MXCFB runtime with a short
  watchdog for verifying graceful `TERM`, framebuffer restoration, and clean
  process exit; it is not a benchmark.
- **Benchmark MXCFB GC16/DU/A2/DU4** runs explicit 12-frame local
  completion benchmarks with a one-minute watchdog. They use native evdev for
  local Exit, derive geometry from `/dev/fb0`, and open no X11 or PaperSpoon
  connection; DU, A2, and DU4 are experimental fidelity/ghosting tradeoffs.
- **Benchmark A2 512px/256px/64px** — centered dirty-region completion tests
  for measuring how update area affects the fastest full-screen waveform.
- **Inspect framebuffer metadata** — read-only diagnostics; it does not draw or
  submit an e-ink refresh.
- **Inspect input devices** — read-only evdev capability and absolute-axis
  metadata; it does not consume events or request an exclusive grab.
- **Trace input events (15s; tap)** — bounded raw evdev record capture for
  controlled touch diagnostics; it does not request an exclusive grab.

PaperPad's direct framebuffer view covers KUAL. Stop it with the **Exit** button
or wait for the watchdog. The launcher serializes runs and escalates from `TERM`
to `KILL` only after revalidating the recorded PaperPad process.

## Connection configuration

PaperPad normally discovers PaperSpoon automatically:

```text
Kindle UDP 5582 -- DISCOVER --> broadcast UDP 5580
Kindle UDP 5582 <-- HERE ----- PaperSpoon
Kindle          -- PPFB    --> advertised TCP port
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
`1..=65535`. Invalid configuration fails before display and input
initialization. There is no hard-coded IP fallback.

Other runtime overrides are:

| Variable | Meaning |
| --- | --- |
| `PAPERPAD_MXCFB_WAVEFORM=gc16\|du\|a2\|du4` | Remote update policy. Defaults to `du` with GC16 for grayscale |
| `PAPERPAD_MXCFB_BENCHMARK_WAVEFORM=gc16\|du\|a2\|du4` | Explicit benchmark waveform; defaults to GC16 |
| `PAPERPAD_MXCFB_BENCHMARK_FRAMES=2..120` | Completed benchmark samples; defaults to 8 |
| `PAPERPAD_MXCFB_BENCHMARK_REGION=full\|WIDTHxHEIGHT` | Centered benchmark region inside the remote viewport; defaults to full |
| `PAPERPAD_TRANSPORT=auto\|v2\|v3-raw\|v3-auto\|v3-zstd` | Defaults to `auto` with v2 compatibility |
| `PAPERPAD_EXT_DIR` | KUAL extension directory |
| `PAPERPAD_WATCHDOG_SECONDS` | Maximum run duration |
| `PAPERPAD_WATCHDOG_TERM_GRACE_SECONDS` | Grace period before forced stop; defaults to 15 seconds |

Legacy environment variable names are not read.

## Display runtime

| Backend | Mono1 | Gray8 and JPEG | Notes |
| --- | --- | --- | --- |
| Native MXCFB | Yes | Yes | Exclusive evdev touch and no X11 connection |

PaperPad has no X11 source modules or client dependencies. The historical
`rust_x11_hello` package and extension names remain for deployment
compatibility; they are not runtime dependencies.

MXCFB startup requires the tested framebuffer layout and HWTCON capabilities.
PaperPad compares both frames' pixel values and can submit separate waveform
updates within one viewport. The native runtime derives fixed geometry from `/dev/fb0`,
uses exclusive evdev input, and opens no X11 connection. Direct framebuffer
output, native touch, local Exit, Gray8, JPEG, and Apple Music Now Playing have
been observed on the documented Paperwhite 6. Subsequent
device runs completed marker-specific waits and visibly restored the original
screen after every benchmark. Exact pixel fidelity, sleep/wake interaction,
and every failure fallback have not all been physically verified.

See [MXCFB device evidence](docs/archive/mxcfb-device-evidence.md) for recorded
claims and [the manual validation sequence](docs/archive/mxcfb-manual-validation.md)
for repeatable device checks.

### MXCFB completion benchmark

The four opt-in KUAL actions measure serialized, full-viewport update
completion. On the tested Paperwhite 6, none approached the 60 updates/s
software budget; A2 was the fastest at about 9 updates/s. These are
kernel-timed completion results, not camera-verified visual FPS.

See [MXCFB benchmarks](docs/archive/mxcfb-benchmarks.md) for the current results,
methodology, commands, operating conditions, cleanup behavior, and waveform
limitations.

## PaperView desktop preview

With PaperSpoon already listening, launch the read-only desktop client with the
same viewport dimensions used by its frame commands:

```sh
cargo run -p paperview -- --host 127.0.0.1 --port 5581 --viewport 1272x1624
```

PaperView starts in aspect-preserving Fit mode. Press `F` for Fit, `1`, `2`, or
`4` for exact physical-pixel integer zoom, the arrow keys to pan by 32 source
pixels, and `Home` to recenter. The title reports the active zoom and source
pan. Panning is bounded so every source edge remains reachable without
silently wrapping. PaperView sends no pointer actions by default.

Add `--interactive` to opt into primary-button PPFB pointer input. Left-button
Down/Up coordinates are mapped through the exact Fit or integer-zoom transform;
letterbox, secondary-button, and disconnected clicks are ignored. Keyboard
zoom/pan remains the preview control, while a left drag represents remote touch.
Run PaperSpoon with `--no-forward-url` when exercising application buttons.

Add `--save-dir DIR` to enable `S`, which exports the current validated source
frame as an 8-bit grayscale PNG. PaperView creates a deterministic filename
from the frame ID, format, and dimensions and refuses to overwrite an existing
file. Fit, zoom, pan, and operating-system scaling never affect the export.

PaperSpoon serves one current PPFB client; starting PaperView may replace a
connected PaperPad session. The native window and frame rendering have been
exercised on macOS Apple Silicon; the keyboard bindings and viewport model are
covered by automated tests. Linux X11/Wayland and Windows remain unverified.

See the [PaperView local development workflow](docs/paperview-development-workflow.md)
for a safe two-terminal Gray8 and Now Playing inspection sequence.

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
See [the Now Playing design and validation record](docs/archive/frame-nowplaying-via-MediaRemote.md)
for acquisition limits, lifecycle details, tests, and the exact host/device
evidence.

### Live Now Playing

Start PaperSpoon with `--now-playing-fake` for development or
`--now-playing-apple-music` on macOS for Apple Music. With a Gray8 client
connected, enter `nowplaying start`. Tap **Prev**, **Play/Pause**, **Next**,
**Shuffle**, **Repeat**, or the progress bar. Tap the cover to show Apple
Music on the Mac and reveal the current track when available. Enter `nowplaying stop` to
return to the application grid. On Kindle, use **Run Paperpad DU (5m)**
for binary controls and GC16 artwork.

Apple Music must be running, and macOS must allow the launching application
to control Music under **Privacy & Security → Automation**. See the
[live Now Playing workflow](docs/paperview-development-workflow.md#live-now-playing)
for commands, refresh settings, and testing conditions.

## Touch and Mac actions

PaperPad forwards only primary native evdev contacts (`detail=1`) inside the remote
viewport. PaperSpoon fills a grid button while its contact is armed and restores
it before dispatching the action. It activates a button only when a matching
release remains inside the button that was pressed. Gaps, unmatched releases,
viewport changes, and releases elsewhere cancel or do nothing. The local
**Exit** control never sends an application action across the protocol.

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

## Latency traces

Normal KUAL runs create a fresh
`/extensions/rust_x11_hello/paperpad.latency.jsonl`. Start PaperSpoon with a
separate host trace:

```sh
./target/release/paperspoon 5581 /tmp/paperspoon.log \
  --latency-log=/tmp/paperspoon.latency.jsonl
```

Each forwarded pointer carries a run-scoped session ID, trace ID, and PaperPad
monotonic origin. The receiver still accepts legacy untraced pointers, but an
older PaperSpoon cannot decode the traced payload, so deploy PaperPad and run
PaperSpoon from the same build for measurements.

After the run stops, retrieve and analyze the device trace:

```sh
mtp-rs get /extensions/rust_x11_hello/paperpad.latency.jsonl \
  paperpad.latency.jsonl --replace
python3 scripts/analyze-latency.py \
  paperpad.latency.jsonl /tmp/paperspoon.latency.jsonl \
  /tmp/paperpad-hammerspoon-latency.jsonl > latency.csv
```

The Hammerspoon trace is optional and requires installing the updated sample
handler. The analyzer reports PaperPad input/queue time, feedback transport
round trip, a symmetry-based one-way estimate, PaperSpoon processing and
`open` duration, frame presentation return and backend submission duration,
Hammerspoon receipt, handler dispatch duration, and
release/down-to-dispatch-return latency. Frame
presentation return is not camera-verified e-ink completion, and Hammerspoon
dispatch return does not prove that the target application changed state. An
action-specific watcher can call `paperpadRecordObservedEffect(actionId)` after
observing the real state change; the analyzer then reports
release/down-to-effect latency.

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
visible panel output. Native touch requires an `input reader: opened` record and
evdev Press/Release records in the Kindle log. Current normal and benchmark
runs do not open X11; archived ignored X11 button records are not a second
application-input path.

## Apple Container network issues

If the host can reach Debian and crates.io but Apple Container times out, the
host VPN may not forward the guest's `vmnet` traffic. Enable the local workaround
in the ignored `.env` file:

```dotenv
PROXY=true
```

With this opt-in, `make image`, `make build`, and `make shell` query `container
network list --format json`, start a restricted host proxy on the discovered
gateway and an ephemeral port, and inject its URL only into the child build
environment. No address is persisted. The proxy accepts only the discovered
private subnet, permits only public destinations on ports 80 and 443, and stops
with the command. It requires Python 3.

Remove `PROXY` or set it to `false` to use direct guest networking. `PROXY`
accepts only `true` or `false`. Set `APPLE_CONTAINER_NETWORK` only when using a
network other than `default`.

## Repository map

| Path | Responsibility |
| --- | --- |
| `src/` | PaperPad lifecycle, native input, networking, and MXCFB display |
| `crates/paper-protocol/` | Transport-independent PPFB types and codecs |
| `tools/paperspoon/` | Host rendering, input resolution, discovery, and actions |
| `kindle-extension/` | KUAL package and launch scripts |
| `scripts/deploy-kindle-mtp.sh` | Verified MTP install/update workflow |
| `docs/` | Feature designs and physical-validation records |

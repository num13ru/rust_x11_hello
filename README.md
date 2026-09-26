# Paperpad

PaperPad is a bounded Kindle/KUAL remote framebuffer client. It displays the
application UI rendered by the PaperSpoon macOS companion, forwards
viewport-relative core X11 pointer events over Wi-Fi, and owns a separate
device-local Exit control. PaperSpoon handles application hit testing and
semantic actions.

The repository, Cargo package, device binary, environment variables, and
extension path retain the MVP identifier `rust_x11_hello`. The KUAL launcher
serializes launch attempts with an owner-checked lock and stops a run after 90
seconds, with a five-second `TERM` grace followed by `KILL` only after
revalidating the recorded PID and executable.

## Ownership and module boundaries

| Area | Owner |
| --- | --- |
| Process setup and teardown | `src/main.rs` |
| Environment parsing and validation | `src/config.rs` |
| Remote pointer normalization and local Exit decisions | `src/app.rs`, using pure logic from `src/ui/` |
| X11 resources, event translation, and rendering | `src/x11/` |
| PaperPad TCP lifecycle, workers, pointer queue, and frame mailbox | `src/net/` |
| Unique-endpoint discovery policy | `src/discovery.rs` |
| Shared wire constants, framebuffer representation, formatting, and parsing | `crates/paper-protocol/` |
| PaperSpoon listener, application rendering/input, discovery responder, and forwarding | `tools/paperspoon/` |
| KUAL lifecycle and MTP packaging | `kindle-extension/` and `scripts/deploy-kindle-mtp.sh` |

The dependency direction is deliberate: protocol code depends only on
`std`; PaperPad's viewport and Exit logic know nothing about X11 or sockets;
PaperSpoon owns application geometry and contact decisions. The X11 thread
owns the window and `AppState`. Network workers own blocking connection work,
with pointer messages crossing a bounded queue and frames crossing a
single-slot latest-value mailbox.

## Conditions under which this works

- A jailbroken Kindle with KUAL and an X server compatible with `x11rb` core X11 requests.
- An ARMv7/EABI5-compatible Kindle userspace. The produced binary is statically linked with musl; a device that cannot execute ARMv7 binaries needs a different build target.
- KUAL supplies the working `DISPLAY`/`XAUTHORITY` environment used by the existing device launcher.
- `mtp-rs` can access the unlocked Kindle over USB. Its `/extensions` path maps to `/mnt/us/extensions` at runtime.
- The canonical extension is installed as `/extensions/rust_x11_hello`. Do not keep the legacy `/extensions/rust_hello` entry alongside it.
- Physical-device logs are authoritative for touch support. Host compilation or desktop pointer events cannot prove Kindle touchscreen translation.

## Geometry-aware rendering

- The app opens a borderless window at `(0,0)` covering the selected X11 screen. The Paperwhite 6 reports `1272 x 1696` in portrait; runtime dimensions come from X11 rather than a fixed device resolution.
- PaperPad partitions that physical extent into a top remote-content viewport and a fixed 72px PaperPad-owned system strip at the bottom. On the Paperwhite the remote viewport is `(0,0) 1272 x 1624`, and the system strip is `(0,1624) 1272 x 72`. PaperSpoon must render only the remote viewport.
- PaperSpoon's application layout uses square grid cells. On this Kindle, cells are `384 x 384`, with 20px outer margins and 40px gaps between rows and columns. Any division remainder is absorbed by the column gaps so both outer edges stay aligned.
- PaperSpoon draws the title above the grid and reserves a 40px status region within the remote viewport. PaperPad draws Exit in its local bottom strip, spanning `screen width - 40px` with 20px horizontal margins. The status region is not a `display` command target.
- For shorter viewports, PaperSpoon reduces the cell side to fit the grid and status region without stretching cells; sufficiently small viewports have no application buttons. PaperPad's Exit remains available whenever the window is at least 41px wide and 72px tall. Device layout coordinates are capped at X11's signed-coordinate limit.
- The final event in each `Expose` batch redraws PaperPad's local UI and the last successfully uploaded remote frame, if cached.
- A size-changing `ConfigureNotify` updates PaperPad's viewport and Exit geometry, clears its local contact state, and queues `ViewportChanged` for PaperSpoon. Duplicate geometry is ignored; a zero-width/zero-height report is logged without replacing the last valid extent.

Host tests cover the host application layout and input decisions, PaperPad viewport and Exit geometry, smaller and landscape windows, division remainders, gaps, and zero/maximum dimensions. ARM/static checks verify buildability. Full-screen rendering and touch behavior still require a physical Kindle run.

## Logical hit testing

The remote viewport contains PaperSpoon's nine application buttons in a 3×3 grid. The bottom system strip contains PaperPad's separate local Exit control. Both use half-open bounds, so trailing edges and gaps do not activate a control.

X11 pointer events use signed, physical window coordinates. PaperPad maps contacts inside the remote viewport to unsigned viewport-relative coordinates and sends binary `PointerDown`/`PointerUp` messages to PaperSpoon. Physical points in the system strip or outside the window have no remote coordinate. If a remote contact ends outside the viewport, PaperPad sends an out-of-bounds `PointerUp` to cancel it. Exit hit testing remains local, in physical coordinates.

Only core-X11 `detail=1` participates in remote pointer forwarding or local
Exit activation. PaperSpoon arms an application button on a primary down and
resolves an action only when the matching up remains inside that button.
Exit is a PaperPad-local lifecycle control and sends no remote pointer or
semantic action. PaperSpoon maps buttons 1–9 to:

| Button | Semantic action id |
| ------ | ------------------ |
| 1 | `media.play_pause` |
| 2 | `media.next` |
| 3 | `media.previous` |
| 4 | `terminal.new_window` |
| 5 | `tmux.work` |
| 6 | `zoom.toggle_mute` |
| 7 | `stub.button_7` |
| 8 | `stub.button_8` |
| 9 | `stub.button_9` |

Exit has its own PaperPad `SystemUi` geometry and contact state rather than an
application button ID. Its activation is logged as `ui action=activate
system=exit` and closes the window without writing to PaperSpoon.

Buttons 7–9 resolve to placeholder action IDs for future companion bindings.
Rendering and touch behavior remain device-specific and must be rechecked
after changes to geometry, event translation, or the X11 adapter.

These dotted IDs are resolved on the host, not sent over the Kindle wire.
USBNetwork itself is not used: no maintained USBNetwork package targets this
Paperwhite 6, so the transport is Wi-Fi.

Presses in gaps cannot arm an application button. PaperSpoon cancels an armed contact on releases in another button or outside the grid, repeated primary downs, and viewport changes. PaperPad clears its local remote-contact state on geometry changes or window unmapping; these local cancellations do not themselves send a remote `PointerUp`. Unmatched releases do nothing. Auxiliary details such as the observed Kindle `detail=6` and `detail=9` pairs retain their raw diagnostic lines but neither activate nor cancel the armed primary contact. Pointer motion remains unlogged.

## Framebuffer protocol primitives

The shared protocol crate defines a transport-independent `Mono1Frame` for the remote content viewport. It is row-major and MSB-first within each byte: `0` is white and `1` is black. Each row occupies `ceil(width / 8)` bytes with no extra bytes between rows. For widths not divisible by eight, unused low bits in the final byte are required to be white. Frames require nonzero dimensions and an exact `stride * height` payload.

Protocol v2 also defines a 12-byte, big-endian binary header containing `PPFB` magic, version, message type, zero-reserved flags, and a `u32` payload length. Payloads are capped at 16 MiB and rejected from the header before allocation. The borrowing decoder supports partial and consecutive messages without performing I/O.

Typed, big-endian payloads cover `Hello` (version, Mono1 support, remote viewport), `PointerDown`/`PointerUp` (viewport-relative `x/y`), `ViewportChanged` (new remote extent), and `Frame` (`u64` frame ID, extent, Mono1 format, pixels). Reserved payload bytes must be zero. Frame payloads are validated as borrowed bytes without copying; pointer bounds and frame dimensions remain the receiving endpoint's responsibility against its current negotiated viewport.

PaperPad's inbound TCP reader accepts complete `PPFB` v2 `Frame` messages only. Frames are validated against the current remote viewport and coalesced into a latest-frame mailbox. Legacy text, corrupt messages, and unexpected v2 message types end the connection and use the existing reconnect path; dimension mismatches are logged and skipped without replacing a valid pending frame.

The X11 bitmap adapter converts accepted Mono1 frames to the server-advertised XYBitmap representation. It handles 8/16/32-bit scanline units, independent image-byte and bitmap-bit order, scanline padding, and whole-row chunking within the server's maximum request size. Each newly received frame is uploaded with checked depth-1 `PutImage` requests, confined to the remote viewport; the existing GC maps `1` to black and `0` to white. Only a successful upload replaces the cached frame. PaperPad redraws that frame after X11 `Expose` or same-viewport geometry redraws, and invalidates it when the viewport size changes. Unsupported formats, conversion failures, and X11 upload errors are logged without intentionally ending the event loop; they do not replace the cache or touch the local Exit region.

## TCP transport (Wi-Fi)

The Kindle connects to PaperSpoon over TCP and sends a binary v2 `Hello` with
the remote viewport size. While connected, it sends viewport-relative
`PointerDown`/`PointerUp` and size changes as `ViewportChanged`. PaperSpoon
responds with binary `Frame` messages. After `Hello`, it sends the last
successfully sent authoritative frame when its dimensions match; otherwise it
sends the application UI. A viewport change sends a replacement application
frame. A matching diagnostic pattern may therefore reappear after reconnect.

PaperSpoon (a std-only Rust listener, `tools/paperspoon`) logs received
pointer phases and any host-resolved application actions. A dropped connection
starts PaperPad's reconnect path. Pointer events attempted while disconnected
or when its bounded outbound queue is full are not replayed; the next
connection sends a fresh `Hello` and receives an authoritative frame. PaperPad
keeps its local Exit available throughout.

### Zero-config discovery (verified on this Paperwhite 6)

By default PaperPad locates PaperSpoon with a minimal custom UDP
zero-config discovery exchange:

```text
PaperPad binds UDP 0.0.0.0:5582
        |
        | DISCOVER <nonce>  -> 255.255.255.255:5580
        |
PaperSpoon (UDP 5580) replies with a unicast HERE <nonce> <tcp-port>
        |
existing TCP connect
```

- Fixed discovery ports: **UDP 5580** (PaperSpoon listener) and **UDP 5582**
  (PaperPad client). PaperSpoon TCP defaults to **5581**; its discovery reply
  advertises the actual bound TCP port when a different port is selected.
- The wire format is newline-terminated ASCII: `PAPERPAD DISCOVER <nonce>`
  and `PAPERSPOON HERE <nonce> <tcp-port>`. The nonce distinguishes the
  current attempt from stale/unrelated datagrams; the response must echo it.
- The response is **unicast** to the request's source; no broadcast
  responses, no multicast.
- The response payload never contains an IP address; the UDP source address
  is the discovered PaperSpoon address.
- PaperPad listens for the full bounded probe schedule and deduplicates offers
  for the same source-IP/advertised-port endpoint. Exactly one distinct
  endpoint is accepted; two or more are rejected as ambiguous.
- Each discovery attempt is bounded (3 probes, 500 ms window each). After a
  failed startup attempt, PaperPad waits two seconds and retries the whole
  resolution/discovery and TCP connection path in the background.

On the Kindle the firewall INPUT policy is restrictive, so the launcher
installs a narrow temporary ACCEPT rule for the discovery response before
starting PaperPad and removes it on cleanup:

```text
-i wlan0 -p udp --sport 5580 --dport 5582 -j ACCEPT
```

in a dedicated `PAPERPAD_DISCOVERY` iptables chain. The rule exists only for
the bounded run and never flushes unrelated firewall tables.

### Explicit host override

Setting `PAPERPAD_COMPANION` (e.g. to a Wi-Fi run where the Mac is at
`192.168.0.12`) bypasses discovery and connects directly:

```sh
PAPERPAD_COMPANION=192.168.0.12
```

The host value is trimmed. An absent or blank host selects discovery, which
uses the TCP port advertised by PaperSpoon. With an explicit host,
`PAPERPAD_COMPANION_PORT` optionally overrides the default TCP port
5581 and must be a decimal value in `1..=65535`. Empty, zero, malformed, or
out-of-range ports—and a port override without an explicit host—are startup
configuration errors reported before Paperpad creates its X11 window.

This remains the deterministic control path and debugging/recovery override.
When unset, discovery runs and there is **no** fallback to a hard-coded IP.

Verified on the physical Paperwhite 6 (evidence under
`artifacts/kindle-runs/discovery-test-*`): explicit control (Test A), full
discovery chain (Test B), PaperPad restart (Test C), Mac DHCP address change
`192.168.0.12 -> 192.168.0.50` with no configuration edit (Test D), and
bounded failure with the UI alive when PaperSpoon is absent (Test E).

A PaperSpoon that is unreachable costs bounded time per attempt and is logged
on the device; it never breaks the X11 event loop or the on-device input log.
PaperPad retries in the background. Pointer messages attempted while
disconnected fail immediately and are not queued or replayed after connection.
The Kindle opens no listening TCP socket. No application action IDs or text
display commands cross the TCP connection.

### Display backend selection

`PAPERPAD_DISPLAY_BACKEND` selects PaperPad's display backend. It defaults
to `x11`; setting it explicitly to `x11` selects the same reference path.
`mxcfb` is reserved for the direct framebuffer backend and currently fails at
startup with a clear error. PaperPad never silently falls back to X11 after an
explicit MXCFB request.

The KUAL **Inspect framebuffer metadata** action runs `--inspect-framebuffer`
without starting the normal launcher. It collects read-only `/dev/fb0`
information and logs the standard Linux
`FBIOGET_FSCREENINFO` and `FBIOGET_VSCREENINFO` results to
`rust_x11_hello.log`. It also attempts a shared, read-only mapping of the
validated visible framebuffer span, reads its first and last bytes without
logging or interpreting their values, and immediately unmaps it. It then opens
`/dev/fb0` read-write and attempts a shared writable mapping of the same span,
again unmapping immediately without accessing pixel bytes through that mapping.
It then tries the firmware-matched, read-only HWTCON `GET_PANEL_INFO_MTK`
query and discards its returned data. It does not write pixels, map a window,
touch the normal launcher, or submit an e-ink refresh. No PaperSpoon listener
is needed. Retrieve the log
using the normal MTP log command below and look for `mxcfb probe:` lines. If
opening `/dev/fb0`, a query, or either mapping fails, the log records the
specific failure. An accepted writable mapping or panel-info query does not
prove pixel writes, color polarity, e-ink update submission, or panel output.
The HWTCON send-update and wait-complete C layouts are staged from the pinned
PW6 firmware reference and compile-checked, but no update ioctl has been called
or device-verified. This probe also does not prove that X11 input can coexist
with direct display.

Runtime overrides now use the `PAPERPAD_` prefix. The KUAL launcher reads
`PAPERPAD_EXT_DIR`, `PAPERPAD_WATCHDOG_SECONDS`, and
`PAPERPAD_WATCHDOG_TERM_GRACE_SECONDS`. Update existing overrides; legacy
environment names are no longer read.

### Running PaperSpoon

From the repository root, build and run the Rust listener on the Mac:

```sh
cargo build --release --package paperspoon
./target/release/paperspoon 5581 /tmp/paperspoon.log
```

The application UI is sent after PaperPad's `Hello`. To exercise framebuffer
transport and the X11 blitter, type a diagnostic frame command at PaperSpoon's
stdin whose dimensions exactly match PaperPad's current remote viewport. The standard
Paperwhite portrait viewport is `1272x1624`:

```text
frame corners 1272x1624
frame border 1272x1624
frame checkerboard 1272x1624
frame horizontal 1272x1624
frame black 1272x1624
frame white 1272x1624
```

To render and send PaperSpoon's host-owned application UI instead of a
diagnostic pattern, use the same explicit remote viewport dimensions:

```text
ui 1272x1624
```

For a sent application frame, PaperSpoon prints `sent application frame ...`
and PaperPad logs `frame uploaded ... cache=updated`. Repeating `ui` with the
same viewport and unchanged application pixels logs `application frame skipped
unchanged ...` without sending a frame or consuming a frame ID. A viewport
change, a switch back from a diagnostic pattern, or a new `Hello` still sends
an authoritative frame. PaperSpoon performs matching application hit testing
and semantic action dispatch for taps on that frame. While a diagnostic pattern
is authoritative, host application hit testing is inactive. The 72-pixel
local Exit strip remains PaperPad-rendered in either case.

Patterns are generated as validated Mono1 frames and assigned increasing frame
IDs. PaperSpoon prints `sent frame ...`; PaperPad logs `frame uploaded ...
cache=updated`. A mismatched extent or failed upload does not replace the last
successfully displayed frame. PaperPad redraws that cached frame after X11
Expose and same-viewport geometry redraws; successful cache
redraws log `frame redrawn ... cache=hit`. A viewport-size change invalidates
the old cache rather than stretching or clipping it.

For the manual device check, verify the four differently sized blocks in
`corners` occupy the expected corners, the `border` reaches the remote
viewport's rightmost pixel and bottom row, black/white polarity is correct, and
no pattern overwrites the 72-pixel local Exit strip. A deliberately mismatched
frame such as `frame white 1272x1623` must not replace the cached valid frame.
In a separate run, stop PaperSpoon, trigger an X11 Expose (brief sleep/wake on
the tested Paperwhite), and confirm the cached frame returns without the host;
PaperPad logs `frame redrawn ... cause=Expose cache=hit`. Press Exit after a
frame to confirm it remains device-local and responsive.

For a Wi-Fi run, the listener binds `0.0.0.0` on TCP 5581 **and** starts the
UDP discovery responder on `0.0.0.0:5580` (you should see both the TCP
banner and `discovery listening address=0.0.0.0:5580`). With no
`PAPERPAD_COMPANION`, the Kindle discovers PaperSpoon automatically
over the LAN. Wi-Fi and MTP can coexist over the USB link. USBNetwork is not
available on this Paperwhite 6 — no maintained USBNetwork package accepts
the device — so the USBNetwork
interface setup and MTP/USBNetwork exclusivity rules do not apply.

Passing TCP port `0` asks the OS for an ephemeral port; PaperSpoon prints and
advertises that actual port rather than `0` or the default.

### Hammering actions into the Mac (Hammerspoon)

PaperSpoon forwards each host-resolved application action to Hammerspoon as a
URL event: it runs `open -g hammerspoon://paperpad?action=<id>` once per
activation.
Forwarding is on by default; pass `--no-forward-url` to disable it:

```sh
cargo build --release --package paperspoon
./target/release/paperspoon 5581 /tmp/paperspoon.log
```

The banner now shows `forwarding actions to Hammerspoon via open -g
hammerspoon://paperpad/...`.

Hammerspoon handles this from `~/.hammerspoon/init.lua` (a working copy lives
at `tools/hammerspoon/init.example.lua`) via `hs.urlevent.bind("paperpad",
...)`:

- `media.play_pause`, `media.next`, `media.previous` control the Music app
  via in-process AppleScript (`hs.osascript`);
- `terminal.new_window` and `zoom.toggle_mute` dispatch to keyboard
  shortcuts (`cmd+n`, `cmd+shift+a`);
- unknown ids raise a notification.

Only a completed tap within the same host-rendered button resolves an action.
PaperSpoon logs `host action button=... semantic=... dispatch=...` for that
attempt. Hammerspoon delivery and the target application's response still
depend on the host environment; no device-side replay is performed.

## Host checks and Kindle build

Run the complete gate from the repository root, in this order:

```sh
make check
make build
make verify
git diff --check
```

The verified package is `kindle-extension/rust_x11_hello`; its binary is:

```text
kindle-extension/rust_x11_hello/bin/rust_x11_hello
```

`make check` formats, checks, lints, and tests the whole Rust workspace; it
also validates the KUAL scripts, MTP deployment success/rollback/failure
paths, and menu JSON. It requires the Rust toolchain, Bash, `jq`, and local
loopback socket access. `make build` and `make verify` require Docker.
Verification rejects a dynamic interpreter and GLIBC symbol requirements.

## Fresh MTP installation

After removing the legacy extension and confirming `/extensions/rust_x11_hello` does not already exist:

```sh
scripts/deploy-kindle-mtp.sh install
```

The installer verifies each upload by reading it back and uploads `menu.json` last, so KUAL does not expose a partially transferred extension. It refuses to overwrite an existing canonical installation.

For a later update, first use Paperpad's in-window **Exit** button or let the
watchdog stop the app, confirm the window is gone, and run:

```sh
scripts/deploy-kindle-mtp.sh update --confirm-stopped
```

Update mode stages and verifies the new binary, downloads the active binary into a guarded host temporary directory, and uploads a verified device-side copy as `rust_x11_hello.previous`. It then activates the new binary with `put --replace --verify`, because tested Kindle firmware rejects MTP object renames. If activation fails, it attempts a verified replacement from the downloaded prior binary. Another update is refused while the retained backup exists. MTP cannot prove that a process is stopped; `--confirm-stopped` is an explicit operator assertion.

MTP does not provide a multi-file transaction. If an update transfer fails before binary activation, the old binary remains selected but `.new`, `.previous`, or some support files may already exist; inspect the reported remote listing and repair the update before opening KUAL.

## Device test

In KUAL, use **Run Paperpad (90s)**. Perform taps within the visible window,
then use Paperpad's in-window **Exit** button or allow the watchdog to end the
run. There is no separate stop menu item because Paperpad covers KUAL while its
full-screen window is open.

After the process ends, retrieve the log:

```sh
mtp-rs get /extensions/rust_x11_hello/rust_x11_hello.log \
  rust_x11_hello.device.log --replace
```

Expected input records have stable fields such as:

```text
input type=ButtonPress detail=1 event_x=412 event_y=183 root_x=492 root_y=303 time=123456 window=0x2600001 root=0x50d child=0x0 state=0x0000 same_screen=true
```

No `ButtonPress`/`ButtonRelease` records after verifying the deployed checksum, event mask, mapped window, and test geometry means core-X11 touch remains unverified on that Kindle configuration; it is not evidence that the Rust build failed.

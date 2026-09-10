# Paperpad

Paperpad is a bounded Kindle/KUAL grid remote. It translates core X11 touch
events into stable semantic actions, sends them over Wi-Fi to the PaperSpoon
macOS companion, and renders short status commands returned by PaperSpoon.

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
| UI state and activation decisions | `src/app.rs`, using pure logic from `src/ui/` |
| X11 resources, event translation, and rendering | `src/x11/` |
| Paperpad TCP lifecycle, workers, queues, and display mailbox | `src/net/` |
| Unique-endpoint discovery policy | `src/discovery.rs` |
| Shared wire constants, formatting, and parsing | `crates/paper-protocol/` |
| PaperSpoon listener, current connection, discovery responder, and forwarding | `tools/paperspoon/` |
| KUAL lifecycle and MTP packaging | `kindle-extension/` and `scripts/deploy-kindle-mtp.sh` |

The dependency direction is deliberate: protocol code depends only on
`std`; UI geometry and decisions know nothing about X11 or sockets; X11 and
network modules adapt external events into those decisions. The X11 thread
owns the window and `AppState`. Network workers own blocking connection work,
with actions crossing a bounded queue and display updates crossing a
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
- Paperpad partitions that physical extent into a top remote-content viewport and a fixed 72px Paperpad-owned system strip at the bottom. On the Paperwhite the remote viewport is `(0,0) 1272 x 1624`, and the system strip is `(0,1624) 1272 x 72`. PaperSpoon must render only the remote viewport.
- Each of the nine grid cells is square, with side length `(screen width / 3) - (20 * 2)` using integer division. On this Kindle, cells are `384 x 384`, with 20px outer margins and 40px gaps between rows and columns. Any division remainder is absorbed by the column gaps so both outer edges stay aligned.
- The title sits above the grid. Exit occupies the local bottom strip, spanning `screen width - 40px` with 20px horizontal margins. The transitional 40px PaperSpoon `display` status strip remains inside the bottom of the remote viewport, immediately above Exit.
- Shorter windows reduce the cell side to fit the application grid and status strip inside the remote viewport without stretching the cells. Windows too small for the application layout have no application buttons; local Exit remains available whenever the window is at least 41px wide and 72px tall. Layout coordinates are capped at X11's signed-coordinate limit.
- The final event in each `Expose` batch clears and redraws the current window extent.
- A size-changing `ConfigureNotify` updates drawing and hit testing together and cancels an active contact. Duplicate geometry is ignored; a zero-width/zero-height report is logged without replacing the last valid extent.

Host tests cover the portrait layout, smaller and landscape windows, division remainders, gaps, aligned Exit bounds, and zero/maximum dimensions. ARM/static checks verify buildability. Full-screen rendering and touch behavior still require a physical Kindle run.

## Logical hit testing

The remote viewport contains nine logical application buttons in a 3×3 grid. The bottom system strip contains a separate local Exit control. Their geometry is independent of X11 event structures and uses half-open bounds, so trailing edges and gaps do not activate a control.

X11 pointer events use signed, physical window coordinates. Paperpad maps application contacts into unsigned coordinates relative to the remote viewport before application hit testing; physical points in the system strip or outside the window have no remote coordinate. A primary release without a remote coordinate cancels any armed application contact, while local Exit hit testing remains in physical coordinates.

Only core-X11 `detail=1` participates in UI activation. A primary press arms the button under the initial coordinate; a matching primary release activates only when it remains inside that same button and emits:
```text
ui action=activate button=4 semantic=terminal.new_window
```

Every application-grid activation also emits its stable semantic action id.
Exit is a Paperpad-local lifecycle control and never produces a remote semantic
action. The current application grid maps buttons 1–9 to:

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

Exit has its own Paperpad `SystemUi` geometry and contact state rather than an
application button ID. Its activation is logged as `ui action=activate
system=exit` and closes the window without writing to PaperSpoon.

Buttons 7–9 send placeholder action IDs for future companion bindings. Rendering
and touch behavior remain device-specific and must be rechecked after changes
to geometry, event translation, or the X11 adapter.

These dotted ids are the wire units of the semantic protocol; the transport
that carries them is described below. USBNetwork itself is not used: no
maintained USBNetwork package targets this Paperwhite 6, so the transport is
Wi-Fi.

Presses outside the grid, releases in another button or outside the grid, repeated primary presses, geometry changes, and window unmapping cancel the contact. Unmatched releases do nothing. Auxiliary details such as the observed Kindle `detail=6` and `detail=9` pairs retain their raw diagnostic lines but neither activate nor cancel the armed primary contact. Pointer motion remains unlogged.

## TCP transport (Wi-Fi)

The Kindle opens one persistent TCP connection to PaperSpoon on launch.
Each activation sends one newline-terminated protocol line over that
connection:

```text
event action=<semantic-id>;
```

PaperSpoon (a std-only Rust listener, `tools/paperspoon`) prints each
received line and appends it to a log file. It also forwards lines typed on
its stdin to the Kindle as control commands:

```text
display <text>
```

which renders `<text>` in the window's status strip (below the exit bar)
and is logged on the device as `display: <text>`.

Display commands are case-sensitive. Paperpad accepts `display <text>` and
the manual-terminal alias `display:<text>`; it trims surrounding payload
whitespace and ignores empty commands or other strings beginning with
`display`.

Control lines must be valid UTF-8 and remain bounded by the 8 KiB inbound-line
limit. For the Kindle core X11 font, Paperpad preserves printable ASCII
(`U+0020..=U+007E`), renders control and non-ASCII Unicode scalars as `?`, and
draws at most 255 output bytes. Longer status text is truncated at that
rendering boundary without terminating the app; diagnostics retain the
original received text.

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

Setting `RUST_X11_HELLO_COMPANION` (e.g. to a Wi-Fi run where the Mac is at
`192.168.0.12`) bypasses discovery and connects directly:

```sh
RUST_X11_HELLO_COMPANION=192.168.0.12
```

The host value is trimmed. An absent or blank host selects discovery, which
uses the TCP port advertised by PaperSpoon. With an explicit host,
`RUST_X11_HELLO_COMPANION_PORT` optionally overrides the default TCP port
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
on the device; it never breaks the X11 event loop or the on-device activation
log. PaperPad retries in the background every two seconds. Actions made while
disconnected fail immediately and are not queued or replayed after connection.
The Kindle opens no listening TCP socket; only the action id leaves the
device, and only `display` commands enter it.

### Running PaperSpoon

From the repository root, build and run the Rust listener on the Mac:

```sh
cargo build --release --package paperspoon
./target/release/paperspoon 5581 /tmp/paperspoon.log
```

Then type a display command at its stdin:

```text
display hello
```

For a Wi-Fi run, the listener binds `0.0.0.0` on TCP 5581 **and** starts the
UDP discovery responder on `0.0.0.0:5580` (you should see both the TCP
banner and `discovery listening address=0.0.0.0:5580`). With no
`RUST_X11_HELLO_COMPANION`, the Kindle discovers PaperSpoon automatically
over the LAN. Wi-Fi and MTP can coexist over the USB link. USBNetwork is not
available on this Paperwhite 6 — no maintained USBNetwork package accepts
the device — so the USBNetwork
interface setup and MTP/USBNetwork exclusivity rules do not apply.

Passing TCP port `0` asks the OS for an ephemeral port; PaperSpoon prints and
advertises that actual port rather than `0` or the default.

### Hammering actions into the Mac (Hammerspoon)

PaperSpoon forwards every received action line to Hammerspoon as a URL event:
it runs `open -g hammerspoon://paperpad?action=<id>` once per action.
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

`hs.urlevent` fires exactly once per URL open, so every Kindle tap dispatches
exactly one action — no sockets to manage, no timers, no replay loops.

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

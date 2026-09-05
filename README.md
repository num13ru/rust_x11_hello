# rust_x11_hello

A bounded Kindle/KUAL prototype for determining whether the Kindle X server translates touchscreen input into core X11 pointer events.

The current milestone opens a persistent override-redirect X11 window, redraws final `Expose` batches, tracks valid geometry changes, and writes one structured line per `ButtonPress`/`ButtonRelease`. Pointer motion is subscribed but deliberately not logged. The KUAL launcher serializes launch attempts with an owner-checked lock and stops the test after 90 seconds, with a five-second `TERM` grace followed by `KILL` only after revalidating the recorded PID and executable.

## Conditions under which this works

- A jailbroken Kindle with KUAL and an X server compatible with `x11rb` core X11 requests.
- An ARMv7/EABI5-compatible Kindle userspace. The produced binary is statically linked with musl; a device that cannot execute ARMv7 binaries needs a different build target.
- KUAL supplies the working `DISPLAY`/`XAUTHORITY` environment used by the existing device launcher.
- `mtp-rs` can access the unlocked Kindle over USB. Its `/extensions` path maps to `/mnt/us/extensions` at runtime.
- The canonical extension is installed as `/extensions/rust_x11_hello`. Do not keep the legacy `/extensions/rust_hello` entry alongside it.
- Physical-device logs are authoritative for touch support. Host compilation or desktop pointer events cannot prove Kindle touchscreen translation.

## Geometry-aware rendering

- The app opens a borderless window at `(0,0)` covering the selected X11 screen. The Paperwhite 6 reports `1272 x 1696` in portrait; runtime dimensions come from X11 rather than a fixed device resolution.
- Each of the nine grid cells is square, with side length `(screen width / 3) - (20 * 2)` using integer division. On this Kindle, cells are `384 x 384`, with 20px outer margins and 40px gaps between rows and columns. Any division remainder is absorbed by the column gaps so both outer edges stay aligned.
- The title sits above the grid. Exit sits 8px below it, is 72px tall (twice its previous height), and spans `screen width - 40px` with the same outer margins. A 40px status strip remains reserved at the screen bottom for PaperSpoon `display` text.
- Shorter windows reduce the cell side to fit the grid, Exit, and status strip without stretching the cells. Windows too small for the fixed margins and controls have no interactive buttons. Layout coordinates are capped at X11's signed-coordinate limit.
- The final event in each `Expose` batch clears and redraws the current window extent.
- A size-changing `ConfigureNotify` updates drawing and hit testing together and cancels an active contact. Duplicate geometry is ignored; a zero-width/zero-height report is logged without replacing the last valid extent.

Host tests cover the portrait layout, smaller and landscape windows, division remainders, gaps, aligned Exit bounds, and zero/maximum dimensions. ARM/static checks verify buildability. Full-screen rendering and touch behavior still require a physical Kindle run.

## Logical hit testing

The window contains nine logical buttons in a 3×3 grid, with a separate Exit button aligned below. Their geometry is independent of X11 event structures and uses half-open bounds, so the trailing edges and gaps do not activate a button.

Only core-X11 `detail=1` participates in UI activation. A primary press arms the button under the initial coordinate; a matching primary release activates only when it remains inside that same button and emits:
```text
ui action=activate button=4 semantic=terminal.new_window
```

Every activation also emits its stable semantic action id. The current grid maps buttons 1–9 and Exit to:

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
| Exit (ID 10) | `app.exit` (closes the window locally) |

Buttons 7–9 send placeholder action IDs for future companion bindings. The previous
760×528 three-row layout was confirmed on the physical Kindle; the new full-screen
layout still needs device verification.

These dotted ids are the wire units of the semantic protocol; the transport
that carries them is described below. USBNetwork itself is not used: no
maintained USBNetwork package targets this Paperwhite 6 (see
`docs/usbnetwork-pw2-report.md`), so the transport is
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

- Fixed ports: **UDP 5580** (PaperSpoon discovery listener), **UDP 5582**
  (PaperPad discovery client), **TCP 5581** (existing listener).
- The wire format is newline-terminated ASCII: `PAPERPAD DISCOVER <nonce>`
  and `PAPERSPOON HERE <nonce> <tcp-port>`. The nonce distinguishes the
  current attempt from stale/unrelated datagrams; the response must echo it.
- The response is **unicast** to the request's source; no broadcast
  responses, no multicast.
- The response payload never contains an IP address; the UDP source address
  is the discovered PaperSpoon address.
- Discovery is bounded (3 probes, 500 ms window each) and the UI remains
  usable even when PaperSpoon cannot be found.

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

This remains the deterministic control path and debugging/recovery override.
When unset, discovery runs and there is **no** fallback to a hard-coded IP.

Verified on the physical Paperwhite 6 (evidence under
`artifacts/kindle-runs/discovery-test-*`): explicit control (Test A), full
discovery chain (Test B), PaperPad restart (Test C), Mac DHCP address change
`192.168.0.12 -> 192.168.0.50` with no configuration edit (Test D), and
bounded failure with the UI alive when PaperSpoon is absent (Test E).

A PaperSpoon that is unreachable costs bounded time and is logged as
`transport error: ...` on the device; it never breaks the X11 event loop or
the on-device activation log, and the Kindle retries on the next activation.
The Kindle opens no listening TCP socket; only the action id leaves the
device, and only `display` commands enter it.

### Running PaperSpoon

Build and run the Rust listener (on the Mac):

```sh
cd tools/paperspoon
cargo build --release
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
the device (see `docs/usbnetwork-pw2-report.md`) — so the USBNetwork
interface setup and MTP/USBNetwork exclusivity rules do not apply.

## Host checks and Kindle build

```sh
make check
make build
make verify
```

The verified package is `kindle-extension/rust_x11_hello`; its binary is:

```text
kindle-extension/rust_x11_hello/bin/rust_x11_hello
```

`make check` requires the Rust toolchain, Bash, and `jq`. `make build` and `make verify` require Docker. Verification rejects a dynamic interpreter and GLIBC symbol requirements.

## Fresh MTP installation

After removing the legacy extension and confirming `/extensions/rust_x11_hello` does not already exist:

```sh
scripts/deploy-kindle-mtp.sh install
```

The installer verifies each upload by reading it back and uploads `menu.json` last, so KUAL does not expose a partially transferred extension. It refuses to overwrite an existing canonical installation.

For a later update, first let the watchdog stop the app (or use the validated stop action), confirm the window is gone, and run:

```sh
scripts/deploy-kindle-mtp.sh update --confirm-stopped
```

Update mode stages and verifies the new binary, downloads the active binary into a guarded host temporary directory, and uploads a verified device-side copy as `rust_x11_hello.previous`. It then activates the new binary with `put --replace --verify`, because tested Kindle firmware rejects MTP object renames. If activation fails, it attempts a verified replacement from the downloaded prior binary. Another update is refused while the retained backup exists. MTP cannot prove that a process is stopped; `--confirm-stopped` is an explicit operator assertion.

MTP does not provide a multi-file transaction. If an update transfer fails before binary activation, the old binary remains selected but `.new`, `.previous`, or some support files may already exist; inspect the reported remote listing and repair the update before opening KUAL.

## Device test

In KUAL, use **Run Rust X11 Hello (90s)**. Perform taps within the visible window, then allow the watchdog to end the run. If KUAL remains accessible, **Stop Rust X11 Hello** sends `TERM` only after verifying the PID belongs to the installed binary.

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

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

- The initial Kindle window remains `(80,120)` at `760 x 400`; the 2×3 button grid, exit bar, and status strip scale from that reference geometry.
- The final event in each `Expose` batch clears and redraws the current window extent.
- A size-changing `ConfigureNotify` updates the active geometry and redraws immediately. Duplicate geometry is ignored, and a defensive zero-width/zero-height report is logged without replacing the last valid extent.
- Grid bounds and text origins scale from the verified `760 x 400` reference layout (grid + exit bar above a 40px status strip where companion `display` text renders). Arithmetic is bounded for tiny windows and the full core-X11 `u16` extent; text coordinates saturate at the protocol's signed-coordinate limit.

Host tests verify layout and geometry decisions at default, half, one-pixel, zero, and maximum dimensions. The ARM/static gates verify deployability, but runtime resize/redraw behavior still requires observation on an X server that actually sends resize events; no such event was part of the fixed-geometry Kindle evidence run.

## Logical hit testing

The window contains six logical buttons in a 2×3 grid. Their geometry is independent of X11 event structures and uses half-open bounds, so every shared edge belongs to exactly one button.

Only core-X11 `detail=1` participates in UI activation. A primary press arms the button under the initial coordinate; a matching primary release activates only when it remains inside that same button and emits:
```text
ui action=activate button=4 semantic=terminal.new_window
```

Every activation also emits its stable semantic action id. The current grid maps buttons 1-6 to:

| Button | Semantic action id |
| ------ | ------------------ |
| 1 | `media.play_pause` |
| 2 | `media.next` |
| 3 | `media.previous` |
| 4 | `terminal.new_window` |
| 5 | `tmux.work` |
| 6 | `zoom.toggle_mute` |

These dotted ids are the wire units of the semantic protocol; the transport
that carries them is described below. USBNetwork itself is not used: no
maintained USBNetwork package targets this Paperwhite 6 (see
`docs/usbnetwork-pw2-report.md`), so the transport is
Wi-Fi.

Presses outside the grid, releases in another button or outside the grid, repeated primary presses, geometry changes, and window unmapping cancel the contact. Unmatched releases do nothing. Auxiliary details such as the observed Kindle `detail=6` and `detail=9` pairs retain their raw diagnostic lines but neither activate nor cancel the armed primary contact. Pointer motion remains unlogged.

## TCP transport (Wi-Fi)

The Kindle uses one persistent TCP connection for versioned events, ACKs, and
`display` controls. An explicit host is tried directly; otherwise a background
worker discovers the companion over UDP without blocking the X11 event loop.

```text
event action=<semantic-id>;
```

The line above is the migration format accepted by the new companion for an
old Kindle binary. A negotiated version-1 session sends and acknowledges:

```text
hello v=1 session=<32-hex-id>;
welcome v=1 companion=<32-hex-id>;
event v=1 session=<session-id> seq=<u64> action=<semantic-id>;
ack v=1 session=<session-id> seq=<u64> status=logged;
```

`logged` means the companion validated the event, appended it to its log,
flushed it, and called `sync_data`. It does not mean a later media, tmux, or
other external side effect completed. The Kindle retries an unacknowledged
event at most three times with the same `(session, seq)` identity. The
companion suppresses duplicates only within its current process; exactly-once
execution across a companion restart is not claimed.

The companion (a std-only Rust listener, `tools/companion`) prints each
received line and appends it to a log file. It also forwards lines typed on
its stdin to the Kindle as control commands:

```text
display <text>
```

which renders `<text>` in the window's status strip (below the exit bar)
and is logged on the device as `display: <text>`.

With no explicit host, the Kindle sends bounded discovery probes to UDP port
`5580`, validates the echoed random nonce, and connects to the TCP port in the
single valid offer (default `5581`). Multiple unpaired companion identities are
an error. Set `RUST_X11_HELLO_COMPANION_ID` to the selected 32-hex companion ID
or set `RUST_X11_HELLO_COMPANION` to bypass discovery with a hostname/IP. The
explicit port override is `RUST_X11_HELLO_COMPANION_PORT`.

```sh
cd tools/companion && cargo build --release && \
  ./target/release/companion 5581 /tmp/companion.log
```

Discovery uses three 250 ms response windows; TCP connect uses a 300 ms bound,
the handshake uses a 750 ms bound, and ACK retry uses a one-second timeout.
Those waits occur only in the network worker. Failure never blocks or exits the
X11 loop. Pending actions and all handoff channels are bounded. The Kindle
opens no listening socket.

### Running the companion

Build and run the Rust listener (on the Mac):

```sh
cd tools/companion
cargo build --release
./target/release/companion 5581 /tmp/companion.log companion.id 5580
```

Then type a display command at its stdin:

```text
display hello
```

The companion binds TCP and discovery UDP on `0.0.0.0`. Its identity file is
created atomically and reused. CLI syntax is
`companion [tcp-port] [log-file] [identity-file] [discovery-port]`.

Automatic discovery works only on an IPv4 broadcast domain where the AP,
macOS firewall, VLAN policy, and guest/client isolation allow peer UDP and TCP.
It is unauthenticated: a stable companion ID selects a peer but does not prove
its identity. Use only on a trusted LAN, and retain the explicit hostname/IP
fallback for networks that do not pass broadcast. USBNetwork is unavailable on
this Paperwhite 6; see `docs/usbnetwork-pw2-report.md`.

To talk to an old companion, set both an explicit host and
`RUST_X11_HELLO_LEGACY=1`. Legacy mode sends the old event line without hello,
ACK, retry, or a receipt claim; there is no automatic downgrade from a failed
versioned handshake.

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

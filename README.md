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

- The initial Kindle window remains `(80,120)` at `760 x 360`; the 2×3 button grid and labels scale from that reference geometry.
- The final event in each `Expose` batch clears and redraws the current window extent.
- A size-changing `ConfigureNotify` updates the active geometry and redraws immediately. Duplicate geometry is ignored, and a defensive zero-width/zero-height report is logged without replacing the last valid extent.
- Grid bounds and text origins scale from the verified `760 x 360` reference layout. Arithmetic is bounded for tiny windows and the full core-X11 `u16` extent; text coordinates saturate at the protocol's signed-coordinate limit.

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

These dotted ids are the wire units of the semantic protocol; the USBNetwork
transport that carries them is described below.

Presses outside the grid, releases in another button or outside the grid, repeated primary presses, geometry changes, and window unmapping cancel the contact. Unmatched releases do nothing. Auxiliary details such as the observed Kindle `detail=6` and `detail=9` pairs retain their raw diagnostic lines but neither activate nor cancel the armed primary contact. Pointer motion remains unlogged.

## USBNetwork semantic transport

Each activation also sends one newline-terminated protocol line over TCP to the
companion host:

```text
event action=<semantic-id>;
```

The Kindle connects to `192.168.15.201:5581` (the USBNetwork static host) with
a 150 ms connect timeout, writes the line, and disconnects. A companion that is
down or unreachable costs a bounded 150 ms per activation and is logged as
`transport error: ...` on the device; it never breaks the X11 event loop or the
on-device activation log. This milestone still opens no listening socket of its
own and carries no network payload other than the action id.

The companion receiver is:

```sh
python3 tools/companion_listen.py
```

It listens on `0.0.0.0:5581` on the Mac, prints each received line, and
validates the id against the six known semantic actions. The Mac must have the
USBNetwork interface up with this host at `192.168.15.201` (Kindle at
`.200`); MTP mode and USBNetwork cannot run over the same USB link at once, so
deploy the binary over MTP first, then switch the device to USBNetwork for the
run.

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

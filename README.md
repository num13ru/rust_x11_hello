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

- The initial Kindle window remains `(80,120)` at `760 x 360`; that verified geometry produces the same two rectangles and four text baselines as before.
- The final event in each `Expose` batch clears and redraws the current window extent.
- A size-changing `ConfigureNotify` updates the active geometry and redraws immediately. Duplicate geometry is ignored, and a defensive zero-width/zero-height report is logged without replacing the last valid extent.
- Rectangle insets and text origins scale from the verified `760 x 360` reference layout. Arithmetic is bounded for tiny windows and the full core-X11 `u16` extent; text coordinates saturate at the protocol's signed-coordinate limit.
- This milestone does not interpret touches or change rendering in response to input. Logical hit testing remains the next step.

Host tests verify layout and geometry decisions at default, half, one-pixel, zero, and maximum dimensions. The ARM/static gates verify deployability, but runtime resize/redraw behavior still requires observation on an X server that actually sends resize events; no such event was part of the fixed-geometry Kindle evidence run.

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

Update mode stages and verifies the new binary, retains the old binary as `rust_x11_hello.previous`, and refuses another update while that backup exists. MTP cannot prove that a process is stopped; `--confirm-stopped` is an explicit operator assertion.

MTP does not provide a multi-file transaction. If an update transfer fails before binary activation, the old binary remains selected but some support files may already be new; inspect the reported remote listing and rerun or repair the update before opening KUAL.

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

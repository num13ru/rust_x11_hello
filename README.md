# PaperPad

PaperPad uses a Kindle as a Wi-Fi display and touch surface for a companion app.
PaperSpoon renders the application on the Mac. PaperPad displays the received
pixels and sends touch coordinates back. PaperView previews the same application
on a desktop.

## Build and run

### Build the Kindle package

Run the standard checks and build with the pinned Nix toolchain.

```sh
nix develop -c make check build verify
```

The verified static ARMv7 binary is written to
`kindle-extension/rust_x11_hello/bin/rust_x11_hello`.
See the [Kindle build guide](docs/containerless-kindle-build.md) for alternate
backends and cache management.

### Start PaperSpoon

```sh
cargo run --release -p paperspoon -- 5581 /tmp/paperspoon.log
```

PaperSpoon listens on TCP 5581 and answers discovery on UDP 5580. Keep the Mac
and Kindle on the same Wi-Fi network. Start PaperSpoon with `--no-forward-url`
when previewing controls without dispatching actions to Hammerspoon. The
[example Hammerspoon handler](tools/hammerspoon/init.example.lua) shows how to
wire up the application grid.

### Install or update the Kindle extension

Stop PaperPad with its on-screen **Exit** button before connecting the Kindle
over USB. Unlock the device, allow USB accessory access, and check that
`mtp-rs devices` lists it.

For a new installation, use the install command.

```sh
scripts/deploy-kindle-mtp.sh install
```

For an existing installation, use the update command.

```sh
scripts/deploy-kindle-mtp.sh update
```

An update keeps the previous binary as `bin/rust_x11_hello.previous`. Archive
or remove the existing rollback copy before a subsequent update. Both commands
verify the transferred files and publish the menu last.

### Launch from KUAL

Unplug USB and choose **PaperPad → Run Paperpad DU**. The app stays open until you
tap **Exit** or the process ends. Exit works even if PaperSpoon is disconnected.

- **Run Paperpad DU** uses DU for binary updates and GC16 for grayscale.
- **Run Paperpad DU with PPFB v2** forces legacy Raw transport.
- **Run Paperpad GC16** uses GC16 for all remote updates.
- **Run Paperpad A2** and **Run Paperpad DU4** are experimental waveform modes.
  Pixel transitions outside their supported levels use GC16.

PaperPad uses native MXCFB display updates and exclusive evdev touch input.
On exit it releases input, restores the original display, and closes the
temporary discovery firewall rule.

### Preview on the desktop

With PaperSpoon running, start:

```sh
cargo run -p paperview -- --host 127.0.0.1 --port 5581 --viewport 1272x1624
```

PaperView starts read-only. Add `--interactive` to send pointer input, or
`--save-dir DIR` to export a source-frame PNG using **S**. PaperSpoon serves a
single display client, so opening PaperView can replace the Kindle session.
See the [PaperView guide](docs/paperview-development-workflow.md) for preview and
Apple Music Now Playing setup.

## Connection settings

Compatible peers negotiate PPFB v3 with independent Zstandard level-1 frames
and per-frame Raw fallback. Automatic mode reconnects using PPFB v2 when a
legacy host closes the v3 handshake. The [PPFB v3 guide](docs/ppfb-v3.md)
describes explicit Raw and v2 modes and the wire format.

PaperPad discovers PaperSpoon over UDP by default. Use an explicit host on
networks that block discovery.

| Variable | Purpose |
| --- | --- |
| `PAPERPAD_COMPANION` | PaperSpoon host name or IP address |
| `PAPERPAD_COMPANION_PORT` | TCP port with an explicit host, defaults to 5581 |
| `PAPERPAD_TRANSPORT` | `auto`, `v2`, `v3-raw`, `v3-auto`, or `v3-zstd` |
| `PAPERPAD_MXCFB_WAVEFORM` | `du`, `gc16`, `a2`, or `du4` |
| `PAPERPAD_EXT_DIR` | Launcher extension directory |

On Kindle, the launcher writes `rust_x11_hello.status` and
`rust_x11_hello.log` in the extension directory.

## Project layout

| Path | Role |
| --- | --- |
| `src/` | Kindle runtime, input, and display backend |
| `crates/paper-protocol/` | Shared transport and framebuffer formats |
| `tools/paperspoon/` | Companion application and renderer |
| `tools/paperview/` | Desktop preview client |
| `kindle-extension/` | KUAL menu and launcher |

Completed plans and device notes are in [docs/archive](docs/archive/).
Historical compression measurements are in [docs/benchmarks](docs/benchmarks/).

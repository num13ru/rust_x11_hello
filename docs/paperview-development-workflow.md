# PaperView local development workflow

This workflow previews PaperSpoon-owned PPFB frames in a native desktop window
without a Kindle. PaperView remains read-only by default: it does not render
application text or acquire Now Playing metadata. The explicit `--interactive`
mode can send viewport-relative PPFB pointer input for application testing.

## Conditions

- Run PaperSpoon on macOS when using `frame nowplaying`; its media adapter is
  macOS-specific and needs an active system Now Playing source.
- Run both commands from the repository root after
  `cargo check -p paperview -p paperspoon` succeeds.
- Use the same viewport in PaperView, `ui`, and every diagnostic command that
  takes dimensions. The examples use the Paperwhite 6 viewport, `1272x1624`.
- PaperSpoon serves one current PPFB client. Starting PaperView may replace a
  connected PaperPad session.
- Use `--no-forward-url` so application-UI experiments cannot open Hammerspoon
  action URLs.

If TCP port 5581 is already occupied, choose another nonzero port and use it in
both terminal commands.

## Terminal 1: start PaperSpoon

Keep the two append-style logs outside the repository:

```sh
cargo run -p paperspoon -- \
  5581 \
  /private/tmp/paperspoon-paperview.log \
  --latency-log=/private/tmp/paperspoon-paperview-latency.jsonl \
  --no-forward-url
```

Leave this terminal open. PaperSpoon accepts diagnostic commands on its stdin
and sends them to the most recently connected client.

## Terminal 2: start PaperView

```sh
cargo run -p paperview -- \
  --host 127.0.0.1 \
  --port 5581 \
  --viewport 1272x1624 \
  --save-dir /private/tmp
```

PaperView should connect without blocking the window event loop. Its title
reports connection state, the most recent frame ID and pixel format, the view
mode, and the negotiated endpoint and viewport.

## Optional pointer input

Omit `--interactive` for the default read-only preview. Add it to the PaperView
command only when local application input is intentional:

```sh
cargo run -p paperview -- \
  --host 127.0.0.1 \
  --port 5581 \
  --viewport 1272x1624 \
  --interactive
```

Keep PaperSpoon's `--no-forward-url` flag for this workflow. In interactive
mode:

- The primary button sends existing PPFB Down/Up messages. Secondary buttons,
  clicks in Fit or zoom letterboxing, and clicks while disconnected are ignored.
- A primary drag is remote touch input. Use the keyboard bindings below for
  local zoom and pan.
- Focus loss, resize, scale-factor changes, and view-transform changes end an
  active contact once at its last valid source coordinate. A disconnect clears
  it without replaying input into the next session.
- The window title includes `input interactive`; reconnect creates a fresh
  pointer session.

## Exercise the same window

Enter these commands one at a time in Terminal 1:

```text
ui 1272x1624
frame gray-gradient 1272x1624
frame gray-bars 1272x1624
frame nowplaying
```

The existing PaperView window should update after each valid frame; do not
restart it between commands. `ui` exercises PaperSpoon's application renderer
and may produce Mono1. The gradient, bars, and successful Now Playing frame use
Gray8. Frame IDs in the title should advance.

PaperSpoon rejects a diagnostic whose explicit dimensions differ from the
negotiated viewport. A missing media player, media-adapter timeout, or other
Now Playing acquisition failure reports an error in Terminal 1 and leaves the
previous valid PaperView image unchanged.

## Inspect source pixels

Use only PaperView's presentation controls; they do not change PaperSpoon
layout or source pixels:

- `F`: aspect-preserving Fit with neutral letterboxing.
- `1`: one source pixel per physical window pixel.
- `2` or `4`: exact integer nearest-neighbor enlargement.
- Arrow keys: pan by 32 source pixels while in an integer zoom mode.
- `Home`: recenter the bounded source-pixel pan.
- `S`: save the current validated source frame as an 8-bit grayscale PNG.

At 1x, 2x, and 4x, inspect antialiased Now Playing text without introducing a
second font or text-rendering path. When the scaled source exceeds the window,
use the arrow keys to reach its edges. The title's pan coordinates make the
crop explicit.

`S` writes into the existing `--save-dir` with a deterministic name such as
`paperview-frame-00000000000000000042-gray8-1272x1624.png`. It refuses to
overwrite an existing path. A nonexistent or unwritable directory produces a
terminal error and leaves the displayed frame unchanged. The exported pixels
come from the retained PPFB frame, not the fitted or zoomed window surface.

An operating-system screenshot is useful for manual inspection but is not a
canonical source-frame fixture: window scaling, capture, and display color
management may affect it. Do not commit screenshots containing personal music
metadata or artwork.

## Live Now Playing

Add `--now-playing-fake` to the PaperSpoon command in Terminal 1 and
`--interactive` to the PaperView command in Terminal 2. Keep PaperSpoon's
stdin open and enter `nowplaying start`. The live view needs a client that
advertises Gray8. The standard `1272x1624` viewport fits artwork, track
details, the three playback buttons, and a row for **Shuffle** and
**Repeat**. The seek bar spans the content width, with elapsed and
remaining time below it. Smaller viewports may omit the mode controls
or show only track details.

| PaperSpoon stdin | Effect |
| --- | --- |
| `nowplaying status` | Report playback and Gray8 connection state |
| `nowplaying refresh` | Request a fresh playback sample and artwork |
| `nowplaying stop` | Stop live updates and restore the grid for a Mono1-capable client |
| `nowplaying fake play`, `nowplaying fake pause`, `nowplaying fake next` | Simulate a change in the player |
| `nowplaying fake seek 45` | Simulate an external seek to 45 seconds |
| `nowplaying fake fail`, `nowplaying fake recover` | Exercise stale playback and recovery |
| `nowplaying fake slow`, `nowplaying fake normal` | Toggle an 800 ms command delay |

The fixture includes three tracks, long metadata, a delayed artwork response,
and a stream with unknown duration. Tap the seek bar to see one seek on
release. A missing duration disables the seek bar. Try pressing a button and
releasing over the artwork, then repeat with a disconnect between the two
events. Enter a valid `ui` or `frame` command to leave live mode and show that
command's output. If a diagnostic fails, its previous displayed pixels stay
in place. Start live mode again with `nowplaying start`.

Tap **Shuffle** to toggle on or off. **Repeat** cycles through **Off**,
**All**, and **One**. The buttons display the state reported by the player,
including changes made directly in Apple Music. A setting that Music
cannot report disables its button. The fake provider changes the button
states while keeping its track sequence deterministic.

### Apple Music

Restart PaperSpoon with `--now-playing-apple-music` and keep
`--no-forward-url` to disable Hammerspoon forwarding for grid actions.
The Apple Music flag authorizes direct playback commands separately.
Open Music and allow Automation access for the terminal or application
that launches PaperSpoon. Start playback in Music before testing the
controls. PaperSpoon polls and controls Music through AppleScript, so
switching system audio to another app leaves the widget on Music. The
system-wide `frame nowplaying` snapshot continues to use MediaRemote.

The pinned MediaRemote Perl adapter exposes a system session stream with
metadata, source identity, position, rate, and artwork. Its playback commands
target the active system session. The live provider uses Music for both
acquisition and commands so that controls act on the source shown in the
widget. Music reports state and position through AppleScript at a nominal
1× playback rate. Track identity uses Music's persistent ID when available
and its track ID otherwise. Artwork is limited to JPEG or PNG and cached by
track identity. Use `nowplaying refresh` if artwork changes for the same
track. Stopped playback, missing track identity, and stale samples disable
the controls.

### Refresh and measurement

PaperSpoon polls Music about once per second during playback, every two
seconds when paused, and at intervals of up to five seconds when stopped,
unavailable, or failing. It interpolates progress with a monotonic clock.
Progress refresh defaults to `1000` ms. Select `100`, `250`, `500`, `1000`,
or `2000` with `--now-playing-progress-ms=VALUE`. Track changes and button
feedback can publish between progress ticks. Unchanged pixels suppress
publication, so steady paused playback sends no periodic frames. A
reconnecting Gray8 client receives the current view at its negotiated size.

The PaperSpoon log records acquisition, command, artwork, render, and
socket-write durations, along with payload size, changed pixels, and the
bounding area of changed pixels. The logged state change after a command
describes a subsequent sample consistent with that command. Panel refresh
and ghosting require observations on the Kindle. Use **Run Paperpad GC16
(5m)** for this baseline and record any waveform experiments separately.

### Host verification on 2026-11-27

On macOS 26.7.1, PaperView at `1272x1624` showed progress, elapsed and
remaining time, long-title ellipsis, and delayed artwork. Prev, Play/Pause,
Next, and tap-to-seek worked with the fake provider. A localhost PPFB test
covered pause, provider failure and recovery, reconnect, viewport changes,
a stalled reader, and diagnostic takeover. The Apple Music provider
acquired playback data and grayscale artwork, and applied a seek while
Music was paused. The test restored the original position.

Short loopback runs at `1272x1624` used a debug build and the first fake
track. Each published Gray8 PPFB frame used 2,065,756 bytes.

| Progress interval | Received frames per second |
| ---: | ---: |
| 100 ms | 7.43 |
| 250 ms | 4.17 |
| 500 ms | 2.10 |
| 1000 ms | 1.05 |
| 2000 ms | 0.52 |

Physical Kindle results and operator observations are recorded with the
[GC16 device evidence](archive/mxcfb-device-evidence.md#live-now-playing-gc16-baseline-2026-11-27).

## Stop and clean up

Stop PaperView with its close button or `Ctrl-C` in Terminal 2, then stop
PaperSpoon with `Ctrl-C` in Terminal 1. Remove the temporary host logs when they
are no longer needed:

```sh
rm -f \
  /private/tmp/paperspoon-paperview.log \
  /private/tmp/paperspoon-paperview-latency.jsonl
```

Remove any exported PNG by its exact reported path when it is no longer
needed; avoid committing personal Now Playing metadata or artwork.

This workflow has been exercised on macOS Apple Silicon for native window
connection, frame rendering, keyboard zoom, read-only mouse behavior, and
interactive pointer mapping across Fit, 1x, 2x, 4x, disconnect, and reconnect.
Linux X11/Wayland and Windows remain unverified.

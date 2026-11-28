# PaperView local development workflow

This workflow previews PaperSpoon-owned PPFB frames in a native desktop window
without a Kindle. PaperView remains read-only by default: it does not render
application text or acquire Now Playing metadata. The explicit `--interactive`
mode can send viewport-relative PPFB pointer input for application testing.

## Conditions

- Run PaperSpoon on macOS when using **Now Playing → Snapshot**. The media
  adapter needs an active system Now Playing source.
- Run both commands from the repository root after
  `cargo check -p paperview -p paperspoon` succeeds.
- Keep the PaperSpoon viewport set to **Auto** to use PaperView's negotiated
  dimensions. The examples use the Paperwhite 6 viewport, `1272x1624`.
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

Leave the fullscreen console open. Its diagnostic buttons send frames to the
connected client.

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

Select these actions one at a time in the PaperSpoon console:

- **Application → Show UI**
- **Gray8 → Gradient**
- **Gray8 → Bars**
- **Now Playing → Snapshot**

The existing PaperView window should update after each valid frame.
**Show UI** exercises PaperSpoon's application renderer
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

At 1x, 2x, and 4x, inspect the binary Now Playing text and grayscale artwork.
When the scaled source exceeds the window,
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
`--interactive` to the PaperView command in Terminal 2. Select
**Now Playing → Start** in PaperSpoon. The live view needs a client that
advertises Gray8. The standard `1272x1624` viewport fits artwork, track
details, the three playback buttons, and a row for **Shuffle** and
**Repeat**. The seek bar spans the content width, with elapsed and
remaining time below it. Smaller viewports may omit the mode controls
or show only track details.

On the Paperwhite 6 viewport, the widget sits 24 pixels above the
device-local Exit strip, with 96 pixels between the artwork and title.

| PaperSpoon action | Effect |
| --- | --- |
| **Now Playing → Status** | Report playback and Gray8 connection state |
| **Now Playing → Refresh** | Request a fresh playback sample and artwork |
| **Now Playing → Stop** | Stop live updates and restore the grid for a Mono1-capable client |
| **Test provider → Play**, **Pause**, or **Next** | Simulate a change in the player |
| **Test provider → Seek…**, then enter `45` | Simulate an external seek to 45 seconds |
| **Test provider → Fail** or **Recover** | Exercise stale playback and recovery |
| **Test provider → Slow** or **Normal** | Toggle an 800 ms command delay |

The fixture includes three tracks, long metadata, a delayed artwork response,
and a stream with unknown duration. Tap the seek bar to see one seek on
release. A missing duration disables the seek bar. Try pressing a button and
releasing over the artwork, then repeat with a disconnect between the two
events. Use **Show UI** or a diagnostic action to leave live mode. If the
diagnostic fails, the previous displayed pixels stay in place. Select
**Now Playing → Start** to resume live updates.

Tap **Shuffle** to toggle on or off. **Repeat** cycles through **Off**,
**All**, and **One**. The buttons display the state reported by the player,
including changes made directly in Apple Music. A setting that Music
cannot report disables its button. The fake provider changes the button
states while keeping its track sequence deterministic.

### Apple Music

Restart PaperSpoon with the Terminal 1 command to select the default Apple
Music provider on macOS. Keep `--no-forward-url` to disable Hammerspoon
forwarding for grid actions. Select **Now Playing → Start** for live Music
sampling and playback controls.
Tap the cover to bring Music to the front and reveal the current track
where Music supports it. This also works with the cover placeholder.
Open Music and allow Automation access for the terminal or application
that launches PaperSpoon. Start playback in Music before testing the
controls. PaperSpoon polls and controls Music through AppleScript, so
switching system audio to another app leaves the widget on Music.
**Now Playing → Snapshot** uses the system-wide MediaRemote feed.

The pinned MediaRemote Perl adapter exposes a system session stream with
metadata, source identity, position, rate, and artwork. Its playback commands
target the active system session. The live provider uses Music for both
acquisition and commands so that controls act on the source shown in the
widget. Music reports state and position through AppleScript at a nominal
1× playback rate. Track identity uses Music's persistent ID when available
and its track ID otherwise. Artwork is limited to JPEG or PNG and cached by
track identity. Use **Now Playing → Refresh** if artwork changes for the same
track. Stopped playback, missing track identity, and stale samples disable
the controls.

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

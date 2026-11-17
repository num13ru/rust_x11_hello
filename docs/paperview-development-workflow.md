# PaperView local development workflow

This workflow previews PaperSpoon-owned PPFB frames in a native desktop window
without a Kindle. PaperView remains a read-only framebuffer client: it does not
render application text, acquire Now Playing metadata, or send pointer actions.

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
connection and frame rendering. Linux X11/Wayland, Windows, and automated GUI
keypress injection remain unverified.

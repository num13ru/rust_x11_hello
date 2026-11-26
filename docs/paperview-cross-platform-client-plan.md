# PaperView: cross-platform PPFB v2 framebuffer preview

Status: MVP implemented and validated on macOS; Linux and Windows unverified\
Scope: New host-side Rust application (`tools/paperview/`)\
Platforms: macOS, Linux (X11/Wayland), Windows\
Display backend: `winit` + `softbuffer`\
Target: Inspect PaperSpoon's actual Mono1 and Gray8 frames without a Kindle\
Implementation: Incremental milestones with explicit verification gates

## Implementation status (2026-11-26)

- Stages 0 through 3 are implemented. The optional Stage 4 mouse/touch
  emulation remains unimplemented; PaperView is read-only.
- On macOS, a single PaperView window received a Mono1 application frame and
  Gray8 gradient, bars, and real Now Playing frames from PaperSpoon at
  `1272x1624`. Fit, 1x, 2x, 4x, and keyboard panning were exercised through the
  native UI.
- The Now Playing source frame exported as a `1272x1624`, 8-bit grayscale PNG.
  A second save attempt left the original file's size, modification time, and
  SHA-256 unchanged, confirming the no-overwrite path.
- `make check` passes on macOS. It must run with loopback networking available
  because PaperSpoon's tests bind local TCP and UDP sockets.
- Linux and Windows build and manual validation were explicitly skipped because
  those environments were unavailable. They are unverified, not passing.
- This work was host-only: no Kindle binary was rebuilt or deployed.

## Goal

Create **PaperView**, a small cross-platform desktop PPFB v2 client that connects to PaperSpoon and displays its existing framebuffer as pixels in an OS window. The primary use case is iterating on PaperSpoon's Now Playing layout and its completed OpenType typography without deploying to, or operating, a physical PaperPad.

PaperView must render the **received pixels**, not reimplement PaperSpoon's widgets, font shaping, media integration, or UI layout.

```text
                     macOS (PaperSpoon)
              Widget/layout/OpenType rendering
                             |
                        PPFB v2/TCP
                             |
             +---------------+---------------+
             |                               |
      PaperView (MVP)               PaperPad (existing)
      winit + softbuffer                 MXCFB
      macOS/Linux/Windows              Kindle

MVP: exactly one of these clients is the current PaperSpoon session.
Simultaneous output to both is a separate follow-up, not this plan.
```

## Baseline and assumptions

The observations below describe the repository's public `master` as inspected on 2026-11-17. **Codex must re-check the current checkout before editing.** The current checkout is authoritative if names or behavior have changed.

- The workspace root `Cargo.toml` lists `crates/paper-protocol` and `tools/paperspoon` but not PaperView.
- `crates/paper-protocol/` is a dependency-light, transport-independent crate. It exports `V2Hello`, `V2PixelFormats`, `V2PixelFormat`, `V2Payload`, `decode_v2_message`, `decode_v2_payload`, and framebuffer types.
- PPFB v2 transmits `Mono1` (packed MSB-first, **0 = white**, 1 = black) and `Gray8` (one luminance byte per pixel, **0 = black**, 255 = white). Do not invert these conventions.
- The shared protocol validates frame dimensions, expected lengths, Mono1 row padding, and a maximum v2 message payload of 16 MiB. Maintain equivalent bounds in any additional buffering or presentation allocation.
- `V2Hello::with_pixel_formats()` can advertise `MONO1.with(Gray8)` and a viewport. Use existing protocol serializers and decoders rather than constructing binary packets manually.
- PaperSpoon listens on TCP 5581 by default, currently uses a single `CurrentConnection`, and targets the **current** peer. A PaperView connection may replace an active Kindle session; do not change this for the MVP.
- The tested Paperwhite 6 has a 1272x1624 remote viewport, excluding the device-local bottom strip. Treat these as **default simulated client dimensions**, not hardcoded application geometry.
- PaperSpoon already owns the OpenType rendering: pinned IBM Plex Mono for Mono1 and Inter for Gray8. No font dependency or font files belong in PaperView.
- `frame nowplaying` is a PaperSpoon stdin command; on macOS it uses a real system Now Playing snapshot. PaperView only displays the result. Deterministic fake metadata is a separate follow-up unless a suitable fixture pathway already exists.

## Non-goals

- No changes under `src/`, `kindle-extension/`, or the PaperPad runtime; no MTP deployment.
- No changes to PPFB v2 wire format, discovery protocol, `paper-protocol` semantics, or MXCFB waveform handling.
- No second application renderer, OpenType engine, JPEG decoder, Apple Music integration, or Kindle system Exit strip.
- No GPU texture pipeline, shaders, e-ink physics, ghosting simulation, or fixed-FPS rendering loop in the MVP.
- No local broadcast discovery: use an explicit TCP endpoint, defaulting to `127.0.0.1:5581` for local development.
- No attempt to keep Kindle and PaperView simultaneously connected in this change.
- No generalized desktop GUI framework or plugin system.

## Architectural decisions

1. **Independent workspace binary:** create `tools/paperview/` with a small executable depending on `paper-protocol`, `winit`, and `softbuffer`. Add other dependencies only when they solve an actual stage requirement. Avoid importing PaperSpoon internals.
2. **Existing protocol first:** PaperView acts as a PPFB v2 peer: connect, send `Hello` with viewport and both format capabilities, receive complete binary messages, accept validated `Frame` payloads, and handle EOF/reconnect. Reuse exact shared decoder/encoder APIs and identify the existing server-to-client message set before implementation.
3. **Bounded network worker:** perform blocking TCP reads outside the UI event loop. Decode full protocol messages with strict size limits. Send small wake notifications to the window thread through `winit`'s user-event mechanism (or an equally simple supported mechanism). Keep only the newest complete **validated** frame pending presentation; a slow window must not create an unbounded queue.
4. **Single-threaded window ownership:** create/manage `winit` window and `softbuffer` context/surface on the required UI thread; process resize, scale-factor, focus, close, and redraw events there. Do not move window/surface objects to the socket worker.
5. **Software pixel presentation:** expand validated Mono1 and Gray8 frames to `0x00RRGGBB` pixels for `softbuffer`, respecting stride, bit polarity, image bounds, and maximum allocation size. Preserve source pixels independently from the scaled window display.
6. **Viewport vs window separation:** the PPFB viewport is established by the client's `Hello`. Resizing the OS window changes only **local preview scale**; it must not send `ViewportChanged` or mutate PaperSpoon's widget layout. An explicit future viewport control may deliberately renegotiate it.
7. **Preview, not hardware emulation:** PaperView represents a digital pixel-perfect PPFB client. Any physical e-ink timing, waveform, and ghosting claims remain device-only.
8. **No surprise host actions:** initial MVP is read-only. Pointer-event simulation may be enabled later with an explicit opt-in; never synthesize presses on startup or reconnect.

## Proposed layout

```text
tools/paperview/
  Cargo.toml
  src/
    main.rs            # CLI and application startup
    connection.rs      # TCP lifecycle, Hello, bounded PPFB receive loop
    framebuffer.rs     # Validated frame -> presentable RGB buffer
    window.rs          # winit state, softbuffer surface, redraw/resize
    viewport.rs        # Fit/zoom and coordinate transforms
    input.rs           # Optional, only after read-only preview is stable
  tests/               # Integration fixtures as needed
```

Names are suggestions, not a mandate to add empty abstractions. Keep pure conversion and geometry functions testable without a GUI, display server, or live PaperSpoon process.

## Stage 0 — Baseline and dependency spike

- [ ] Read `AGENTS.md`, `README.md`, existing `docs/` plans, workspace manifests, PaperSpoon's `main.rs`, `server.rs`, and the `paper-protocol` v2 codec/framebuffer APIs.
- [ ] Inspect the complete PaperSpoon session flow: what the initial `Hello` triggers, which messages it sends to a receiver, how it handles a disconnect, and what happens when a new client replaces an old one.
- [ ] Confirm current stable, mutually compatible `winit`/`softbuffer` versions and the supported application-event APIs; record the chosen versions in `Cargo.lock`. Prefer a maintained `ApplicationHandler`-style API if supported.
- [ ] Verify that a minimal native window displaying a synthetic RGB checkerboard compiles and can present on the development macOS machine without an OpenGL/Vulkan/wgpu dependency.
- [ ] Confirm the build expectations and required system libraries for Linux X11/Wayland and Windows. Do not claim platforms tested solely because the Rust code is nominally portable.
- [ ] Confirm what `softbuffer` expects for color packing (`0x00RRGGBB`) and pixel dimensions; record where OS scale factors enter.

**Gate:** The chosen window backend can present a locally generated pixel buffer. The server handshake and incoming frame format are understood. No PPFB or PaperPad modifications have been made.

## Stage 1 — Read-only PPFB client and validated frames

- [ ] Add `paperview` as a workspace member and make `cargo run -p paperview -- --host 127.0.0.1 --port 5581 --viewport 1272x1624` the documented developer entry point. Parse errors, invalid ports, and zero/oversized viewports must fail clearly.
- [ ] Use an explicit host and port; do not require UDP discovery. Default to loopback and the existing PaperSpoon TCP port.
- [ ] Connect with a bounded timeout and send one `V2Hello::with_pixel_formats()` advertising **both** Mono1 and Gray8. Use existing protocol encoders.
- [ ] Read PPFB header first, validate declared length **before allocating**, then read payload. Reuse `decode_v2_message` and `decode_v2_payload`; reject malformed, unknown, unsupported, or out-of-order message types according to the server-to-client contract.
- [ ] Accept only complete, validated frame payloads. Verify format is advertised and dimensions equal the negotiated viewport. Validate length and stride using `paper-protocol`; do not trust raw metadata for offsets or allocations.
- [ ] Hold the last successfully received frame in application state. An invalid replacement must never partially overwrite it.
- [ ] Keep network I/O off the GUI thread. Bound pending decoded frames to one latest-frame slot; ensure close/shutdown cannot hang behind a blocking read forever.
- [ ] Expose connection status (connecting / connected / disconnected / error) in the window title and/or a separate non-frame status area, not drawn into the source frame.
- [ ] On failure or EOF, keep the last valid frame visible and reconnect with a bounded delay. A new session sends a new Hello; discard partial data from the old session. Gracefully handle PaperSpoon not running.
- [ ] If PaperSpoon already sends its retained authoritative frame on reconnect, verify that behavior end-to-end. Do not add a parallel frame-history mechanism to PaperView.

**Gate:** PaperView connects to a real PaperSpoon over localhost, receives Mono1 and Gray8 frames using PPFB v2, survives malformed/disconnected sessions, and never constructs an unbounded frame backlog.

## Stage 2 — Pixel-perfect native window

- [ ] Implement a pure, tested conversion from the two validated source formats to displayable `0x00RRGGBB` pixels.
- [ ] Mono1 conversion must use row-major MSB-first bits, `0 => white`, `1 => black`, correctly handling non-multiple-of-eight widths and excluding row padding.
- [ ] Gray8 conversion must map each byte `v` to `(v, v, v)` without thresholding, dithering, color management changes, or font rerendering.
- [ ] Create `winit` window and `softbuffer` surface on the UI thread. Resize or recreate presentation buffers correctly on window-size and scale-factor changes; skip drawing for a zero-sized/minimized surface.
- [ ] Implement **Fit to window** with aspect-ratio preservation and neutral letterboxing. Initial window size should fit on a typical laptop display rather than assuming the full 1272x1624 viewport is visible at 1:1.
- [ ] Implement 1:1 physical-pixel inspection and convenient integer zoom (e.g., 2x/4x), with nearest-neighbor sampling. If the image exceeds window dimensions, expose panning/scrolling or another predictable way to inspect the full source, without silently cropping critical areas.
- [ ] Use physical window pixel dimensions rather than logical DPI-independent coordinates for the final presentation. Avoid extra blur from OS scaling where practical; document limitations per OS.
- [ ] Request redraw only on a new valid frame, resize, zoom/pan, expose/restore, or connection-state UI update. Do not introduce a 60 FPS busy loop.
- [ ] Prevent allocation overflow (`width * height * bytes_per_pixel`) and excessive or repeated full-frame copies. Reuse buffers when dimensions are unchanged.
- [ ] Preserve the last valid image across transient network failures and window expose events. No stale data may be presented after an incompatible viewport change.

**Gate:** Every source pixel maps predictably to displayed colors for both formats. The preview remains responsive to resizing and does not continuously redraw unchanged content.

## Stage 3 — Now Playing development workflow

- [ ] Document a reproducible local workflow using two terminals: start PaperSpoon, start PaperView, enter `frame nowplaying` in PaperSpoon. Preserve `--no-forward-url` for safe UI experimentation.
- [ ] Show a Gray8 Now Playing frame at fitted size, then inspect text antialiasing at 1:1 and 2x/4x without introducing another text-rendering path.
- [ ] Verify that switching between `ui WIDTHxHEIGHT`, `frame gray-gradient WIDTHxHEIGHT`, `frame gray-bars WIDTHxHEIGHT`, and `frame nowplaying` updates the same window and respects negotiated frame formats. Use the **current negotiated** viewport in each command.
- [ ] Add an explicit Save source frame action (e.g., keyboard shortcut `S`) to export the original Mono1/Gray8 pixels to PNG. Do **not** save a screenshot of the zoomed/scaled window as the canonical fixture. Use a small PNG dependency only at this stage if necessary.
- [ ] For export, retain exact source dimensions and grayscale polarity. Name output deterministically when a path is supplied; handle nonexistent/unwritable paths and do not overwrite existing files without an explicit policy.
- [ ] Add a small set of checked-in synthetic **protocol/frame** fixtures for black/white, non-byte-aligned Mono1, Gray8 gradients, and expected Now Playing viewport geometry. Keep actual user music data and personal screenshots untracked.
- [ ] Document that real `frame nowplaying` depends on a working macOS media provider. On Linux/Windows, PaperView can display a frame sent by PaperSpoon running on a reachable Mac; PaperView does not acquire media metadata itself.
- [ ] If deterministic fake Now Playing metadata is needed, propose a separate PaperSpoon-side fixture-rendering task; **do not** duplicate its layout in PaperView or silently expand this plan.

**Gate:** A developer can inspect a real Now Playing Gray8 frame, save its exact source pixels, and repeat layout comparisons without a Kindle.

## Stage 4 — Optional mouse/touch emulation (after the viewer is stable)

- [ ] Add an **opt-in** mode (for example, `--interactive`; default is read-only). Distinguish this from drag-to-pan/zoom UI operations.
- [ ] Convert mouse coordinates through the **inverse** of the exact preview transform (window physical pixels -> image pixels), accounting for DPI, letterboxing, pan, and zoom. Ignore events outside the viewport.
- [ ] Send only existing PPFB `V2Pointer` Down/Up messages, with primary-button semantics. Do not add semantic action IDs or duplicate application button geometry.
- [ ] Guarantee at most one matched Up per Down and avoid accidental actions on focus loss, outside release, disconnect, or reconnect. Document cancellation behavior and ensure protocol expectations are met.
- [ ] For manual tests, launch PaperSpoon with `--no-forward-url` so clicking application buttons never triggers unexpected Hammerspoon actions.
- [ ] Test the mapping at fit scale and multiple zoom levels, near edges, with letterboxing, and under high-DPI settings.

**Gate:** With explicit opt-in, a window click acts like a viewport-relative PaperPad touch without changing PPFB or PaperSpoon hit-testing. Read-only behavior remains the default.

## Verification

### Automated checks

- [x] `cargo fmt --all -- --check`
- [x] `cargo test -p paper-protocol -p paperview` and applicable workspace tests.
- [x] `cargo clippy -p paperview --all-targets -- -D warnings` (record unrelated existing workspace issues, if any).
- [x] `cargo check -p paperview` on the development host; where available, check native builds on macOS, Linux, and Windows rather than claiming untested support.
- [x] `make check` (or the current equivalent specified by `AGENTS.md`) and `git diff --check` to catch regressions outside the new crate.
- [ ] Pure unit tests: Mono1 black/white polarity, MSB bit order, each-row padding, odd widths, Gray8 endpoints/midtones, correct channel packing, stride and payload bounds.
- [ ] Pure geometry tests: aspect fit, centered letterboxing, high-DPI transform, 1:1 physical-pixel mapping, zoom/pan bounds, inverse pointer mapping, zero-sized windows.
- [ ] Protocol tests: split TCP reads, partial headers, truncated/oversized frame, invalid format, zero/out-of-range dimensions, unsupported messages, slow presentation, reconnect and stale-session rejection.
- [ ] Window tests where a display is available; keep codec/conversion tests runnable without a display server. Use a virtual display on Linux CI only when necessary.
- [ ] Test clean exit while socket reads are blocked and repeated reconnect without runaway workers or memory growth.

### Manual checks

- [x] macOS: receive `ui 1272x1624`, `frame gray-gradient 1272x1624`, `frame gray-bars 1272x1624`, and `frame nowplaying` from a local PaperSpoon. Verify source dimensions and grayscale export, zoom, panning, clipping, and scaling.
- [ ] Linux: skipped on 2026-11-26 because no Linux environment was available; X11 and Wayland remain unverified.
- [ ] Windows: skipped on 2026-11-26 because no Windows environment was available; native window, resize/zoom, and reconnect behavior remain unverified.
- [ ] Stop PaperSpoon abruptly: the window stays responsive, indicates disconnection, retains the last correct frame, and reconnects when the server restarts.
- [ ] Demonstrate single-session behavior when Kindle was previously connected; do not claim simultaneous PaperPad/PaperView output.
- [x] Verify that no Kindle binary was rebuilt or deployed as part of PaperView-only changes.

## Acceptance criteria

1. `cargo run -p paperview -- --host 127.0.0.1 --port 5581 --viewport 1272x1624` launches a native, resizable window on a supported desktop platform.
2. A PPFB v2 session advertises Mono1 and Gray8, consumes only validated frames, and never bypasses `paper-protocol` frame invariants.
3. Mono1 and Gray8 display with exact stated polarities. The Now Playing OpenType text is received and displayed unchanged as source pixels.
4. The viewport remains stable when the preview window resizes or changes zoom; no unintended layout reflow occurs.
5. Network waits do not block OS window events; no-frame idle work is event-driven and the pending frame queue is bounded.
6. Loss of connection does not crash the program or replace the last valid image with partial data; reconnect restores a valid session.
7. A developer can inspect pixels at 1:1, zoom in, and export an exact source-frame PNG (Stage 3).
8. Existing PaperPad code, protocol wire format, MXCFB path, device-local controls, and OpenType engine remain untouched.
9. Cross-platform claims specify which actual OS/window-system combinations were built and manually exercised. Kindle physical behavior is **not** claimed to be emulated.
10. Documentation states the single-active-client limitation; multi-client PaperSpoon support remains explicitly deferred.

## Risks and future work (outside MVP)

- 💡 **Simultaneous monitor and Kindle:** move PaperSpoon from `CurrentConnection` to a deliberately designed multi-client session model, including per-client viewport/capability state and safe frame fan-out. This requires its own design and tests.
- 💡 **Deterministic Now Playing fixtures:** expose a host-side test input with long titles, Unicode, empty values, missing artwork, and unusual aspect ratios. Keep metadata/layout generation in PaperSpoon.
- 💡 **Visual regression testing:** compare exported source PNGs against pinned fixtures with explicit tolerance policies for grayscale/text rasterization. Avoid using zoomed OS screenshots as golden images.
- 💡 **Dirty-region diagnostics:** visualize changes between successive frames as an optional overlay, but keep the baseline pixel-perfect image accessible and unchanged.
- 💡 **E-ink behavior simulation:** experiment with ghosting and waveform models as explicitly approximate visual effects only after observing physical device behavior; never confuse a simulation with hardware verification.
- 💡 **Cross-platform package distribution:** only add installers, release automation, and OS-specific integration once local builds and dependencies are verified on each OS.

## Codex execution instructions

Implement stages sequentially, treating each **Gate** as a stopping point for verification. Reuse repository patterns and the existing PPFB codec, keep changes localized, and do not create placeholder modules without meaningful content. At each gate report changed files, exact commands/tests run, whether results passed, any untested target OS, and unresolved tradeoffs. Prioritize a working read-only preview over speculative abstraction. Follow `AGENTS.md`; this plan does **not** authorize Kindle deployment because the intended changes are strictly host-only. If Stage 0 reveals incompatible API versions or different repository behavior, revise the plan's implementation details rather than assuming an outdated signature is correct.

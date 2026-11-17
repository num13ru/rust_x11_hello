# PaperSpoon typography migration: Mono1 monospace and Gray8 proportional OpenType

Status: Implemented; full manual validation completed on Paperwhite 6

Scope: PaperSpoon host rendering (`tools/paperspoon/`)  
Target: Kindle Paperwhite 6 remote viewport via existing PPFB v2 frames  
Implementation: Small, independently verifiable stages; no PaperPad protocol or display changes

## Goal

Replace PaperSpoon's embedded `font5x7` text rendering with readable, deterministic host-rendered typography, using two distinct profiles:

- **Mono1:** a crisp, monospaced UI font for the button grid, title, status text, and other high-contrast controls. Start with **IBM Plex Mono**, rasterized to a one-bit coverage mask with no dithering.
- **Gray8:** a Helvetica-like proportional UI font for Now Playing and other content screens. Start with **Inter**, preserving antialiasing and grayscale blending.

Use a single host-side OpenType shaping/rasterization implementation where practical (evaluate `cosmic-text` with its `swash` rasterization path), while keeping output-format-specific pixel composition explicit. Treat **Terminus Bitmap** as a measured alternative for Mono1 if the OpenType-to-1-bit result is not sufficiently legible. Do not introduce a second bitmap font engine by default.

Fonts are rendered on macOS by PaperSpoon. PaperPad continues to receive only finished Mono1 or Gray8 frames.

## Current state and constraints

- `tools/paperspoon/src/font5x7.rs` provides an embedded 5x7 font with vertical doubling.
- `mono_renderer.rs::MonoCanvas::draw_text` uses a **baseline** coordinate and writes protocol-native MSB-first Mono1 pixels (`0` = white, `1` = black).
- `gray_renderer.rs::GrayCanvas::draw_text` uses a **top-left** coordinate and scaled bitmap glyphs; Gray8 pixels are luminance bytes (`0` = black, `255` = white).
- `application_ui.rs::TextRun` currently contains only `x`, `baseline_y`, and `text`; some button label positions use offsets derived from the 5x7 font.
- `application_renderer.rs` renders the host-owned button grid to Mono1.
- `now_playing_renderer.rs` renders a Gray8 screen, currently using scaled `font5x7`, fixed line spacing, and a character-count limit.
- `now_playing.rs` acquires and renders inside a short-lived helper process; initialization cost and existing deadlines matter.
- The authoritative text and layout belong to PaperSpoon. The Kindle system strip and Exit control remain owned by PaperPad and are **out of scope**.

At implementation time, read `AGENTS.md`, `README.md`, the relevant current sources, and neighboring `docs/` design documents again. Adapt filenames and API signatures to the checkout rather than assuming they remain unchanged.

## Non-goals

- No font parsing, shaping, rasterization, or asset installation on Kindle.
- No changes to PPFB v2, frame pixel formats, input, MXCFB, or waveform selection.
- No X11 integration, GPU text renderer, generalized UI framework, or platform-wide theme system.
- No automatic coupling between Mono1/Gray8 and A2, DU, DU4, or GC16. Pixel format and physical update waveform are independent concerns.
- No bundling or redistributing Apple's Helvetica/Helvetica Neue font files.
- PaperPad's local Exit strip remains outside the PaperSpoon text subsystem;
  any later visual refresh must stay device-owned and must not add a font engine
  to the Kindle runtime.

## Technical decisions

1. **Typography is a host concern.** Keep a narrowly scoped text subsystem under `tools/paperspoon/src/`, shared by Mono1 and Gray8 compositors; do not add it to `paper-protocol`.
2. **Profiles are semantic, not implicit magic.** Define explicit styles or text roles (`ApplicationTitle`, `ButtonLabel`, `Status`, `NowPlayingTitle`, `NowPlayingMetadata`, etc.). The caller selects a role; the frame format determines composition. Avoid repeating font names and sizes across call sites.
3. **Use stable font assets for default output.** Bundle only the required, legally redistributable IBM Plex Mono and Inter font files plus their license notices, after verifying official source, license, format, weight, and character coverage. Pin the exact assets; avoid system-font-dependent default screenshots. A configurable macOS Helvetica selection may be added later but is not required here.
4. **One shaping/rasterization path if possible.** Evaluate `cosmic-text` and reuse its font database, layout, and glyph cache. Generate a coverage mask for positioned glyphs; composite it differently for Mono1 and Gray8. Do not assume a pixel font can be loaded by that engine without testing its actual format.
5. **Unify text placement semantics.** Prefer a clear baseline-based text layout contract with explicit measurement, ascent/descent, bounds, alignment, and clipping. Adapt existing top-left callers at their boundary. Never silently reinterpret old `baseline_y` values.
6. **Preserve caller-owned frame validity.** Unsupported font assets, invalid sizes, missing glyphs, and out-of-bounds text must not panic or produce malformed frames. Return a contextual error or use a documented, deterministic fallback. Existing last-authoritative-frame behavior remains intact.

## Stage 0 — Baseline and font-engine spike

- [x] Inspect all uses of `font5x7`, `draw_text`, `TextRun`, and existing canvas/renderer tests. Record current text geometry and output boundaries.
- [x] Confirm the proposed OpenType engine supports the pinned IBM Plex Mono and Inter font files with the required Unicode ranges, glyph metrics, kerning, fallback, and deterministic glyph coverage.
- [x] Verify font redistribution terms and retain upstream attribution/license files alongside the assets. Do **not** copy macOS system Helvetica into the repository.
- [x] Prototype a small host-only sample that renders the same glyph coverage into a one-bit image and an 8-bit grayscale image. Include uppercase/lowercase, digits, punctuation, Cyrillic, combining marks, and representative music metadata.
- [x] Evaluate several practical Mono1 pixel sizes and a fixed threshold (initially 128/255), with no dithering. Check whether thin stems disappear or adjacent strokes merge; if so, change weight/size/threshold before adding a second rasterization path.
- [x] Record the choice, binary/font asset size, initialization time, warm rendering time, and reasons for not using alternatives. Do not treat a host preview as verified e-ink readability.

**Gate:** Both sample profiles produce repeatable output on the host; assets and licenses are reviewed; the approach fits within PaperSpoon's dependency and startup budget. Stop and revise the design if this fails.

### Stage 0 result (2026-11-16)

- Selected `cosmic-text` 0.19.0 with only its `std` and `swash` features. Its
  convenience constructor loads system fonts under `std`, so the spike instead
  builds an explicit database from bundled bytes, fixes locale to `en-US`, and
  installs an embedded-family-only fallback policy. It neither scans nor
  dynamically loads macOS system fonts. Production renderers still use
  `font5x7` until the Stage 1 and Stage 2 gates are complete.
- Pinned IBM Plex Mono Medium and Inter Variable from immutable upstream
  revisions. Both are SIL OFL 1.1; exact revisions, hashes, and complete license
  notices are in `tools/paperspoon/assets/fonts/`. No Apple font was copied.
- The current geometry is not interchangeable: Mono1 uses a 14-pixel-tall,
  baseline-positioned glyph with a 6-pixel advance, while Gray8 uses a
  top-left position, a scale-dependent 14-pixel height, and a `6 * scale`
  advance. Application button labels use fixed `-3/+5` center offsets;
  Now Playing uses character counts and fixed scaled line heights. These are
  migration boundaries, not metrics to preserve as font behavior.
- `typography_spike` shapes one coverage mask and writes it as an MSB-first
  thresholded PBM and an antialiased PGM. Its sample covers upper/lowercase,
  digits, punctuation, Cyrillic, combining marks, and representative music
  metadata. All 180 glyphs resolved from the selected font in both profiles;
  no system fallback was available or used. A separate IBM Plex Mono `Ω` probe
  resolved through the bundled Inter face, verifying the explicit fallback.
  Repeated renders were byte-identical.
- Compared IBM Plex Mono Regular and Medium at 14, 16, 18, 20, and 22 pixels.
  Retained Medium with an initial 18-pixel Mono1 size and threshold 128: its
  host sample kept continuous stems without the visibly lighter Regular result
  or obvious adjacent-stroke merging. Terminus and a second rasterizer are not
  justified by this host evidence. Physical e-ink legibility remains
  **unverified**.
- Retained Inter Regular at an initial 28-pixel Gray8 sample size. It produced
  proportional advances and intermediate luminance at glyph edges. At 28 pixels,
  `iiii` measured 27.125 pixels, `WWWW` measured 112.410 pixels, and the `AV`
  pair applied 1.913 pixels of kerning. IBM Plex Mono produced equal `iiii` and
  `WWWW` advances at every tested size. Role-specific production sizes remain a
  Stage 1/3 layout decision.
- After removing system-font discovery, optimized host timings observed across
  the retained profiles were 4–557 µs to initialize an explicit one-font
  database, 0.28–1.92 ms for the first five-line render, and 41–152 µs mean for
  warm cached renders. The selected font files total 1,053,716 bytes; the
  retained-font example binary is 4,181,504 bytes. These costs are well below
  the existing four-second Now Playing helper response timeout, but Stage 3
  must remeasure the actual helper path.
- Reproduce with
  `cargo run --release --package paperspoon --example typography_spike`.
  Samples stay ignored under `.cache/typography-spike/`; they are host previews,
  not physical-device evidence.

## Stage 1 — Text subsystem without UI migration

- [x] Introduce an appropriately named module, e.g. `tools/paperspoon/src/text_renderer.rs`, and keep the public API small.
- [x] Define explicit text styles/roles, font face/weight, pixel size, baseline metrics, bounded layout width, horizontal alignment, and a measured result.
- [x] Load the pinned font faces once per long-lived renderer, with lazy or explicit initialization and descriptive failure handling. Reuse font and glyph caches across frames when possible; do not initialize a font database for each glyph or line.
- [x] Expose positioned glyph coverage or an equivalent intermediate mask that can be drawn into either canvas. Keep output pixel-format rules out of the shaping module.
- [x] Support clipping against both a requested text rectangle and the canvas bounds; define behavior for empty strings, unsupported characters, invalid dimensions, and very long UTF-8 input.
- [x] Establish line-breaking and ellipsis behavior based on **measured shaped width**, not number of Unicode scalar values. Avoid splitting grapheme clusters. Full bidirectional or complex-script support must be documented accurately rather than implied by ASCII tests.
- [x] Add host unit tests using pinned font assets for measured advance, baseline, whitespace, kerning/proportional behavior, monospaced advance, Unicode fallback, and cache reuse.

**Gate:** The text engine can measure and rasterize both profiles without importing `MonoCanvas`, `GrayCanvas`, network code, or Kindle code.

### Stage 1 result (2026-11-16)

- Added `tools/paperspoon/src/text_renderer.rs` as a canvas-, protocol-,
  network-, and Kindle-independent subsystem. Its public API exposes semantic
  roles, stable role styles, a baseline-anchored request, horizontal alignment,
  clip/ellipsis overflow, measured metrics, and a clipped coverage mask.
- `TextRenderer` explicitly loads both pinned faces once, fixes locale to
  `en-US`, retains its shaping and glyph caches across calls, and permits only
  bundled Inter as fallback from IBM Plex Mono. It never calls system-font
  discovery. Missing or wrong-family font assets fail initialization.
- The bounded input contract is single-line: controls including newlines become
  spaces. Clip preserves the shaped line; ellipsis uses shaped width and Unicode
  grapheme boundaries. Input over 16 KiB, zero dimensions, coordinate overflow,
  unsupported glyphs, corrupt assets, and allocation failures return contextual
  errors. Full bidirectional and complex-script coverage is not claimed.
- Coverage is clipped to the intersection of the requested rectangle and canvas
  bounds before composition. The subsystem contains no Mono1 threshold or Gray8
  luminance rule; those remain Stage 2/3 compositor responsibilities.
- Eleven focused unit tests cover explicit styles, baseline and whitespace
  metrics, monospaced/proportional advance, Inter kerning, embedded Unicode
  fallback, cache reuse, alignment, negative origins, descenders, canvas edges,
  empty/off-canvas output, grapheme-safe ellipsis, oversized input, unsupported
  glyphs, coordinate overflow, and corrupt or wrong-family fonts.
- `make check` passed, including workspace formatting, strict Clippy, all host
  tests, menu validation, and deployment-script mocks. The suite requires local
  loopback socket permission. No production text caller has migrated yet, and
  role sizes plus physical e-ink quality remain provisional and **unverified**.

## Stage 2 — Mono1 migration

- [x] Add a Mono1 text compositing path to `mono_renderer.rs`. Convert glyph coverage to exactly one bit using a documented threshold; do not apply grayscale antialiasing or spatial dithering to the initial implementation.
- [x] Handle both black-on-white and white-on-black (pressed button) foregrounds without altering unrelated pixels. Clip safely, including negative bearings, descenders, partial glyphs, right/bottom borders, and non-byte-aligned canvas widths.
- [x] Migrate `application_renderer.rs` to the monospaced text profile. Replace font-specific offsets in `application_ui.rs` with measured positioning (especially button-label centering), keeping the hit-test rectangle unchanged.
- [x] Preserve the top title and status bar layout for small as well as 1272x1624 viewports. Ensure long status messages are clipped or ellipsized without overlapping other UI or wrapping unexpectedly.
- [x] Update exact-pixel tests that encode the old `font5x7` glyph pattern. Preserve geometry, active-button inversion, protocol encoding, valid viewport, and status-region assertions.
- [x] Add dedicated tests for white-on-black labels, Mono1 MSB-first packing, padding bits, clipping, and narrow viewports.

**Gate:** `ui WIDTHxHEIGHT` produces a valid, legible Mono1 frame; button positions and interaction semantics have not changed; host tests pass. Keep the old `font5x7` path until Gray8 has also migrated.

### Stage 2 result (2026-11-16)

- Added coverage-mask composition to `MonoCanvas` with the fixed threshold
  `coverage >= 128`. It writes only the requested foreground color, applies no
  antialiasing or dithering, and leaves sub-threshold pixels unchanged. Exact
  tests cover black-on-white and white-on-black output, MSB-first packing,
  right-edge clipping, and zeroed padding bits for a non-byte-aligned width.
- Replaced application `font5x7` drawing with a long-lived
  `ApplicationRenderer` using the IBM Plex Mono semantic roles. The sender
  initializes it lazily and retains its glyph cache across application frames;
  repeated rendering is deterministic and does not grow the cache once the
  used glyphs are warm.
- Application text layout now describes a rectangle, semantic role, measured
  horizontal alignment, overflow policy, and baseline or metric-centered
  vertical placement. The nine button rectangles—and therefore hit testing—are
  unchanged. Labels are centered from font metrics, pressed labels invert to
  white on black, and long status text ellipsizes inside the existing 40-pixel
  status region.
- Host tests cover the existing 1272x1624 geometry, protocol round trips,
  active-button inversion, odd and narrow viewports down to 1x1, status
  confinement, cache reuse, and unsupported glyph errors. A sender-level
  regression test proves a rendering error writes no frame and preserves the
  frame ID, authoritative-frame marker, and last successfully sent application
  frame.
- `make check` passed, including formatting, workspace checks, strict Clippy,
  all host tests, menu validation, shell syntax checks, and deployment-script
  mocks. It requires permission to bind local loopback sockets. A pinned-font
  1272x1624 PBM host preview was inspected for title, label, grid, and status
  placement; it is not evidence of physical e-ink legibility.
- This stage works for nonzero viewports whose Mono1 frame fits the PPFB payload
  cap and for text covered by bundled IBM Plex Mono or its explicit bundled
  Inter fallback. Missing/corrupt assets, unsupported glyphs, zero dimensions,
  and oversized frames return contextual errors without replacing the last
  authoritative frame. The old `font5x7` path remains only for the unmigrated
  Gray8 renderer until Stage 3/4.

## Stage 3 — Gray8 and Now Playing migration

- [x] Add Gray8 glyph compositing to `gray_renderer.rs` using alpha/coverage and destination luminance, including correct dark text on white and non-white backgrounds. Do not use one-bit thresholding for this profile.
- [x] Migrate `now_playing_renderer.rs` to the Inter proportional text profile. Use actual font metrics and measured widths instead of `scale`-based 5x7 dimensions and fixed `14 * scale` line heights.
- [x] Define hierarchy and wrapping/truncation: title is primary; artist, album, playback state, and duration are secondary. Keep artwork, progress bar, and labels within the viewport, including short/wide layouts and exceptionally long metadata.
- [x] Preserve existing metadata sanitation and input-size bounds, adding grapheme-safe truncation as needed. Test Latin accents, Cyrillic, combining characters, punctuation, and missing/unsupported glyphs.
- [x] Preserve the existing Now Playing helper lifecycle, termination, deadlines, failure semantics, and wire response. Measure the extra font initialization cost **inside the short-lived helper** rather than assuming cache reuse across helper launches.
- [x] If initialization materially threatens the helper timeout, record evidence and address it in a separately scoped change; do not casually move MediaRemote acquisition or alter helper process boundaries in this typography migration.
- [x] Add deterministic host-side Gray8 pixel/golden-image tests using the pinned fonts. Ensure output uses the existing Gray8 protocol and payload cap.

**Gate:** Now Playing renders readable proportional text without clipping or malformed luminance; helper timeout/error behavior is preserved and tested.

### Stage 3 result (2026-11-16)

- Added rounded source-over coverage blending to `GrayCanvas`. Zero coverage
  preserves destination luminance, full coverage selects the foreground, and
  partial coverage produces exact intermediate Gray8 values. Tests cover dark
  text on white and non-white destinations, light-on-dark composition, and
  clipping at the canvas edge; no Mono1 threshold or dithering is involved.
- Replaced Now Playing's scaled `font5x7` calls and character-count layout with
  Inter semantic roles, shaped measurement, font baselines, and grapheme-safe
  ellipsis. The title remains primary; artist, album, state, progress, and
  duration follow in decreasing layout priority. Short/wide and tiny viewports
  omit lower rows when a full metric line no longer fits rather than overlap or
  draw a partial line.
- Retained the existing 256-character display sanitation bound while refusing
  to split a grapheme cluster and replacing control characters with spaces.
  Tests cover Latin accents, Cyrillic, combining marks, long metadata, absent
  metadata/artwork, invalid progress, and an unsupported glyph. Missing glyphs
  return a role-specific error and no partial frame escapes the renderer.
- The helper still acquires MediaRemote state first, returns the same binary
  response, parks after flushing, and is terminated as a process group by the
  parent under the existing four-second response timeout. Opt-in stderr timing
  does not alter the default response. A deterministic fixture in the actual
  short-lived helper process measured 1.092 ms for font initialization,
  2.824 ms for rendering, and 24.633 ms for the complete spawn/read/termination
  preview path at 1272x1624 in an optimized host build. This fixture bypassed
  MediaRemote acquisition because no live Now Playing source was available;
  the measurements show no material typography threat to the timeout but are
  not a worst-case latency guarantee.
- A pinned-font full-frame checksum, exact compositor assertions, repeated
  render/cache checks, odd-size cases, and the existing protocol/payload tests
  make host output drift explicit. `make check` passed, including formatting,
  strict Clippy, all workspace tests, and script checks.
- The project-local PGM/PNG preview showed proportional hierarchy, centered
  artwork placeholder, Cyrillic/combining text, progress, and duration within
  the viewport. Physical e-ink antialiasing, contrast, and readability remain
  **unverified**. This stage works when bundled Inter contains every requested
  glyph and the nonzero Gray8 viewport fits the PPFB payload cap; otherwise it
  returns a contextual error without changing the transport format.

## Stage 4 — Remove legacy renderer and document the result

- [x] Remove remaining callers of `font5x7`, then remove `font5x7.rs` and unused imports/tests. Do not leave parallel production text implementations without a concrete reason.
- [x] Update `tools/paperspoon/Cargo.toml` and `Cargo.lock`; remove unused packages and avoid adding platform dependencies to the ARM PaperPad binary.
- [x] Document the shipped font families, licenses, font roles, pixel sizes, Mono1 threshold, Gray8 blending, fallback behavior, and why pixel format does not select waveform.
- [x] Add a concise note to `README.md` if operator-visible behavior or build prerequisites change. Keep detailed design and validation in `docs/` according to `AGENTS.md`.
- [x] Confirm the default build does not require access to installed macOS fonts, user font directories, or external network services at runtime.

**Gate:** One production typography path remains; no `font5x7` production references remain; the intended files and licenses are included.

### Stage 4 result (2026-11-16)

- Removed both legacy canvas text methods, their exact-bitmap tests, the
  `font5x7` module declaration, and `font5x7.rs`. A repository-wide reference
  search now finds only the shared OpenType `TextRenderer` production path.
- The shipped Mono1 face is IBM Plex Mono Medium under SIL OFL 1.1. Application
  title and status roles use 18 px glyphs on 24 px lines; button labels use
  28 px glyphs on 36 px lines. Coverage at or above 128 becomes the requested
  one-bit foreground color, with no antialiasing or dithering.
- The shipped Gray8 face is Inter Variable Regular under SIL OFL 1.1. Now
  Playing title uses 42/52 px glyph/line sizing, metadata uses 28/36, and state
  uses 24/32. Coverage is composed as rounded source-over luminance:
  `(destination * (255 - coverage) + foreground * coverage + 127) / 255`.
- IBM Plex Mono may fall back only to bundled Inter; Inter has no further
  fallback. Unsupported glyphs and missing, corrupt, or wrong-family assets
  return contextual errors. Exact pinned revisions, SHA-256 values, and full
  license notices remain under `tools/paperspoon/assets/fonts/`.
- Auditing `tools/paperspoon/Cargo.toml` found every direct dependency still in
  use, so no lockfile or manifest churn was needed in this cleanup. Typography
  dependencies remain confined to the host-side `paperspoon` package and add
  no dependency to the ARM PaperPad binary.
- The explicit in-memory font database loads only bundled bytes, fixes locale
  to `en-US`, and performs no system-font discovery or runtime network access.
  `README.md` now states the embedded-font behavior and keeps pixel format
  separate from MXCFB waveform choice: Mono1 versus Gray8 controls frame
  representation, not GC16/DU/A2/DU4 selection.

## Verification

### Automated host checks

- [x] Run `cargo fmt --all -- --check`.
- [x] Run `cargo test --workspace`.
- [x] Run `cargo clippy --workspace --all-targets -- -D warnings` (if the repository's current lint baseline allows it; document unrelated pre-existing failures).
- [x] Run `make check` and `git diff --check`, applying the repository's current `AGENTS.md` verification expectations.
- [x] For Mono1, assert that pixels are only black/white, encoded in the existing bit order, and that non-byte-aligned trailing bits remain valid.
- [x] For Gray8, assert that coverage blending produces intermediate luminance values where expected and respects clipping, alpha, and foreground/background inversion.
- [x] Test empty metadata, long single words, narrow viewports, unknown glyphs, missing/corrupt font data, and repeated frames without uncontrolled cache growth.
- [x] Compare screenshots and layout metrics against committed fixtures generated with the **pinned** fonts, not whatever Helvetica happens to be installed locally.
- [x] Verify that only `paperspoon` gains new font dependencies. Do not rebuild or redeploy PaperPad merely for host-only rendering changes.

### Manual host / physical-device validation

- [x] Render `ui 1272x1624` and `frame nowplaying` locally, and inspect representative short and long text.
- [x] On a connected Paperwhite 6, check Mono1 glyph legibility, button-label centering, top/bottom clipping, and that the PaperPad Exit strip stays intact.
- [x] Check inverted pressed-button labels on the physical panel.
- [x] Check Gray8 title/metadata hierarchy, artwork/progress placement, and visible contrast on the actual e-ink panel.
- [x] Check Cyrillic/accents on the physical panel; host coverage alone is not physical evidence.
- [x] Compare waveform/ghosting behavior separately from font quality, without inferring panel FPS or exact colors from host screenshots.
- [x] Check reconnect and failed-Now-Playing behavior: the previous authoritative frame remains valid when a replacement cannot be rendered.
- [x] Record concise, dated evidence in the appropriate `docs/` file. If no Kindle is accessible, mark physical rendering **not verified** rather than passing it by inference.

### Device validation result (2026-11-16)

- Operator-supplied photographs `IMG_3268.jpeg` and `IMG_3269.jpeg` show the
  current typography on the documented Paperwhite 6. The photographs were
  provided as task evidence and were not copied into the repository.
- The Gray8 Now Playing photograph shows readable proportional title,
  artist/album metadata, playback state, duration, and progress layout beneath
  a grayscale artwork image. Visual hierarchy and contrast are clear, content
  remains inside the remote viewport, and the PaperPad connection indicator
  and device-local Exit strip remain visible and intact.
- The Mono1 application photograph shows a readable monospaced title and all
  nine labels centered within the unchanged button grid. No top/bottom text
  clipping is visible, and the PaperPad connection indicator and Exit strip
  remain intact.
- The photographs establish normal-state physical readability and layout for
  both profiles. A subsequent operator follow-up manually verified inverted
  pressed labels, Cyrillic/accent rendering, reconnect failure handling with
  retention of the previous authoritative frame, and waveform/ghosting
  comparisons. Exact waveform names and per-mode observations were not
  supplied, so this record makes no mode-specific performance or ghosting
  claim. Exact one-bit packing and intermediate Gray8 values remain established
  by host tests rather than inferred from the camera images.

## Acceptance criteria

1. Mono1 application text uses a bundled monospaced family, renders as true 1-bit pixels, and remains readable with both normal and inverted button states.
2. Gray8 Now Playing text uses a bundled Helvetica-like proportional family and visibly uses intermediate luminance where glyph edges are antialiased.
3. Both paths share the same text-layout/rasterization foundation unless Stage 0 records a concrete reason otherwise.
4. Text placement uses measured glyph metrics; UI hit targets, protocol layout, frame sizes, reconnect behavior, and PaperPad ownership remain unchanged.
5. The rendering path is deterministic on supported hosts without requiring locally installed fonts. Fonts have documented redistribution rights.
6. Existing tests and protocol validations pass; newly added negative-path, Unicode, clipping, and layout tests pass.
7. The original host typography migration does not require a PaperPad binary,
   PPFB v2 framing, MXCFB, or waveform-selection change. The later baked Exit
   follow-up changes only PaperPad's device-local label pixels.
8. Physical Kindle quality is recorded as **verified** only after an actual device run; otherwise it stays explicitly **unverified**.

## Follow-up — baked PaperPad Exit label (2026-11-17)

- Replaced the hand-authored scaled 5x7 `EXIT` glyphs with a fixed 110x34
  one-bit bitmap generated from the pinned IBM Plex Mono Medium face at 48 px
  and threshold 128. The font input is identified by SHA-256
  `98fbd727aae340b236955879dabed4d991aac9e8e90b3b2a67ce4a59221cc97c`.
- `tools/paperspoon/examples/bake_exit_label.rs` reproduces the Rust bitmap and
  a PBM preview under the project-local ignored `.cache/typography-spike/`
  directory. The committed asset is 476 bytes with MSB-first rows and zeroed
  padding bits.
- PaperPad centers the bitmap in the unchanged 72-pixel local strip and
  unchanged Exit hit rectangle. If an unusually narrow Exit rectangle cannot
  contain the fixed bitmap plus its border, PaperPad preserves the outline and
  touch target while omitting the label rather than distorting the font.
- The PaperPad binary contains neither OpenType bytes nor `cosmic-text`; font
  parsing and rasterization remain host-side build-time tooling. Connection
  indicator rendering, local activation, remote-viewport ownership, and GC16
  system-strip presentation are unchanged.

## Risks and follow-ups (not required for this migration)

- **Mono1 legibility:** monospacing alone does not guarantee crisp 1-bit glyphs. Compare sizes/weights and threshold on-device; consider an original-size Terminus bitmap profile only if measured results justify an additional path.
- **Helvetica selection:** system Helvetica may vary and cannot be redistributed casually. A future user-configurable system-font override should retain bundled Inter as the deterministic fallback.
- **Font coverage:** one bundled family may not cover all metadata scripts or emoji; missing-glyph behavior must be explicit, and broad fallback support is a separate feature.
- **Caching and process boundaries:** a persistent glyph cache benefits the main process but not the one-shot Now Playing helper. Keep benchmark results for both workloads distinct.
- **Dynamic updates:** switching to a visually nicer Gray8 font does not make the e-ink panel refresh faster; future waveform or dirty-region policy should be designed independently.

## Codex execution instructions

Implement stages in order. Treat each gate as a checkpoint: complete the stage, run the relevant checks, and report changes and remaining uncertainty before advancing. Favor localized changes and reuse over a generalized font framework. Do not remove the legacy renderer until all production callers have migrated. If a dependency, font format, license, or platform behavior differs from the assumptions above, verify it in the checkout and document the deviation rather than inventing behavior. Do not present host-rendered images as physical Kindle verification. Follow the repository's `AGENTS.md` for any device activity and deployment permissions.

# PaperSpoon typography migration: Mono1 monospace and Gray8 proportional OpenType

Status: Proposed  
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
- No immediate replacement of PaperPad's local Exit-strip text.

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

- [ ] Add a Mono1 text compositing path to `mono_renderer.rs`. Convert glyph coverage to exactly one bit using a documented threshold; do not apply grayscale antialiasing or spatial dithering to the initial implementation.
- [ ] Handle both black-on-white and white-on-black (pressed button) foregrounds without altering unrelated pixels. Clip safely, including negative bearings, descenders, partial glyphs, right/bottom borders, and non-byte-aligned canvas widths.
- [ ] Migrate `application_renderer.rs` to the monospaced text profile. Replace font-specific offsets in `application_ui.rs` with measured positioning (especially button-label centering), keeping the hit-test rectangle unchanged.
- [ ] Preserve the top title and status bar layout for small as well as 1272x1624 viewports. Ensure long status messages are clipped or ellipsized without overlapping other UI or wrapping unexpectedly.
- [ ] Update exact-pixel tests that encode the old `font5x7` glyph pattern. Preserve geometry, active-button inversion, protocol encoding, valid viewport, and status-region assertions.
- [ ] Add dedicated tests for white-on-black labels, Mono1 MSB-first packing, padding bits, clipping, and narrow viewports.

**Gate:** `ui WIDTHxHEIGHT` produces a valid, legible Mono1 frame; button positions and interaction semantics have not changed; host tests pass. Keep the old `font5x7` path until Gray8 has also migrated.

## Stage 3 — Gray8 and Now Playing migration

- [ ] Add Gray8 glyph compositing to `gray_renderer.rs` using alpha/coverage and destination luminance, including correct dark text on white and non-white backgrounds. Do not use one-bit thresholding for this profile.
- [ ] Migrate `now_playing_renderer.rs` to the Inter proportional text profile. Use actual font metrics and measured widths instead of `scale`-based 5x7 dimensions and fixed `14 * scale` line heights.
- [ ] Define hierarchy and wrapping/truncation: title is primary; artist, album, playback state, and duration are secondary. Keep artwork, progress bar, and labels within the viewport, including short/wide layouts and exceptionally long metadata.
- [ ] Preserve existing metadata sanitation and input-size bounds, adding grapheme-safe truncation as needed. Test Latin accents, Cyrillic, combining characters, punctuation, and missing/unsupported glyphs.
- [ ] Preserve the existing Now Playing helper lifecycle, termination, deadlines, failure semantics, and wire response. Measure the extra font initialization cost **inside the short-lived helper** rather than assuming cache reuse across helper launches.
- [ ] If initialization materially threatens the helper timeout, record evidence and address it in a separately scoped change; do not casually move MediaRemote acquisition or alter helper process boundaries in this typography migration.
- [ ] Add deterministic host-side Gray8 pixel/golden-image tests using the pinned fonts. Ensure output uses the existing Gray8 protocol and payload cap.

**Gate:** Now Playing renders readable proportional text without clipping or malformed luminance; helper timeout/error behavior is preserved and tested.

## Stage 4 — Remove legacy renderer and document the result

- [ ] Remove remaining callers of `font5x7`, then remove `font5x7.rs` and unused imports/tests. Do not leave parallel production text implementations without a concrete reason.
- [ ] Update `tools/paperspoon/Cargo.toml` and `Cargo.lock`; remove unused packages and avoid adding platform dependencies to the ARM PaperPad binary.
- [ ] Document the shipped font families, licenses, font roles, pixel sizes, Mono1 threshold, Gray8 blending, fallback behavior, and why pixel format does not select waveform.
- [ ] Add a concise note to `README.md` if operator-visible behavior or build prerequisites change. Keep detailed design and validation in `docs/` according to `AGENTS.md`.
- [ ] Confirm the default build does not require access to installed macOS fonts, user font directories, or external network services at runtime.

**Gate:** One production typography path remains; no `font5x7` production references remain; the intended files and licenses are included.

## Verification

### Automated host checks

- [ ] Run `cargo fmt --all -- --check`.
- [ ] Run `cargo test --workspace`.
- [ ] Run `cargo clippy --workspace --all-targets -- -D warnings` (if the repository's current lint baseline allows it; document unrelated pre-existing failures).
- [ ] Run `make check` and `git diff --check`, applying the repository's current `AGENTS.md` verification expectations.
- [ ] For Mono1, assert that pixels are only black/white, encoded in the existing bit order, and that non-byte-aligned trailing bits remain valid.
- [ ] For Gray8, assert that coverage blending produces intermediate luminance values where expected and respects clipping, alpha, and foreground/background inversion.
- [ ] Test empty metadata, long single words, narrow viewports, unknown glyphs, missing/corrupt font data, and repeated frames without uncontrolled cache growth.
- [ ] Compare screenshots and layout metrics against committed fixtures generated with the **pinned** fonts, not whatever Helvetica happens to be installed locally.
- [ ] Verify that only `paperspoon` gains new font dependencies. Do not rebuild or redeploy PaperPad merely for host-only rendering changes.

### Manual host / physical-device validation

- [ ] Render `ui 1272x1624` and `frame nowplaying` locally, and inspect representative short and long text.
- [ ] On a connected Paperwhite 6, check Mono1 glyph legibility, button-label centering, inverted pressed labels, top/bottom clipping, and that the PaperPad Exit strip stays intact.
- [ ] Check Gray8 antialiasing, title/metadata hierarchy, artwork/progress placement, Cyrillic/accents, and visible contrast on the actual e-ink panel.
- [ ] If testing multiple waveforms, record requested waveform, physical observations, and ghosting **separately** from font quality; do not make unverified claims about panel FPS or exact colors from host screenshots.
- [ ] Check reconnect and failed-Now-Playing behavior: the previous authoritative frame should remain valid when a replacement cannot be rendered.
- [ ] Record concise, dated evidence in the appropriate `docs/` file. If no Kindle is accessible, mark physical rendering **not verified** rather than passing it by inference.

## Acceptance criteria

1. Mono1 application text uses a bundled monospaced family, renders as true 1-bit pixels, and remains readable with both normal and inverted button states.
2. Gray8 Now Playing text uses a bundled Helvetica-like proportional family and visibly uses intermediate luminance where glyph edges are antialiased.
3. Both paths share the same text-layout/rasterization foundation unless Stage 0 records a concrete reason otherwise.
4. Text placement uses measured glyph metrics; UI hit targets, protocol layout, frame sizes, reconnect behavior, and PaperPad ownership remain unchanged.
5. The rendering path is deterministic on supported hosts without requiring locally installed fonts. Fonts have documented redistribution rights.
6. Existing tests and protocol validations pass; newly added negative-path, Unicode, clipping, and layout tests pass.
7. No change to PaperPad binary, PPFB v2 framing, MXCFB, or waveform selection is required.
8. Physical Kindle quality is recorded as **verified** only after an actual device run; otherwise it stays explicitly **unverified**.

## Risks and follow-ups (not required for this migration)

- **Mono1 legibility:** monospacing alone does not guarantee crisp 1-bit glyphs. Compare sizes/weights and threshold on-device; consider an original-size Terminus bitmap profile only if measured results justify an additional path.
- **Helvetica selection:** system Helvetica may vary and cannot be redistributed casually. A future user-configurable system-font override should retain bundled Inter as the deterministic fallback.
- **Font coverage:** one bundled family may not cover all metadata scripts or emoji; missing-glyph behavior must be explicit, and broad fallback support is a separate feature.
- **Caching and process boundaries:** a persistent glyph cache benefits the main process but not the one-shot Now Playing helper. Keep benchmark results for both workloads distinct.
- **Dynamic updates:** switching to a visually nicer Gray8 font does not make the e-ink panel refresh faster; future waveform or dirty-region policy should be designed independently.

## Codex execution instructions

Implement stages in order. Treat each gate as a checkpoint: complete the stage, run the relevant checks, and report changes and remaining uncertainty before advancing. Favor localized changes and reuse over a generalized font framework. Do not remove the legacy renderer until all production callers have migrated. If a dependency, font format, license, or platform behavior differs from the assumptions above, verify it in the checkout and document the deviation rather than inventing behavior. Do not present host-rendered images as physical Kindle verification. Follow the repository's `AGENTS.md` for any device activity and deployment permissions.

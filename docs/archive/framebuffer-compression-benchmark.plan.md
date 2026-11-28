# Framebuffer Compression Benchmark

## 1. Objective

Evaluate lossless framebuffer compression and inter-frame encoding strategies for PaperSpoon, PaperPad, and PaperView.

Determine the most efficient transport encoding based on real application workloads and measurements performed on the Kindle Paperwhite 6.

Optimize for:

- Reduced network bandwidth.
- Low decoding latency on ARMv7.
- Low CPU utilization.
- Predictable memory consumption.
- Minimal implementation complexity.
- Compatibility with the existing mixed-waveform display backend.
- Potential reduction in battery consumption.

Do not select a production compression algorithm before collecting benchmark results.

## 2. Repository context

Repository:

https://github.com/num13ru/rust_x11_hello

Before implementation:

- Read `AGENTS.md` and `README.md`.
- Inspect the current PPFB protocol implementation.
- Inspect framebuffer representations and pixel formats.
- Inspect PaperSpoon's rendering and frame publication lifecycle.
- Inspect PaperView's framebuffer reception and export capabilities.
- Inspect PaperPad's framebuffer reception, dirty-region detection, and waveform policy.
- Inspect existing benchmark utilities.
- Inspect the current Nix and ARMv7 musl build configuration.
- Review PR #19 and its device performance evidence.

Reuse existing infrastructure wherever possible.

Do not assume that the repository structure has remained unchanged since PR #19.

## 3. Scope and architectural constraints

This PR is a research and benchmarking milestone.

Do not modify:

- PPFB wire format.
- Production network transport.
- MXCFB update behavior.
- Mixed-waveform selection.
- Now Playing rendering behavior.
- Existing application interaction logic.
- Existing connection lifecycle.

Introduce isolated benchmark tooling.

A benchmark executable may reuse existing shared framebuffer types and utilities, but experimental compression dependencies must not become mandatory PaperPad runtime dependencies.

Preserve compatibility with:

- macOS ARM64.
- Linux ARMv7 musl.
- Existing Nix development environment.
- Existing container-based build workflow.
- PaperView.

Do not introduce lossy compression.

Decoded framebuffer data must remain byte-for-byte identical to the original input.

## 4. Current baseline

The current architecture transmits complete framebuffers over TCP using PPFB.

For the current PW6 working viewport:

| Property | Value |
|---|---:|
| Width | 1272 px |
| Height | 1624 px |
| Gray8 framebuffer | Approximately 1.97 MiB |
| Mono1 framebuffer | Approximately 252 KiB |
| Raw Gray8 bandwidth at 1 FPS | Approximately 16.5 Mbit/s |
| Raw Gray8 bandwidth at 10 FPS | Approximately 165.3 Mbit/s |

Verify these values against the actual negotiated viewport, framebuffer stride, and payload representation.

Use actual payload lengths in benchmark calculations instead of hardcoded dimensions.

## 5. Architectural principle

Keep three independent concerns separated:

1. Transport encoding.
2. Framebuffer reconstruction.
3. Physical display refresh.

The future architecture should conceptually follow:

```text
PaperSpoon
    │
    ├── Render framebuffer
    ├── Select transport encoding
    └── Encode payload
            │
            ▼
        PPFB / TCP
            │
            ▼
Shared framebuffer decoder
    │
    ├── Decode payload
    └── Reconstruct original framebuffer
            │
            ├── PaperView
            │
            └── PaperPad
                    ├── Dirty-region detection
                    ├── Waveform classification
                    └── MXCFB
```

The MXCFB backend must remain unaware of transport compression.

Transport-level dirty tiles and physical display update rectangles are different concepts.

Do not assume that they must have identical geometry.

## 6. Benchmark candidates

### 6.1 Full-frame compression

Evaluate the following independent-frame encodings:

| Encoding | Implementation candidate |
|---|---|
| Raw | Existing framebuffer representation |
| RLE | Minimal experimental implementation |
| LZ4 | `lz4_flex` |
| Zstandard | `zstd` |
| Deflate | `flate2` |
| PNG | Existing PNG support or an isolated benchmark dependency |

Evaluate PNG as a reference image compression format, not as the preferred PPFB transport format.

For Zstandard, initially evaluate compression levels:

- 1
- 3
- 6

For Deflate:

- Fast
- Default
- Best

For LZ4, start with its standard block compression implementation.

Prefer reusable output buffers where supported.

### 6.2 XOR delta compression

Evaluate inter-frame encoding using consecutive framebuffers.

```text
delta[i] = current[i] XOR previous[i]
```

Reconstruction:

```text
current[i] = delta[i] XOR previous[i]
```

Unchanged bytes become zero, including unchanged grayscale artwork.

Evaluate:

- Raw XOR delta.
- XOR + RLE.
- XOR + LZ4.
- XOR + Zstandard.
- XOR + Deflate.

Measure the complete cost of producing and reconstructing the delta.

Do not measure compression of an already generated XOR buffer while excluding XOR computation.

Use existing packed Mono1 representations without unnecessarily expanding them to Gray8.

### 6.3 Dirty tiles

Evaluate rectangular framebuffer patches containing absolute pixel values.

Initial candidate tile sizes:

- 16 × 16.
- 32 × 32.
- 64 × 64.
- 128 × 128.

For every tile:

1. Compare the current tile against the previous framebuffer.
2. Skip unchanged tiles.
3. Extract changed tiles.
4. Encode the selected tile representation.
5. Reconstruct the framebuffer by applying decoded tiles.

Evaluate:

- Dirty tiles without compression.
- Dirty tiles + LZ4.
- Dirty tiles + Zstandard.
- Dirty tiles + RLE.
- Dirty tiles using XOR + LZ4, if useful.

Compare independent per-tile compression against compression of an aggregated tile payload.

Include tile coordinates, dimensions, identifiers, and framing overhead in the reported payload size.

Handle partial tiles at viewport boundaries.

Preserve packed Mono1 representation and correctly handle byte alignment.

Do not implement hierarchical or adaptive tile partitioning during the initial benchmark unless fixed-size tiles demonstrate a clear limitation.

### 6.4 Dirty rectangles

Evaluate the existing dirty-region detection approach as an additional transport encoding candidate.

Compare:

- Single dirty bounding rectangle.
- Multiple dirty rectangles.
- Raw rectangle pixels.
- LZ4-compressed rectangle pixels.
- Zstandard-compressed rectangle pixels.

Reuse existing region detection logic if it can be separated from physical waveform selection without modifying production behavior.

Include the computational cost of rectangle detection.

Do not reuse waveform-specific rectangle splitting blindly for transport purposes.

## 7. Representative framebuffer corpus

Synthetic random data is insufficient for selecting a production encoding.

Create a reproducible corpus of real PaperSpoon framebuffer sequences.

### 7.1 Now Playing

Capture:

- Initial complete Now Playing framebuffer.
- Continuous playback for at least 120 seconds.
- Playback timer updates.
- Incremental seekbar movement.
- Forward and backward seeking.
- Play/pause transitions.
- Track changes.
- Album artwork changes.
- Shuffle and repeat state changes.
- Long track metadata.

Use the existing fake media provider to create reproducible scenarios.

Capture real media sessions separately when useful.

### 7.2 Application Grid

Capture:

- Initial application grid.
- Selection and interaction states.
- Navigation between application views.
- Complete screen transitions.
- Changes affecting multiple disconnected regions.

### 7.3 High-motion content

Use existing animation or Bad Apple infrastructure where available.

Capture:

- Mono1 sequences.
- Gray8 sequences, where supported.
- Multiple frame rates.
- Small and large inter-frame changes.

### 7.4 Synthetic edge cases

Generate additional test frames:

- Entirely black.
- Entirely white.
- Identical consecutive frames.
- Single changed pixel.
- Single changed scanline.
- Small isolated rectangles.
- Multiple distant changed rectangles.
- Checkerboard patterns.
- Grayscale gradients.
- High-entropy image data.
- Complete framebuffer replacement.

Use synthetic inputs for correctness testing and worst-case analysis, not as the primary basis for codec selection.

## 8. Frame capture format

Introduce a simple versioned benchmark corpus format.

Store:

- Corpus format version.
- Scenario name.
- Pixel format.
- Framebuffer width and height.
- Row stride.
- Frame index.
- Relative timestamp.
- Raw framebuffer length.
- Raw framebuffer bytes.

Optionally store a checksum for integrity verification.

Preserve original framebuffer bytes.

Do not use PNG as the primary corpus representation because decoding PNG before every benchmark iteration introduces additional work and potentially changes the measured workload.

Support exporting and importing benchmark sequences.

Prefer existing PaperView export infrastructure if it already provides access to the required raw data.

If capture integration is necessary, keep it opt-in and isolated from normal application execution.

Do not commit large framebuffer recordings or copyrighted album artwork to Git.

Provide reproducible corpus-generation instructions and small synthetic fixtures instead.

## 9. Benchmark executable

Create a dedicated benchmark executable or workspace package following existing repository conventions.

Suggested name:

```text
framebuffer-bench
```

Suggested commands:

```sh
framebuffer-bench capture
framebuffer-bench generate
framebuffer-bench encode
framebuffer-bench decode
framebuffer-bench verify
framebuffer-bench report
```

These command names are proposals. Adapt them to the existing CLI architecture.

Support:

- Input corpus selection.
- Encoding selection.
- Compression level selection.
- Tile size selection.
- Warm-up iterations.
- Measurement iterations.
- Decoder-only execution.
- Encoder-only execution.
- End-to-end reconstruction.
- JSON or CSV result export.
- Human-readable terminal reports.
- Deterministic verification.

Avoid building a separate application framework for this utility.

## 10. Benchmark execution model

### 10.1 Host encoding benchmark

Run on the macOS ARM64 workstation.

Measure:

- Frame comparison duration.
- Delta generation duration.
- Dirty-region detection duration.
- Tile extraction duration.
- Compression duration.
- Payload construction duration.
- Total encoding duration.
- Peak encoded payload size.
- Memory consumption.

Generate encoded benchmark payloads for device-side testing.

### 10.2 Device decoding benchmark

Run directly on the Kindle Paperwhite 6.

The device benchmark must not require:

- An active PaperSpoon connection.
- PaperView.
- MXCFB access.
- Physical framebuffer updates.
- X11.
- Music.app.
- Network traffic during measurement.

Load previously generated encoded benchmark sequences.

Measure:

- Payload parsing duration.
- Decompression duration.
- XOR reconstruction duration.
- Tile or rectangle application duration.
- Total framebuffer reconstruction duration.
- CPU time.
- Peak memory consumption.
- Decoder initialization cost.

Keep file I/O outside timed decoding sections.

Report file-loading costs separately when relevant.

### 10.3 End-to-end verification

Every decoded framebuffer must exactly match its source framebuffer.

Verify:

- Complete framebuffer length.
- Pixel values.
- Pixel format.
- Row stride and padding behavior.
- Frame ordering.
- Reconstruction after complete screen changes.

Perform correctness verification outside timed sections unless explicitly measuring validation overhead.

Prevent the compiler from optimizing away benchmark operations.

## 11. ARMv7 compatibility

The current PaperPad target uses ARMv7 musl.

Verify the exact target triple and compiler configuration before introducing dependencies.

For every candidate codec:

- Verify successful cross-compilation.
- Verify successful execution on PW6.
- Record native library dependencies.
- Record static linking compatibility.
- Record binary size impact.
- Record decoder memory requirements.
- Verify compatibility with the pinned Rust toolchain.
- Inspect enabled Cargo features.

Prefer pure Rust implementations for the initial benchmark.

Suggested candidates:

| Crate | Purpose |
|---|---|
| `lz4_flex` | LZ4 encoding and decoding |
| `zstd` | Zstandard through the native implementation |
| `ruzstd` | Alternative pure Rust Zstandard decoding |
| `flate2` | Deflate |
| `miniz_oxide` | Pure Rust Deflate backend |

Verify current crate APIs, supported targets, and MSRV before selecting versions.

Keep benchmark dependencies isolated from production executables.

Do not introduce unrelated Rust, LLVM, Nix, or dependency upgrades.

## 12. Hardware acceleration investigation

Do not assume that the Kindle contains an accessible hardware compression accelerator.

Inspect the actual device environment.

Collect:

```sh
cat /proc/cpuinfo
cat /proc/meminfo
cat /proc/crypto
uname -a
```

Record:

- CPU architecture.
- Available CPU features.
- NEON availability.
- Available memory.
- Kernel version.
- Available kernel compression implementations.
- Relevant hardware capabilities, if documented.

A registered kernel compression algorithm does not necessarily indicate dedicated hardware acceleration.

### 12.1 SIMD

If ARM NEON is available, investigate whether relevant codec implementations already use it.

Compare, where supported:

- Default portable build.
- ARMv7 build with verified target-specific optimizations.
- Existing optimized native codec implementation.

Do not enable unsupported CPU features.

Do not implement custom NEON intrinsics before identifying a measurable decoding bottleneck.

### 12.2 Dedicated acceleration

Investigate dedicated compression hardware only if:

- Its existence is confirmed.
- A supported userspace or kernel interface is available.
- It supports a relevant lossless encoding.
- Transfer and initialization overhead are acceptable.

Do not use GPU compute or the e-ink controller as speculative compression accelerators.

Software decoding is the initial baseline.

## 13. Metrics

For every encoding and scenario, collect:

### Compression

- Original framebuffer bytes.
- Encoded payload bytes.
- Metadata and framing overhead.
- Compression ratio.
- Percentage of original bandwidth.
- Minimum and maximum payload size.
- Average payload size.
- Median payload size.
- P95 and P99 payload size.

### Encoding performance

- Median encoding latency.
- P95 encoding latency.
- P99 encoding latency.
- Maximum encoding latency.
- Encoding throughput.
- Frame comparison cost.
- Delta generation cost.
- Memory allocation count, where practical.

### Decoding performance

- Median decoding latency.
- P95 decoding latency.
- P99 decoding latency.
- Maximum decoding latency.
- Decoding throughput.
- Framebuffer reconstruction latency.
- CPU utilization.
- Peak resident memory.
- Temporary buffer requirements.
- Decoder initialization cost.

### Operational complexity

- Dependency count.
- Native dependency requirements.
- Binary size impact.
- Cross-compilation complexity.
- Implementation complexity.
- State synchronization requirements.
- Error recovery requirements.

Define throughput calculations explicitly.

For delta and tile encodings, distinguish encoded input throughput from reconstructed framebuffer throughput.

## 14. Bandwidth projections

Calculate projected network bandwidth for:

- 0.5 FPS.
- 1 FPS.
- 2 FPS.
- 4 FPS.
- 8 FPS.
- 10 FPS.

Report:

- Bytes per frame.
- KiB/s.
- MiB/s.
- Mbit/s.
- MiB per hour.
- GiB per hour.

Calculate scenario-specific projections.

Do not extrapolate Now Playing compression ratios to animation workloads.

Include protocol overhead assumptions in the report.

Distinguish measured payload size from projected network usage.

## 15. Keyframe and delta dependency analysis

Inter-frame compression introduces state dependencies.

Benchmark sequences must represent:

- Initial complete keyframe.
- Consecutive delta frames.
- Complete screen changes.
- Periodic keyframes.
- Explicit reconstruction reset.
- Missing or incorrect base frames.

Evaluate keyframe intervals experimentally:

- Every 10 frames.
- Every 30 frames.
- Every 60 frames.
- Keyframes only on initialization and recovery.

Periodic keyframes are benchmark candidates, not production requirements.

Account for their bandwidth overhead.

For future production integration, distinguish:

```text
Latest rendered framebuffer
Latest encoded framebuffer
Latest transmitted framebuffer
Latest decoded framebuffer
Latest physically displayed framebuffer
```

A delta must reference a framebuffer that the receiver can reconstruct.

Dropping intermediate physical display updates is acceptable.

Dropping intermediate dependent transport frames without rebuilding the dependency chain is not acceptable.

Do not implement production synchronization changes in this PR.

## 16. Adaptive encoding experiment

Evaluate an offline encoding selector.

For each framebuffer transition, compare:

- Raw full framebuffer.
- LZ4-compressed full framebuffer.
- Zstandard-compressed full framebuffer.
- XOR + LZ4.
- Dirty tiles + LZ4.
- Dirty rectangles + LZ4.

Record which candidate produces the smallest complete payload.

Calculate the theoretical bandwidth improvement of adaptive selection.

This exhaustive comparison represents an upper-bound research result, not a proposed production algorithm.

Additionally evaluate inexpensive selection heuristics based on:

- Changed byte count.
- Changed tile count.
- Estimated changed area.
- Pixel format.
- Previous compression results.
- Full-screen transition detection.

Measure the computational overhead of selecting an encoding.

Do not assume that the smallest payload always provides the best end-to-end latency.

## 17. Memory management

Evaluate reusable buffers.

Avoid allocating multiple complete framebuffer buffers on every frame unnecessarily.

Measure:

- Previous framebuffer storage.
- Current framebuffer storage.
- Delta buffer storage.
- Compressed payload storage.
- Decoder scratch buffers.
- Codec-specific working memory.

Compare:

- Fresh allocations per frame.
- Reused buffers.
- In-place reconstruction where supported.
- Direct reconstruction into a designated output buffer.

Ensure that state mutation cannot corrupt the previous framebuffer when decoding fails.

Do not sacrifice reconstruction correctness for lower memory consumption.

## 18. Decoder robustness

Test malformed and incomplete inputs.

Include:

- Truncated compressed payloads.
- Invalid compressed data.
- Unexpected decoded lengths.
- Oversized declared framebuffer dimensions.
- Integer overflow in size calculations.
- Invalid rectangle coordinates.
- Invalid tile coordinates.
- Duplicate tiles.
- Unexpected pixel format changes.
- Missing keyframes.
- Incorrect delta base.
- Corrupted payloads.
- Empty payloads.

Enforce decoded size limits.

For benchmark formats containing length prefixes, validate lengths before allocation.

Reject invalid input without panicking or causing uncontrolled memory consumption.

Verify that a subsequent valid keyframe can restore a usable reconstruction state.

## 19. PaperView validation

Use PaperView for visual verification of decoded framebuffer sequences.

Where practical, replay reconstructed benchmark sequences through existing framebuffer visualization or export mechanisms.

Verify:

- Album artwork remains pixel-identical.
- Binary text remains unchanged.
- Seekbar rendering is preserved.
- Timer digits remain unchanged.
- Mono1 content remains correctly packed and reconstructed.
- Gray8 content retains its original grayscale values.

PaperView must not require a separate compression implementation.

The compression benchmark must not alter physical waveform behavior.

Successful byte-for-byte reconstruction is the primary correctness criterion.

Visual validation is supplementary.

## 20. Device performance targets

Use the following provisional decoding latency targets for PW6.

| Workload | Target P95 reconstruction latency |
|---|---:|
| Now Playing at 1 Hz | Under 50 ms |
| Interactive updates at 4 Hz | Under 25 ms |
| Experimental animation at 10 Hz | Under 10 ms |

These targets are initial engineering goals, not measured device capabilities or hard release requirements.

Prefer low CPU utilization and predictable latency over maximizing throughput.

Evaluate whether a slower codec with substantially smaller payloads provides better overall behavior.

Record failures to meet these targets instead of excluding unfavorable results.

## 21. Power consumption considerations

Compression may reduce Wi-Fi activity while increasing CPU utilization.

Do not assume a net battery improvement without measurement.

If reliable device power telemetry is available, perform a separate controlled experiment comparing:

- Raw framebuffer transport.
- Full-frame LZ4.
- Full-frame Zstandard.
- XOR + LZ4.
- Dirty tiles + LZ4.

Keep the following conditions consistent:

- Frame sequence.
- Playback duration.
- Physical refresh policy.
- Network conditions.
- Screen illumination.
- Device power configuration.

A decoder-only benchmark cannot establish total transport-related battery consumption.

If reliable power measurements are unavailable, document this limitation and defer battery conclusions.

## 22. Automated tests

### Full-frame encoding

- [ ] Raw round-trip succeeds.
- [ ] LZ4 round-trip succeeds.
- [ ] Zstandard round-trip succeeds.
- [ ] Deflate round-trip succeeds.
- [ ] RLE round-trip succeeds.
- [ ] PNG reference encoding preserves pixels.

### Delta encoding

- [ ] Identical frames reconstruct correctly.
- [ ] Single-pixel changes reconstruct correctly.
- [ ] Complete frame changes reconstruct correctly.
- [ ] Long delta chains reconstruct correctly.
- [ ] Keyframe resets reconstruct correctly.
- [ ] Incorrect delta bases are rejected by the benchmark reconstruction model.

### Dirty tiles and rectangles

- [ ] Single changed tile reconstructs correctly.
- [ ] Multiple disconnected tiles reconstruct correctly.
- [ ] Partial boundary tiles reconstruct correctly.
- [ ] Non-aligned viewport dimensions are handled.
- [ ] Packed Mono1 data is handled correctly.
- [ ] Invalid coordinates are rejected.
- [ ] Unchanged frames produce no unnecessary patches.

### Robustness

- [ ] Truncated payloads are rejected.
- [ ] Invalid lengths are rejected.
- [ ] Oversized allocations are prevented.
- [ ] Invalid inputs do not panic.
- [ ] Decoder state can recover after a valid reset.

### Compatibility

- [ ] macOS ARM64 benchmark builds and runs.
- [ ] ARMv7 musl benchmark builds.
- [ ] ARMv7 benchmark runs on PW6.
- [ ] Existing PaperSpoon tests remain green.
- [ ] Existing PaperPad tests remain green.
- [ ] Existing PaperView tests remain green.
- [ ] Production protocol remains unchanged.

## 23. Benchmark report

Create a concise report following existing repository documentation conventions.

Suggested path:

```text
docs/benchmarks/framebuffer-compression.md
```

Include:

1. Test environment.
2. CPU architecture and available features.
3. Rust toolchain and build configuration.
4. Codec implementations and versions.
5. Corpus description.
6. Benchmark methodology.
7. Full-frame compression results.
8. Delta compression results.
9. Dirty tile results.
10. Dirty rectangle results.
11. ARMv7 decoding performance.
12. Memory consumption.
13. Bandwidth projections.
14. Native dependency and binary size impact.
15. Known limitations.
16. Recommended production encoding strategy.

Suggested comparison table:

| Encoding | Scenario | Payload size | Ratio | Host encode P95 | PW6 decode P95 | Peak memory |
|---|---|---:|---:|---:|---:|---:|
| Raw | Now Playing | TBD | TBD | TBD | TBD | TBD |
| LZ4 | Now Playing | TBD | TBD | TBD | TBD | TBD |
| Zstd | Now Playing | TBD | TBD | TBD | TBD | TBD |
| XOR + RLE | Now Playing | TBD | TBD | TBD | TBD | TBD |
| XOR + LZ4 | Now Playing | TBD | TBD | TBD | TBD | TBD |
| XOR + Zstd | Now Playing | TBD | TBD | TBD | TBD | TBD |
| Dirty tiles + LZ4 | Now Playing | TBD | TBD | TBD | TBD | TBD |
| Dirty rectangles + LZ4 | Now Playing | TBD | TBD | TBD | TBD | TBD |

Replace placeholders only with actual measurements.

Do not fabricate missing device results.

## 24. Decision criteria

Select the future production encoding according to measured trade-offs.

Prioritize:

1. Byte-for-byte reconstruction correctness.
2. Reliable ARMv7 compatibility.
3. Predictable decoding latency.
4. Reduced network bandwidth.
5. Low CPU and memory overhead.
6. Simple dependency management.
7. Straightforward recovery and synchronization.
8. Maintainability.

Initial hypotheses:

- Full-frame LZ4 may provide the simplest initial transport improvement.
- XOR + LZ4 may provide substantial bandwidth savings for persistent widgets.
- Dirty tiles may reduce both bandwidth and decoder memory traffic.
- Dirty rectangles may outperform fixed-size tiles for localized UI changes.
- Zstandard may provide better compression for grayscale artwork.
- RLE may be competitive for sparse binary changes.
- PNG may provide useful reference results but unnecessary transport complexity.

Treat all hypotheses as unverified until the benchmark is complete.

## 25. Implementation milestones

### M0 — Repository audit

- [ ] Inspect existing protocol and framebuffer implementation.
- [ ] Inspect current benchmark infrastructure.
- [ ] Inspect PW6 hardware and build configuration.
- [ ] Identify reusable capture infrastructure.

### M1 — Corpus generation

- [ ] Implement reproducible framebuffer corpus generation.
- [ ] Capture Now Playing sequences.
- [ ] Capture Application Grid sequences.
- [ ] Capture animation sequences.
- [ ] Generate synthetic correctness fixtures.

### M2 — Independent-frame compression

- [ ] Implement raw baseline.
- [ ] Integrate LZ4.
- [ ] Integrate Zstandard.
- [ ] Integrate Deflate.
- [ ] Implement minimal RLE.
- [ ] Add PNG reference measurements.

### M3 — Inter-frame compression

- [ ] Implement XOR delta benchmark.
- [ ] Implement compressed XOR variants.
- [ ] Implement dirty tile benchmark.
- [ ] Evaluate multiple tile sizes.
- [ ] Implement dirty rectangle benchmark.
- [ ] Add keyframe experiments.

### M4 — Host benchmarking

- [ ] Measure macOS ARM64 encoding performance.
- [ ] Measure compression ratios.
- [ ] Measure encoding overhead.
- [ ] Export encoded sequences for device testing.

### M5 — Kindle benchmarking

- [ ] Cross-compile benchmark executable.
- [ ] Verify ARMv7 compatibility.
- [ ] Run decoder-only benchmarks on PW6.
- [ ] Measure decoding latency and memory.
- [ ] Investigate SIMD only if justified.
- [ ] Record actual device results.

### M6 — Analysis

- [ ] Compare all candidate encodings.
- [ ] Calculate bandwidth projections.
- [ ] Evaluate adaptive encoding.
- [ ] Identify practical trade-offs.
- [ ] Produce benchmark report.
- [ ] Recommend the next production implementation milestone.

## 26. Future production roadmap — out of scope

Based on benchmark results, a subsequent PR may introduce:

### Phase A — Independent compressed frames

- Capability negotiation.
- Raw fallback.
- Selected full-frame compression.
- Bounded decompression.
- Shared PaperPad and PaperView decoder.

### Phase B — Inter-frame encoding

- Frame identifiers.
- Base frame identifiers.
- Keyframe handling.
- Decoder synchronization.
- Reconnection recovery.
- Backpressure-aware delta generation.

### Phase C — Adaptive transport

- Dirty tiles or rectangles.
- Adaptive full-frame versus delta selection.
- Reusable reconstruction buffers.
- Reduced redundant framebuffer comparisons.
- Transport performance instrumentation.

Do not implement these phases in the current benchmark PR.

## 27. Additional research opportunities

- 💡 **Mixed encodings:** evaluate whether grayscale-heavy frames benefit from Zstandard while binary UI updates benefit from LZ4 or RLE.
- 💡 **Zero-copy reconstruction:** investigate applying absolute rectangular patches directly to a persistent decoded framebuffer.
- 💡 **Sparse XOR encoding:** compare generic RLE against a specialized representation containing zero-run lengths followed by literal changed bytes.
- 💡 **Tile bitmaps:** compare explicit tile coordinate headers against a compact bitmap identifying changed tiles.
- 💡 **Static dictionaries:** investigate whether recurring UI elements improve Zstandard compression when using a fixed dictionary, accounting for decoder memory and synchronization complexity.
- 💡 **Network latency crossover:** calculate when compression and decompression overhead becomes smaller than the time saved by transmitting fewer bytes.
- 💡 **Framebuffer comparison reuse:** investigate whether future transport patches can provide candidate dirty regions to the physical refresh planner without coupling transport encoding to waveform selection.

These experiments are optional and must not delay the core benchmark.

## 28. Definition of done

The milestone is complete when:

- Representative framebuffer sequences are available.
- Every candidate encoding is evaluated against identical source data.
- Every decoded framebuffer is verified byte-for-byte.
- Host encoding performance is measured.
- ARMv7 decoding performance is measured on PW6.
- Memory consumption and dependency requirements are documented.
- Bandwidth improvements are quantified.
- Performance limitations and failure cases are documented.
- A production encoding strategy is recommended based on evidence.
- Existing PPFB, PaperSpoon, PaperPad, PaperView, and MXCFB behavior remains unchanged.

If physical device access is unavailable, complete the host benchmark and prepare reproducible ARMv7 test artifacts. Mark device validation as pending.

Do not claim Kindle compatibility or performance based exclusively on successful cross-compilation or macOS benchmarks.

The purpose of this milestone is to establish evidence for the transport architecture, not to prematurely implement compression in production.

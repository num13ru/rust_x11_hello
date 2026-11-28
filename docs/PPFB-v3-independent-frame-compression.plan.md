# PPFB v3 — Independent Frame Compression

## 1. Objective

Introduce lossless framebuffer compression into the production PPFB transport based on the completed framebuffer compression benchmark.

Reduce network bandwidth while preserving:

- Independent framebuffer reconstruction.
- Low decoding latency on Kindle Paperwhite 6.
- Existing mixed-waveform rendering.
- Existing framebuffer semantics.
- PaperView compatibility.
- Event-driven rendering.
- Frame coalescing and backpressure handling.
- Predictable memory consumption.
- Static ARMv7 musl builds.

Use full-frame Zstandard compression as the initial production encoding.

Retain raw framebuffer transport as a fallback.

Do not introduce inter-frame dependencies in this milestone.

## 2. Repository context

Repository:

https://github.com/num13ru/rust_x11_hello

Relevant implementation history:

- PR #19 — Real-time Now Playing and mixed-waveform rendering.
- Framebuffer Compression Benchmark.
- Existing PPFB v2 transport.
- Shared `paper-protocol` crate.
- PaperSpoon host renderer.
- PaperPad Kindle client.
- PaperView desktop client.

Before implementation:

- Read `AGENTS.md` and `README.md`.
- Locate and read the framebuffer compression benchmark report.
- Inspect the benchmark codec implementations.
- Inspect PPFB v2 handshake, framing, and validation.
- Inspect the current network connection lifecycle.
- Inspect frame publication and coalescing behavior.
- Inspect PaperPad's framebuffer ownership and display update lifecycle.
- Inspect PaperView's receiver and export functionality.
- Inspect the pinned Nix and ARMv7 build configuration.

Treat the current repository state as the source of truth.

Do not introduce unrelated refactoring.

## 3. Benchmark evidence

The compression benchmark evaluated 432 encoded sequences across eight scenarios on macOS ARM64.

Physical Kindle validation included:

- 36 decoder configurations.
- 4,779 timed framebuffer reconstructions.
- Successful integrity verification for every decoded frame.
- Successful static ARMv7 musl compilation and execution.

### 3.1 Now Playing results on PW6

| Encoding | Mean bytes/frame | Decode P95 | Peak decoder RSS |
|---|---:|---:|---:|
| Raw | 2,065,756 | 2.451 ms | 4.42 MiB |
| Full LZ4 | 43,764 | 7.250 ms | 2.57 MiB |
| Full Zstandard level 1 | 14,961 | 4.682 ms | 2.94 MiB |
| PNG | 14,084 | 13.888 ms | — |
| XOR + LZ4 | 9,526 | 10.291 ms | 4.59 MiB |
| 16-pixel tiles + LZ4 | 1,255 | 2.883 ms | 5.43 MiB |
| 16-pixel tiles + Zstandard | 578 | 2.988 ms | 5.80 MiB |
| Multiple rectangles + LZ4 | 1,396 | 3.003 ms | 5.38 MiB |

### 3.2 Full-frame Zstandard level 1

| Workload | Mean bytes/frame | PW6 decode P95 |
|---|---:|---:|
| Now Playing | 14,961 | 4.682 ms |
| Application Grid | 811 | 0.336 ms |
| Mono1 motion at 10 Hz | 3,004 | 0.817 ms |
| Gray8 motion at 10 Hz | 4,810 | 3.548 ms |

Full-frame Zstandard level 1 outperformed the tested full-frame LZ4 implementation in both compressed size and P95 decoding latency across all four workloads.

It also met all provisional device decoding latency targets.

### 3.3 Projected bandwidth

For the recorded Now Playing workload at 1 Hz:

| Encoding | Projected traffic per hour |
|---|---:|
| Raw | 6.93 GiB |
| Full Zstandard level 1 | 51.36 MiB |
| 16-pixel tiles + Zstandard | 1.98 MiB |

These projections exclude TCP and Wi-Fi overhead.

### 3.4 Limitations

The existing results represent:

- One physical device benchmark run.
- Procedural motion sequences.
- Simplified fake-provider album artwork.
- Decoder-only execution without live network transport.
- Reconstruction timing excluding file I/O and checksum verification.

Do not interpret these measurements as complete production latency or battery consumption results.

Do not generalize the observed Zstandard advantage to every implementation of LZ4 or every workload.

## 4. Scope

### In scope

- Additional benchmark validation with realistic content.
- PPFB protocol versioning.
- Compression capability negotiation.
- Independent Zstandard-compressed frames.
- Raw fallback.
- Shared framebuffer encoding and decoding.
- Bounded decompression.
- Reusable codec contexts and buffers.
- PaperSpoon integration.
- PaperPad integration.
- PaperView integration.
- Transport diagnostics.
- Network and physical device validation.
- Documentation.

### Out of scope

- XOR delta encoding.
- Dirty tiles.
- Transport-level dirty rectangles.
- Inter-frame dictionaries.
- Base frame identifiers.
- Periodic keyframes.
- Adaptive full-frame versus delta encoding.
- Lossy compression.
- Custom SIMD implementations.
- Hardware compression acceleration.
- MXCFB waveform policy changes.
- Rendering architecture changes.
- Music provider changes.

Keep experimental codecs available in the benchmark package without making them production transport dependencies.

## 5. Production encoding strategy

Support two independent framebuffer encodings initially:

| Encoding | Purpose |
|---|---|
| Raw | Compatibility and fallback |
| Zstandard | Default compressed transport |

Use Zstandard compression level 1 by default.

Compression level is a host-side encoder configuration.

The decoder must support valid Zstandard frames produced by supported encoder configurations, subject to the negotiated protocol limits.

Do not require the receiver to know the encoder's compression level.

Every transmitted framebuffer must remain independently reconstructable.

Do not use:

- Persistent inter-frame compression history.
- External dictionaries.
- Shared compression state between connections.
- Dependencies on previously decoded frames.

Codec context reuse is permitted, but compression history must not persist across independent frames.

## 6. Additional benchmark validation

Before enabling compression as the production default, extend the existing corpus.

### 6.1 Realistic album artwork

Capture or generate representative Now Playing sequences containing:

- Photographic album covers.
- High-frequency grayscale detail.
- Dark artwork.
- Bright artwork.
- Smooth gradients.
- High-contrast artwork.
- Artwork with text.
- Rapid artwork replacement.
- Missing artwork.

Use locally available media or appropriately licensed test images.

Do not commit copyrighted artwork or large private recordings to Git.

### 6.2 Realistic motion

Extend the existing motion corpus with:

- Bad Apple sequences, where existing infrastructure and local assets permit.
- Rapid full-screen transitions.
- Moving text.
- Scrolling content.
- Dense grayscale changes.
- Low-entropy animations.
- High-entropy grayscale content.

Retain both Mono1 and Gray8 workloads.

### 6.3 Zstandard compression levels

Evaluate:

- Level 1.
- Level 3.
- Level 6.

Measure host encoding cost and PW6 decoding cost separately.

The default remains level 1 unless additional evidence demonstrates a meaningful advantage for another level.

### 6.4 Repeated device measurements

Repeat representative physical decoder benchmarks.

Record:

- P50.
- P95.
- P99.
- Maximum latency.
- Peak RSS.
- Process CPU time.
- Device temperature, where reliably available.
- Available memory.
- Background system activity.

Document run-to-run variance.

Do not silently discard unfavorable measurements.

If physical device access is unavailable, prepare reproducible ARMv7 artifacts and mark validation as pending.

## 7. Architecture

Preserve the existing separation between transport, rendering, and physical display updates.

```text
PaperSpoon
    |
    +-- Render complete Mono1 / Gray8 framebuffer
    |
    +-- Select Raw or Zstandard
    |
    +-- Encode independent framebuffer payload
    |
    v
PPFB / TCP
    |
    v
Shared transport decoder
    |
    +-- Validate frame metadata
    +-- Validate payload bounds
    +-- Decode into complete framebuffer
    +-- Validate reconstructed framebuffer
    |
    +------------------------+
    |                        |
    v                        v
PaperView                 PaperPad
    |                        |
Native display          Dirty-region detection
                             |
                        Waveform policy
                             |
                           MXCFB
```

The display backend must not know whether the framebuffer was compressed.

The transport decoder must not know which pixels represent artwork, text, timers, or controls.

PaperSpoon must remain the sole application renderer.

PaperPad and PaperView must receive equivalent decoded framebuffer contents.

## 8. PPFB protocol evolution

### 8.1 Versioning

Introduce an explicitly versioned compression-capable protocol.

PPFB v3 is the proposed version for this change.

Audit the existing versioning and handshake mechanisms before finalizing the wire format.

Do not silently reinterpret existing PPFB v2 fields.

Do not send compressed bytes using a message type that existing clients interpret as raw framebuffer data.

Document the exact wire format before integrating production transport.

### 8.2 Compatibility requirements

Support the following compatibility matrix where the existing handshake permits safe negotiation:

| Host | Client | Expected behavior |
|---|---|---|
| PPFB v2 | PPFB v2 | Existing raw transport |
| PPFB v3 | PPFB v2 | Raw PPFB v2 |
| PPFB v2 | PPFB v3 | Raw PPFB v2 |
| PPFB v3 | PPFB v3 | Negotiated Raw or Zstandard |

If the existing handshake cannot support transparent fallback in a particular direction, implement an explicit and documented legacy connection mechanism.

Do not rely on ambiguous protocol detection.

Do not retry arbitrary incompatible data as another protocol version.

Unsupported combinations must fail clearly without hanging or corrupting the connection.

### 8.3 Capability negotiation

Negotiate supported framebuffer encodings during connection establishment.

Conceptually:

```text
Supported protocol versions:
    PPFB v2
    PPFB v3

Supported framebuffer encodings:
    Raw
    Zstandard
```

The actual representation must follow existing PPFB conventions.

Requirements:

- Raw is mandatory for compression-capable peers.
- Zstandard is optional.
- The sender must not use an unsupported encoding.
- The negotiated encoding capabilities remain valid for the connection lifetime.
- Reconnection performs negotiation again.
- Unknown capability identifiers must be handled safely.
- Negotiation failure must produce an actionable diagnostic.

Do not introduce a general-purpose capability framework beyond demonstrated requirements.

## 9. Frame representation

Define an independent encoded framebuffer message.

Suggested conceptual fields:

| Field | Purpose |
|---|---|
| Encoding identifier | Raw or Zstandard |
| Encoded payload length | Network payload size |
| Decoded payload length | Expected framebuffer size |
| Framebuffer dimensions | Existing viewport representation |
| Pixel format | Mono1 or Gray8 |
| Frame identifier | Diagnostics, if useful |
| Payload | Raw or compressed bytes |

Reuse existing negotiated layout information and framing fields wherever possible.

Do not duplicate metadata that is already available and validated for the connection.

A frame identifier is optional for independent frames.

Do not introduce base frame identifiers or delta dependencies.

Specify:

- Field widths.
- Byte order.
- Encoding identifiers.
- Reserved fields.
- Maximum payload sizes.
- Expected decoded lengths.
- Error behavior.
- Compatibility rules.

Create golden binary fixtures for the finalized format.

## 10. Shared codec implementation

Reuse the implementation proven by the framebuffer compression benchmark.

The benchmark used:

- `zstd` 0.14.5.
- `libzstd-rs-sys` Rust backend.

Inspect the actual dependency declarations and enabled Cargo features.

Preserve the tested backend unless a change is justified by measurements or compatibility requirements.

Avoid introducing a mandatory native C toolchain dependency.

Place production encoding and decoding in an appropriate shared transport module or crate.

Do not duplicate decompression logic across PaperPad and PaperView.

Where practical, allow the benchmark to exercise the same codec implementation used by production.

Keep benchmark-only algorithms isolated.

### 10.1 Dependency boundaries

PaperSpoon requires:

- Raw encoding.
- Zstandard compression.
- Negotiation and framing.

PaperPad requires:

- Raw decoding.
- Zstandard decompression.
- Negotiation and framing.

PaperView requires:

- The same receiver-side decoding implementation as PaperPad.

Investigate Cargo feature separation if it meaningfully reduces device binary size or dependencies.

Avoid unnecessary feature complexity if the compiler already removes unused implementation code.

## 11. Encoding selection

For every complete framebuffer:

1. Determine whether compression was negotiated and enabled.
2. Attempt Zstandard compression using the configured level.
3. Compare complete raw and compressed message sizes.
4. Select Zstandard when compression provides a meaningful size reduction.
5. Otherwise select Raw.
6. Publish the selected independent framebuffer message.

Conceptually:

```text
if compression is disabled or unsupported:
    send Raw
else:
    compress framebuffer

    if compressed message is smaller:
        send Zstandard
    else:
        send Raw
```

Include encoding-specific framing overhead in the comparison.

Initially use a simple deterministic selection policy.

Do not introduce content classification or workload-specific codec selection.

### 11.1 Compression failure

If host-side compression fails before transmission:

- Record the failure.
- Send a valid raw framebuffer when possible.
- Preserve connection consistency.
- Avoid repeatedly executing a failing compression path without diagnostics.

A partially written encoded network message must not be followed by an arbitrary raw message.

Terminate the connection when message framing can no longer be guaranteed.

### 11.2 High-entropy content

Verify that incompressible framebuffer content does not result in unnecessary network expansion.

Test with seeded noise and dense grayscale content.

Raw fallback must remain available even when Zstandard is negotiated.

## 12. Decoder safety

Treat every network payload as untrusted input.

Before allocation or decompression:

- Validate the negotiated protocol version.
- Validate the encoding identifier.
- Validate pixel format.
- Validate framebuffer dimensions.
- Validate row stride.
- Compute the expected framebuffer length using checked arithmetic.
- Validate the encoded payload length.
- Validate the declared decoded length.
- Enforce configured resource limits.

The maximum decoded framebuffer length must be derived from trusted layout information and supported viewport limits.

Do not allocate memory directly from an unrestricted compressed-frame size declaration.

### 12.1 Bounded decompression

Decompress into a bounded destination buffer.

Reject:

- Invalid Zstandard data.
- Truncated compressed frames.
- Unexpected decoded lengths.
- Unsupported encoding identifiers.
- Oversized payloads.
- Oversized decoder window requirements.
- Trailing data or concatenated frames if not explicitly allowed by the protocol.
- Invalid Mono1 padding.
- Invalid framebuffer layouts.

Verify the selected decoder API's actual behavior regarding frame boundaries, window limits, and output size enforcement.

A destination buffer limit alone must not be assumed to bound all decoder working memory.

### 12.2 Error recovery

A failed decode must not replace the last successfully decoded framebuffer.

Do not present partially decoded output.

If framing remains valid and recovery is explicitly supported, the next independent frame may restore normal operation.

Otherwise terminate the connection and reconnect.

Define which errors permit frame rejection and which require connection termination.

Do not silently fall back to raw decoding when a compressed payload is malformed.

## 13. Frame integrity

Preserve the existing framebuffer validation requirements.

Verify that decoded content:

- Has the expected byte length.
- Uses the negotiated pixel format.
- Preserves packed Mono1 representation.
- Preserves canonical Mono1 tail bits.
- Preserves every Gray8 luminance byte.
- Matches the expected viewport layout.

Evaluate whether production framing requires an additional checksum.

The benchmark's CRC32 is useful for verification, but it is not automatically a production protocol requirement.

Measure checksum overhead separately.

Do not describe CRC32 as cryptographic authentication.

## 14. Memory management

Reuse compression and decompression contexts between independent frames where supported.

Reuse bounded input, output, and scratch buffers.

Avoid repeated allocation of full-size framebuffers.

Preserve ownership separation between:

- Encoded network payload.
- Decoder scratch storage.
- Latest validated framebuffer.
- Framebuffer currently used for presentation.
- Last physically presented framebuffer, where applicable.

Ensure that decoder buffer reuse cannot modify a framebuffer still used by another task.

Ensure that failed decoding cannot corrupt the current presentation state.

Measure actual production memory consumption rather than extrapolating standalone benchmark RSS.

## 15. PaperSpoon integration

Integrate compression after framebuffer rendering and before transport publication.

```text
Render
    -> Frame coalescing
    -> Encoding selection
    -> Compression
    -> Network publication
```

Reuse the existing publication lifecycle.

Requirements:

- Preserve unchanged-frame suppression.
- Preserve event-driven rendering.
- Preserve existing application modes.
- Preserve Now Playing update scheduling.
- Preserve pointer processing.
- Preserve viewport negotiation.
- Preserve existing connection behavior.

Compression must not unnecessarily block:

- Input handling.
- Media acquisition.
- Playback commands.
- Connection management.

Evaluate whether synchronous compression is sufficiently inexpensive before introducing worker tasks.

If asynchronous compression is required, keep pending work bounded.

Avoid introducing a separate unbounded compression queue.

## 16. Backpressure and frame coalescing

Independent compressed frames must preserve existing frame-dropping behavior.

A frame can be discarded before transmission without invalidating any subsequent framebuffer.

Requirements:

- Prefer the latest desired framebuffer.
- Avoid compressing stale frames when possible.
- Avoid transmitting stale frames when they have not yet entered the network stream.
- Do not interrupt a partially written framed message unless terminating the connection.
- Do not accumulate unbounded encoded payloads.
- Preserve semantic input events independently of visual frame dropping.

Distinguish:

```text
Latest rendered framebuffer
Latest scheduled framebuffer
Latest encoded framebuffer
Latest transmitted framebuffer
Latest decoded framebuffer
Latest physically displayed framebuffer
```

Compression must not introduce accidental coupling between these states.

## 17. PaperPad integration

Decode transport frames before invoking existing display planning logic.

```text
Receive PPFB frame
    -> Validate framing
    -> Decode complete framebuffer
    -> Validate framebuffer
    -> Existing dirty-region planner
    -> Existing waveform policy
    -> Existing MXCFB presentation
```

Preserve:

- Mono1 support.
- Gray8 support.
- Mixed-waveform rendering.
- Binary UI updates.
- GC16 grayscale artwork rendering.
- DU, A2, and DU4 configuration.
- Existing local controls.
- Existing reconnect behavior.
- Existing framebuffer restoration on exit.

Do not modify waveform selection to account for transport compression.

Do not pass compressed payloads to MXCFB.

Do not treat compressed payload changes as physical display dirty regions.

The existing physical update planner must continue comparing actual decoded pixels.

## 18. PaperView integration

PaperView must use the same production transport decoder as PaperPad.

Verify:

- Raw PPFB v2 reception.
- Raw compression-capable protocol reception.
- Zstandard-compressed framebuffer reception.
- Interactive pointer input.
- Live Now Playing updates.
- Application Grid interactions.
- Mono1 presentation.
- Gray8 presentation.
- PNG export.
- Framebuffer equality between raw and compressed sessions.

Add optional diagnostics displaying:

- Negotiated protocol version.
- Current frame encoding.
- Raw framebuffer bytes.
- Encoded payload bytes.
- Compression ratio.
- Decoding latency.
- Received bytes per second.
- Received frames per second.

Keep diagnostics local to PaperView.

Do not modify source framebuffer pixels.

## 19. Configuration

Provide explicit transport compression configuration.

Suggested conceptual interface:

```text
compression = auto | raw | zstd
compression_level = 1
```

Adapt configuration syntax to existing CLI and environment conventions.

### Auto

- Negotiate compression support.
- Prefer Zstandard when beneficial.
- Fall back to Raw when necessary.

### Raw

- Disable framebuffer compression.
- Preserve a reproducible baseline for diagnostics.

### Zstd

- Request Zstandard-capable transport.
- Use Zstandard when supported and beneficial.
- Define explicit behavior when the peer does not support it.
- Retain per-frame raw fallback for incompressible content.

Do not silently change the requested compatibility policy.

Keep compression level configuration on PaperSpoon.

PaperPad must not require the user to select a matching compression level.

Initially keep compressed transport opt-in until validation is complete.

Make automatic negotiation the default only after compatibility and physical device tests pass.

## 20. Observability

Extend existing diagnostics rather than introducing an unrelated monitoring system.

### Connection diagnostics

Record:

- Protocol version.
- Negotiated capabilities.
- Selected compression configuration.
- Reconnection count.
- Protocol validation errors.

### Frame diagnostics

Record or aggregate:

- Raw framebuffer size.
- Encoded message size.
- Compression ratio.
- Encoding selection.
- Host encoding duration.
- Network publication duration.
- Receiver decoding duration.
- Frame validation duration.
- Dropped or coalesced frames.
- Raw fallback count.
- Decode failures.

Keep network timing separate from decoding and physical display timing.

Avoid logging every frame at normal verbosity.

Do not log raw framebuffer contents.

## 21. Performance validation

### 21.1 Host encoding

Measure production PaperSpoon compression performance on macOS ARM64.

Compare:

- Raw transport.
- Zstandard level 1.
- Zstandard level 3.
- Zstandard level 6.

Record:

- Encoding P50, P95, and P99.
- Encoded bytes.
- Memory consumption.
- Frame coalescing behavior.
- CPU utilization.
- Input responsiveness.

### 21.2 Kindle decoding

Measure production PaperPad decoding on PW6.

Include:

- Network payload parsing.
- Decompression.
- Framebuffer validation.
- Transfer into presentation ownership.

Report reconstruction timing separately from complete receiver processing.

Use the existing standalone benchmark results as a reference, not as a substitute for production measurements.

### 21.3 Network throughput

Compare Raw and Zstandard using equivalent frame sequences.

Test:

- Now Playing at 1 Hz.
- Application Grid.
- Interactive seeking.
- Rapid track changes.
- Mono1 animation.
- Gray8 animation.
- High-entropy content.

Record actual transmitted and received bytes.

Use network measurements that account for transport overhead where practical.

Distinguish application payload throughput from total network-interface traffic.

### 21.4 End-to-end latency

Measure the following stages independently where possible:

```text
Input event
    -> Host processing
    -> Frame rendering
    -> Compression
    -> Network transmission
    -> Decompression
    -> Framebuffer validation
    -> MXCFB submission
    -> MXCFB completion
```

For cross-device measurements, do not subtract timestamps from unsynchronized monotonic clocks.

Use local stage timings, a verified clock synchronization mechanism, or host-observed round-trip measurements.

Do not equate MXCFB submission or completion timing with independently observed visual panel response.

### 21.5 Provisional performance targets

| Workload | PW6 reconstruction P95 |
|---|---:|
| Now Playing at 1 Hz | Under 50 ms |
| Interactive updates at 4 Hz | Under 25 ms |
| Animation at 10 Hz | Under 10 ms |

The existing benchmark achieved less than 5 ms for full-frame Zstandard level 1 across its four recorded application and motion workloads.

Treat that result as a regression reference rather than a universal guarantee.

Investigate significant regressions using equivalent input data and timing boundaries.

## 22. Physical e-ink validation

After PaperView validation, test on the Kindle Paperwhite 6.

Verify:

- [x] Gray8 album artwork remains visually unchanged by transport compression.
- [x] Binary metadata remains readable.
- [x] Seekbar updates retain their existing waveform behavior.
- [x] Timer updates remain stable.
- [x] Mixed-waveform selection remains unchanged for equivalent framebuffers.
- [x] Playback controls remain responsive.
- [x] Repeated seeking works.
- [x] Track changes work.
- [x] Application transitions work.
- [x] Reconnection restores the latest framebuffer.
- [x] Local Exit remains responsive.
- [x] Framebuffer restoration remains correct.
- [x] Extended operation does not produce increasing memory consumption.
  Sampled RSS showed no sustained growth during the matched nine-minute runs.
  [Playback stability results](benchmarks/ppfb-v3-validation.md#matched-nine-minute-playback-stability)

Use raw transport as the behavioral baseline.

Identical decoded framebuffer sequences should result in equivalent physical refresh planning, subject to timing and frame coalescing differences.

Do not change the existing e-ink refresh policy solely to improve compression benchmark results.

## 23. Battery and long-running validation

Compression reduces network traffic but introduces CPU work.

Do not assume that lower network bandwidth guarantees longer battery life.

Compare Raw and Zstandard during controlled sessions.

Keep consistent:

- Media workload.
- Frame update rate.
- Album artwork.
- Physical waveform configuration.
- Screen illumination.
- Network conditions.
- Device background activity.
- Test duration.

Measure, where reliable telemetry is available:

- Battery discharge.
- CPU time.
- Network traffic.
- Memory usage.
- Reconnection behavior.
- Thermal behavior.

Perform extended playback tests to identify:

- Memory leaks.
- Codec context growth.
- Buffer growth.
- Connection instability.
- Gradual performance degradation.

Document measurement limitations.

Battery validation may remain a separate follow-up if reliable telemetry is unavailable.

## 24. Automated tests

Checked items have workspace test or controlled loopback evidence. The
transport tests replace a connection during frame encoding, close a peer after
it reads only part of the payload, and deliver fragmented input to PaperPad.
When the display loop pauses during a burst of 1,024 frames, the inbound
mailbox retains the latest complete frame only.
[PPFB v3 validation results](benchmarks/ppfb-v3-validation.md)

### Protocol negotiation

- [x] Existing v2 host and client behavior remains unchanged.
- [x] Compression-capable peers negotiate supported encodings.
- [x] Raw-only peers receive raw frames.
- [x] Unsupported encodings are never transmitted.
- [x] Unknown capability identifiers are handled safely.
- [x] Unsupported versions fail explicitly.
- [x] Reconnection performs fresh negotiation.
- [x] Legacy fallback does not introduce ambiguous framing.

### Frame encoding and decoding

- [x] Mono1 Raw round-trip.
- [x] Gray8 Raw round-trip.
- [x] Mono1 Zstandard round-trip.
- [x] Gray8 Zstandard round-trip.
- [x] Odd-width Mono1 round-trip.
- [x] Canonical Mono1 padding is preserved.
- [x] Realistic grayscale artwork remains byte-identical.
- [x] Multiple independent frames decode correctly.
- [x] Reused codec contexts preserve frame independence.
- [x] Raw and Zstandard frames can alternate within one supported connection.
- [x] Incompressible data selects Raw.

### Decoder robustness

- [x] Truncated messages are rejected.
- [x] Malformed Zstandard payloads are rejected.
- [x] Invalid encoding identifiers are rejected.
- [x] Invalid decoded lengths are rejected.
- [x] Oversized payloads are rejected.
- [x] Oversized window requirements are rejected.
- [x] Invalid dimensions are rejected.
- [x] Integer overflow is prevented.
- [x] Unsupported trailing data is rejected.
- [x] Decoder failures do not modify the last valid framebuffer.
- [x] Recovery follows documented connection semantics.
- [x] Malformed inputs do not panic.

### Transport lifecycle

- [x] Disconnect during compression.
- [x] Disconnect during payload transmission.
- [x] Disconnect during payload reception.
- [x] Slow receiver.
- [x] Slow network.
- [x] Rapid frame replacement.
- [x] Frame coalescing.
- [x] Reconnection after interrupted transmission.
- [x] No unbounded queue growth.
- [x] No stale-frame dependency.

### Application compatibility

- [x] Static framebuffer commands.
- [x] Application Grid.
- [x] Interactive Now Playing.
- [x] Album artwork changes.
- [x] Playback controls.
- [x] Seeking.
- [x] Mono1 rendering.
- [x] Gray8 rendering.
- [x] PaperView export.
- [x] Mixed-waveform classification.

## 25. Build and dependency verification

Use the existing pinned Rust and Nix environment.

Preserve the current ARMv7 target:

```text
armv7-unknown-linux-musleabihf
```

Verify the actual target and repository configuration before implementation.

Run relevant repository checks, including:

```sh
cargo fmt --all -- --check
cargo check --workspace --locked
cargo test --workspace --locked
git diff --check
make check
```

For native ARMv7 builds, use the repository's existing Nix workflow.

For Kindle deployment verification, use the existing build and ELF validation scripts.

Requirements:

- Successful macOS ARM64 build.
- Successful ARMv7 musl build.
- Successful static ELF validation.
- Successful execution on physical PW6.
- No unexpected native runtime dependencies.
- No unrelated toolchain upgrade.
- No mandatory benchmark dependency in production.

Measure production binary size before and after introducing Zstandard.

Report both stripped and unstripped sizes where relevant.

Do not use the benchmark executable size as a substitute for measuring PaperPad binary growth.

## 26. Documentation

Update the appropriate repository documentation.

Document:

- PPFB protocol changes.
- Wire format.
- Version negotiation.
- Encoding capabilities.
- Raw fallback behavior.
- Compression configuration.
- Decoder resource limits.
- Error handling.
- Compatibility matrix.
- PaperView testing workflow.
- Kindle deployment workflow.
- Production performance measurements.
- Known limitations.

Preserve the original compression benchmark report.

Add production measurements separately so standalone benchmark results and integrated transport results remain distinguishable.

## 27. Implementation milestones

PPFB v3 is the production default for compatible peers. It uses independent
full-frame Zstandard level 1 with per-frame Raw fallback. Automatic negotiation
retries PPFB v2 Raw on a fresh connection when a legacy host closes the v3
probe. The release accepts the recorded measurement limitations.
[Production conclusion](benchmarks/ppfb-v3-validation.md#production-conclusion)

### M0 — Additional evidence

- [x] Extend the artwork corpus.
- [x] Extend the motion corpus.
- [x] Evaluate Zstandard levels 1, 3, and 6.
- [x] Repeat representative PW6 measurements.
- [x] Confirm the initial production codec decision.

### M1 — Shared codec

- [x] Extract or reuse the benchmark's Zstandard implementation.
- [x] Implement independent framebuffer encoding.
- [x] Implement bounded framebuffer decoding.
- [x] Implement reusable codec contexts.
- [x] Implement Raw fallback selection.
- [x] Add correctness and malformed-input tests.
- [x] Verify ARMv7 compilation.

### M2 — Protocol evolution

- [x] Audit PPFB v2 compatibility.
- [x] Define the compression-capable wire format.
- [x] Implement explicit version negotiation.
- [x] Implement encoding capability negotiation.
- [x] Implement encoded framebuffer messages.
- [x] Add golden wire fixtures.
- [x] Add compatibility tests.
- [x] Document protocol behavior.

### M3 — PaperSpoon and PaperView

- [x] Integrate host-side compression.
- [x] Preserve frame coalescing.
- [x] Preserve input responsiveness.
- [x] Integrate the shared PaperView decoder.
- [x] Add compression diagnostics.
- [x] Verify interactive Now Playing.
- [x] Verify Application Grid.
- [x] Compare exported framebuffer pixels.

### M4 — PaperPad integration

- [x] Integrate the shared decoder.
- [x] Preserve existing framebuffer ownership.
- [x] Preserve mixed-waveform planning.
- [x] Verify static ARMv7 compilation.
- [x] Deploy to PW6.
- [x] Verify Raw and Zstandard sessions.
- [x] Verify reconnect and failure paths.

### M5 — Production measurements

- [x] Measure host encoding latency.
- [x] Measure PW6 reconstruction latency.
- [x] Measure receiver processing latency.
- [x] Measure actual network traffic.
- [x] Measure peak memory consumption.
- [x] Measure production binary size.
- [ ] Measure end-to-end responsiveness.
  The paired videos confirm a visible seekbar response in both modes. The
  device logs provide release-to-software-presentation timing. Exact
  touch-to-panel timing is unmeasured and deferred for this release.
  [Filmed Kindle seeks](benchmarks/ppfb-v3-validation.md#filmed-kindle-seeks)
- [x] Perform extended playback tests.
  Matched nine-minute runs covered device RSS, CPU time, and Wi-Fi traffic.
  Longer playback and controlled battery and thermal measurements remain open.

### M6 — Default activation and documentation

- [x] Review compatibility results.
- [x] Review physical device measurements.
- [x] Enable automatic PPFB v3 negotiation for compatible peers.
  Zstandard level 1 is preferred and Raw fallback is available. Legacy peers
  can use automatic downgrade or explicit PPFB v2.
- [x] Preserve explicit Raw configuration.
- [x] Complete protocol documentation.
- [x] Record production evidence.
- [x] Document unresolved limitations.

Do not proceed to inter-frame encoding within this milestone.

## 28. Future optimization roadmap

The benchmark demonstrated substantial potential for workload-specific patch encoding.

For Now Playing:

| Encoding | Mean bytes/frame | Decode P95 |
|---|---:|---:|
| Full Zstandard | 14,961 | 4.682 ms |
| 16-pixel tiles + Zstandard | 578 | 2.988 ms |

The tiled representation reduced mean payload size by approximately 25.9 times relative to full-frame Zstandard.

However, it introduces:

- Previous-frame storage.
- Reconstruction state.
- Additional memory consumption.
- Synchronization requirements.
- More complex recovery.
- Poorer performance on some dense-motion workloads.

Defer these changes to a separate milestone.

### Future Phase A — Absolute framebuffer patches

Investigate:

- Fixed-size dirty tiles.
- Dirty rectangles.
- Aggregated Zstandard-compressed patches.
- Absolute pixel values rather than mandatory XOR encoding.
- Compact tile metadata.

### Future Phase B — State synchronization

Introduce, if required:

- Frame identifiers.
- Base frame identifiers.
- Verified reconstruction state.
- Explicit reset behavior.
- Recovery keyframes.
- Backpressure-aware patch generation.

### Future Phase C — Adaptive encoding

Evaluate selection between:

- Raw independent frames.
- Zstandard independent frames.
- Zstandard dirty tiles.
- Zstandard dirty rectangles.

Prefer full-frame encoding for dense changes and patch encoding for localized updates when supported by measurements.

Do not execute every available encoder for every production frame merely to discover the smallest payload.

### Future Phase D — Display planner optimization

Investigate whether transport patch geometry can provide safe candidate regions for physical dirty-region detection.

Do not couple transport patch geometry directly to MXCFB waveform selection.

The physical display planner must continue accounting for the actual previous display state, framebuffer pixels, and hardware update constraints.

## 29. Additional engineering considerations

- **Compression level optimization:** exploit the host's substantially greater processing capacity only when higher compression levels provide meaningful network savings without increasing device decoding latency.

- **Codec context reuse:** reduce allocation pressure while preserving independently decodable Zstandard frames.

- **Decoder memory limits:** enforce Zstandard window constraints in addition to output buffer limits.

- **Binary size:** measure the actual contribution of compression dependencies to the deployed PaperPad executable.

- **Network backpressure:** investigate whether smaller payloads reduce stale-frame transmission and improve interactive response latency.

- **Remote transport:** evaluate operation under restricted bandwidth and increased latency without introducing remote-access or authentication changes in this milestone.

- **Long-running stability:** verify that persistent compression contexts and reusable buffers do not accumulate memory during extended playback.

- **Benchmark continuity:** keep production codec implementation and benchmark codec implementation aligned to prevent future measurements from testing a different execution path.

- **Hardware acceleration:** revisit NEON or dedicated acceleration only if production profiling identifies a meaningful decoding bottleneck.

## 30. Definition of done

The milestone is complete when:

- The production codec decision is supported by representative benchmark evidence.
- PaperSpoon supports independent Zstandard-compressed framebuffers.
- Raw transport remains available.
- Compression capabilities are negotiated explicitly.
- PaperPad and PaperView share compatible decoding behavior.
- Decoded framebuffers remain byte-for-byte identical.
- Existing PPFB v2 compatibility behavior is verified or explicitly documented where limited.
- Decoder memory and payload sizes are bounded.
- Invalid compressed data cannot corrupt presentation state.
- Existing frame coalescing remains functional.
- Existing mixed-waveform rendering remains unchanged.
- Static ARMv7 builds remain functional.
- Physical PW6 validation is complete.
- Actual network bandwidth savings are measured.
- Production reconstruction and end-to-end latency are measured.
- Memory and binary size impact are documented.
- Existing application behavior remains functional.
- Protocol documentation is complete.

The initial production release must prioritize independent framebuffer reconstruction, transport correctness, compatibility, and predictable device performance.

Full-frame Zstandard is the baseline production solution.

Dirty tiles, delta encoding, and adaptive inter-frame compression are subsequent optimization milestones, not requirements for the initial compressed transport release.

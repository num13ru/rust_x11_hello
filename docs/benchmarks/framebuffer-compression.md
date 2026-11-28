# Framebuffer compression benchmark

## Repository audit

- The shared `paper-protocol` crate represents Mono1 as MSB-first packed rows
  with white tail bits and Gray8 as tightly packed luminance bytes. The recorded
  Paperwhite 6 remote viewport is 1272×1624. At that size Gray8 uses 2,065,728
  bytes per frame and Mono1 uses 258,216. PPFB v2 adds 28 bytes of framing to
  either format, before TCP overhead.
- PaperSpoon renders the negotiated viewport and publishes formatted frames.
  Live Now Playing coalesces stale renders and skips identical frames. PaperView
  validates and retains raw frame bytes before presentation. Its PNG export
  expands Mono1 to 8-bit luminance for visual inspection.
- PaperPad receives and validates PPFB v2 frames before planning display updates.
  Its dirty-region planner splits updates by waveform compatibility. The MXCFB
  benchmarks depend on `/dev/fb0` and time device update submissions and
  completion. PR #19 measured display behavior on Paperwhite 6 firmware
  5.17.1.0.4. See the [device evidence](../archive/mxcfb-device-evidence.md#pixel-based-waveform-build-2026-11-27).
- The native Nix shell pins Rust 1.97.1 and installs
  `armv7-unknown-linux-musleabihf`. The native and container builds use
  `rust-lld`, static linking, and the same ARM hard-float ELF validation script.

## Reproducing the corpus

Run from the repository root with the pinned Rust toolchain. Pick a fresh
output directory outside the worktree. Generation requires enough space for
the raw sequences and publishes each file after the complete write succeeds.

```sh
out=$(mktemp -d)
cargo build --locked --release -p framebuffer-bench
for scenario in now-playing application-grid \
    motion-mono-4hz motion-mono-10hz motion-gray-4hz motion-gray-10hz \
    synthetic-mono synthetic-gray; do
    target/release/framebuffer-bench generate "$scenario" "$out/$scenario.fbcp" || break
    target/release/framebuffer-bench verify "$out/$scenario.fbcp" || break
done
```

| Sequence | Source and timing |
|---|---|
| `now-playing` | Gray8 PaperSpoon renderer with the fake media provider. Initial frame and 120 seconds of playback at 1 Hz, followed by seeks, pause, resume, new artwork and track, shuffle and repeat changes, and long metadata. The sequence uses a simulated clock. |
| `application-grid` | Mono1 PaperSpoon renderer and hit testing, with button presses at distant positions, a status-text transition, two diagnostic screens, and return to the grid. |
| `motion-{mono,gray}-{4,10}hz` | Procedurally rendered packed Mono1 or Gray8 at the device viewport. A moving shape on a stable background is followed by a full scene change and moving background. |
| `synthetic-{mono,gray}` | 67×37 correctness sequence with repeated frames, one changed pixel, a scanline, disconnected patches, checkerboard, gradient, seeded noise, and full replacements. |

The generated application sequences exercise the public renderers directly.
Each timestamp is a scenario sample time. Live PPFB publishing can coalesce or
skip some of those frames. The version 1 corpus format stores one layout per
file, so pixel format or viewport changes start a new sequence.

### Corpus file format

All integers are little-endian. The file begins with `FBCP`, a version byte
(`1`), a format byte (`1` for Mono1 or `2` for Gray8), the scenario-name length
as a byte, and a reserved zero byte. Then come `u16` width and height, `u32`
stride and frame count, and the ASCII scenario name. Each record contains a
zero-based `u32` index, `u64` relative timestamp in microseconds, `u32` raw
byte count, and the original framebuffer bytes. The verifier checks ordering,
timestamps, stride, size, completeness, and canonical Mono1 padding.

## Running the benchmark

```sh
bench=target/release/framebuffer-bench
"$bench" encode "$out/now-playing.fbcp" lz4 "$out/now-playing-lz4.fbce" \
    --warmup 1 --iterations 3 --csv
"$bench" verify "$out/now-playing.fbcp" "$out/now-playing-lz4.fbce"
"$bench" decode "$out/now-playing-lz4.fbce" --warmup 1 --iterations 3 --csv

"$bench" encode "$out/now-playing.fbcp" tiles-lz4 "$out/now-playing-tiles.fbce" \
    --tile-size 16 --keyframe 30 --warmup 1 --iterations 3
python3 tools/framebuffer-bench/run-matrix.py "$bench" "$out" "$out/matrix"
```

The encode command times compression and verifies each reconstruction against
the source bytes outside the timer. It saves one measured pass. The decode
command streams that file, times reconstruction, and validates the raw CRC32
and Mono1 padding outside the timer. `verify` also compares each decoded frame
and timestamp with the source corpus. The encoded file uses the corpus header
with `FBCE` magic followed by a codec byte, a mode byte, and a `u16` tile size.
Each frame has a 28-byte record header with the `u32` index, `u64` timestamp,
`u32` encoded length, `u32` raw CRC32, `u32` decoded payload length, and `u32`
base index. `0xffffffff` marks an independent frame. Other indices reference
the immediately previous verified frame. The reported payload size includes
the per-frame record header. The file header and TCP overhead are outside that
size. The CRC detects accidental corruption of reconstructed bytes.

The delta modes benchmark whole-frame XOR, aggregated absolute tiles,
individually compressed tiles, aggregated XOR tiles, and single or multiple
absolute rectangles. Rectangle and tile coordinates describe row-byte spans,
which preserve Mono1 packing even on partial edge tiles. `rects` merges
consecutive rows with equal horizontal extents into rectangles. The encoder
uses a full frame initially and at any requested keyframe interval. The
decoder advances its base after checksum and pixel-format verification.

Timing uses whole-frame samples and nearest-rank percentiles. Encode includes
delta generation, dirty-region detection, extraction, and compression for
the applicable modes. Decode includes codec work, delta reconstruction, and
patch application. Raw throughput divides reconstructed bytes by the sum of
timed samples. Loading time includes file reads and record parsing. Process
CPU time includes streaming, warm-up, verification, and output writes. Peak
RSS is the high-water mark of the entire process. Each matrix entry uses a
fresh process for encode and decode. The matrix exports metrics, bandwidth
projections, and adaptive comparisons as CSVs.

The benchmark builds independently of PaperPad. It uses `lz4_flex` 0.14.2 block
compression, `zstd` 0.14.5 with the `libzstd-rs-sys` Rust backend, `flate2` 1.1.10
with `miniz_oxide`, and `png` 0.18.1 as a reference. The PNG path writes Mono1 as
one-bit grayscale with inverted polarity and restores the original packed bits
on decode. Experimental RLE records a byte count and value for each run.

### ARMv7 build

```sh
nix develop -c cargo build --locked --release --no-default-features \
    --target armv7-unknown-linux-musleabihf -p framebuffer-bench
nix develop -c bash scripts/check-static.sh \
    target/armv7-unknown-linux-musleabihf/release/framebuffer-bench
```

`--no-default-features` omits the PaperSpoon corpus generators. The encoded
file and benchmark binary suffice for a decoder run. A Paperwhite 6 shell is
required to execute the binary and collect device measurements.

## Host results

Release build on macOS 26.7.1 ARM64 with Rust 1.97.1. One warm-up and three
measured passes per configuration. The 54 configurations per scenario produced
432 encoded sequences across eight scenarios. Every sequence was verified
against the raw corpus. The ARMv7 release binary passed the repository’s
static ARMv7-A EABI5 hard-float ELF check and was 2,003,176 bytes unstripped.
The macOS release binary without the scenario generators was 1,731,088 bytes.
With the buffer comparison option below, the corresponding binaries are
2,004,308 bytes and 1,731,312 bytes.

| Scenario | Encoding | Mean bytes/frame | Host encode P95 (µs) | Host decode P95 (µs) |
|---|---|---:|---:|---:|
| Now Playing | Raw | 2,065,756 | 29.67 | 28.96 |
| Now Playing | RLE | 363,440 | 933.12 | 426.83 |
| Now Playing | LZ4 | 43,764 | 138.92 | 227.96 |
| Now Playing | Zstandard level 1 | 14,961 | 346.62 | 222.50 |
| Now Playing | Zstandard level 6 | 12,000 | 1,141.67 | 202.29 |
| Now Playing | Deflate default | 25,355 | 2,862.21 | 209.38 |
| Now Playing | PNG | 14,084 | 3,220.46 | 1,213.92 |
| Now Playing | XOR + LZ4 | 9,526 | 122.42 | 191.25 |
| Now Playing | 32-pixel aggregated tiles + LZ4 | 1,362 | 110.33 | 50.62 |
| Now Playing | 32-pixel individual tiles + LZ4 | 1,642 | 240.58 | 66.08 |
| Now Playing | Bounding rectangle + LZ4 | 1,723 | 603.00 | 64.00 |
| Now Playing | Multiple rectangles + LZ4 | 1,396 | 592.08 | 39.46 |
| Application Grid | Full Zstandard level 1 | 811 | 20.25 | 25.38 |
| Mono1 motion at 10 Hz | Full Zstandard level 3 | 2,802 | 75.54 | 42.92 |
| Gray8 motion at 10 Hz | Full Zstandard level 6 | 4,061 | 533.29 | 67.92 |

Now Playing aggregated LZ4 tiles at 16, 32, 64, and 128 pixels averaged
1,255, 1,362, 1,458, and 2,160 bytes respectively. Aggregated 16-pixel
tiles with Zstandard level 1 averaged 578 bytes. Full-frame Zstandard was
competitive for the grid and motion sequences, while multiple rectangles
with Zstandard averaged 399 bytes on the grid and 128-pixel Zstandard tiles
averaged 1,720 bytes in the 10 Hz Mono1 sequence. The 10 Hz Gray8 sequence
included dense changes where RLE expanded to 2,423,114 bytes per frame.

Peak host decoder RSS for Now Playing was 3.91 MiB with full LZ4, 4.09 MiB
with full Zstandard level 1, 5.95 MiB with XOR + LZ4, and 6.86 MiB with
32-pixel aggregated LZ4 tiles. Inter-frame formats retain the last verified
frame in addition to the reconstruction output. Patch decoders also keep
decoded patch data or a reusable tile buffer.

### Buffer reuse

Run encode or decode with `--fresh-buffers` to release benchmark output and
scratch buffers before each measured frame. The input buffer, codec contexts,
and verified delta base stay available between frames. Allocations for output
and scratch occur inside the timer. The comparison used the same Now Playing
corpus with one warm-up and five measured passes. Both modes produced identical
encoded bytes for all four candidates.

| Encoding | Decode P95 with reused buffers (µs) | Decode P95 with fresh buffers (µs) | Decoder peak RSS reused / fresh (MiB) |
|---|---:|---:|---:|
| Full LZ4 | 234.46 | 229.00 | 3.92 / 3.94 |
| Full Zstandard level 1 | 238.04 | 224.29 | 4.11 / 4.14 |
| 16-pixel tiles + LZ4 | 29.92 | 31.12 | 6.80 / 8.05 |
| 16-pixel tiles + Zstandard level 1 | 35.88 | 32.33 | 7.06 / 8.31 |

Buffer reuse lowered peak decoder RSS for both tiled configurations in this
run. The P95 measurements varied in direction by codec.

### Bandwidth, keyframes, and adaptive selection

The matrix computes bytes/s as mean encoded bytes per frame times the
projected FPS. It divides by 1,024 and 1,048,576 for KiB/s and MiB/s, and
multiplies by eight and divides by 1,000,000 for Mbit/s. Multiplying bytes/s
by 3,600 gives the hourly projection before unit conversion. These estimates
include experimental record framing and exclude TCP and Wi-Fi overhead.

| Workload and rate | Raw | Full LZ4 | Full Zstandard level 1 | 32-pixel tiles + LZ4 |
|---|---:|---:|---:|---:|
| Now Playing at 1 FPS | 16.526 Mbit/s | 0.350 Mbit/s | 0.120 Mbit/s | 0.0109 Mbit/s |
| Mono1 motion at 10 FPS | 20.660 Mbit/s | 1.007 Mbit/s | 0.240 Mbit/s | 0.835 Mbit/s |
| Gray8 motion at 10 FPS | 165.260 Mbit/s | 2.163 Mbit/s | 0.385 Mbit/s | 3.869 Mbit/s |

On Now Playing, XOR + LZ4 averaged 9,526 bytes with an initial keyframe only,
12,829 with keyframes every 10 frames, 10,628 every 30, and 10,077 every 60.
An offline selector compared raw, full LZ4 and Zstandard, XOR + LZ4,
32-pixel tiles + LZ4, and a bounding rectangle + LZ4, adding one byte for a
per-frame variant ID. On Now Playing it averaged 1,009 bytes per frame,
26% below the smallest fixed candidate in that comparison. A row-sampling
heuristic averaged 2,179 bytes and took 18.92 µs per decision in Python on
the same sequence. It used more bytes than the smallest fixed candidate on
each of the six application and motion sequences.

### Limits and Kindle validation

The fake provider supplies simple artwork and the high-motion sequences use
procedural content. A diverse set of real artwork and video could change the
relative results. The reported timing covers complete codec operations.
Fourteen Now Playing, Application Grid, and odd-width Mono1 frames were
passed through PaperView's PPFB receiver and PNG export. Their luminance
matched the benchmark's PNG reference pixel-for-pixel. Separate phase
timing, physical display checks, and battery measurements remain pending.

## Paperwhite 6 decoder results

On November 27, 2026, the Paperwhite 6 completed 36 configurations and
4,779 timed frame reconstructions. The standalone ARMv7 musl binary ran from
KUAL with local files. Each configuration used a separate process, one
warm-up pass, three measured passes, and reusable buffers. The executable
read back over USB MTP matched SHA-256
`c6c89e18656fe1a4129efa113069541410a6ea5f964af1f373d70e593b9c9b34`.
The Kindle independently checked the executable and all 36 encoded files
against the upload manifest. The run status was `COMPLETE`. Every device
reconstruction passed its pixel checksum, and the same encoded files had
already been verified byte-for-byte against the original host corpus.

The device reported firmware 5.17.1.0.4, Linux 5.15.41-lab126, and two
ARMv7 processors on a MediaTek Cortex-A7 platform. CPU flags included NEON,
VFPv4, and CRC32. It reported 979,852 KiB total memory and 637,524 KiB
available before the run. The kernel registered generic LZO, LZO-RLE, and
Deflate implementations. The benchmark used userspace codecs. Hardware
compression capabilities and the performance effect of NEON remain
unverified.

Each cell shows the mean payload bytes per frame, including the 28-byte
record header, followed by the measured decoder P95 in milliseconds.

| Encoding | Now Playing | Application Grid | Mono1 motion at 10 Hz | Gray8 motion at 10 Hz |
|---|---:|---:|---:|---:|
| Raw | 2,065,756 B / 2.451 ms | 258,244 B / 0.148 ms | 258,244 B / 0.266 ms | 2,065,756 B / 2.886 ms |
| Full LZ4 | 43,764 B / 7.250 ms | 2,324 B / 0.997 ms | 12,589 B / 1.287 ms | 27,037 B / 7.789 ms |
| Full Zstandard level 1 | 14,961 B / 4.682 ms | 811 B / 0.336 ms | 3,004 B / 0.817 ms | 4,810 B / 3.548 ms |
| Deflate fast | 64,846 B / 6.716 ms | 3,348 B / 0.918 ms | 13,175 B / 1.553 ms | 48,323 B / 6.499 ms |
| PNG | 14,084 B / 13.888 ms | 2,656 B / 3.536 ms | 6,478 B / 4.237 ms | 14,011 B / 19.374 ms |
| XOR + LZ4 | 9,526 B / 10.291 ms | 1,728 B / 1.875 ms | 9,111 B / 2.152 ms | 50,681 B / 15.944 ms |
| 16-pixel tiles + LZ4 | 1,255 B / 2.883 ms | 13,413 B / 4.829 ms | 39,013 B / 5.359 ms | 58,390 B / 13.567 ms |
| 16-pixel tiles + Zstandard level 1 | 578 B / 2.988 ms | 3,204 B / 4.865 ms | 7,506 B / 5.378 ms | 15,808 B / 13.762 ms |
| Multiple rectangles + LZ4 | 1,396 B / 3.003 ms | 1,058 B / 1.236 ms | 15,712 B / 2.110 ms | 14,082 B / 7.622 ms |

### Now Playing timing and memory

Process CPU time covers all four passes, file loading, and verification.
Peak RSS is the process high-water mark. The latency percentiles time
reconstruction with file I/O and checksum verification outside the timer.

| Encoding | P50 (ms) | P99 (ms) | Max (ms) | Process CPU (s) | Peak RSS (MiB) |
|---|---:|---:|---:|---:|---:|
| Raw | 1.771 | 3.661 | 5.814 | 7.136 | 4.42 |
| Full LZ4 | 7.088 | 7.816 | 7.957 | 6.561 | 2.57 |
| Full Zstandard level 1 | 4.433 | 4.772 | 4.837 | 5.196 | 2.94 |
| XOR + LZ4 | 10.024 | 11.672 | 15.471 | 8.080 | 4.59 |
| 16-pixel tiles + LZ4 | 2.614 | 6.989 | 7.985 | 4.272 | 5.43 |
| 16-pixel tiles + Zstandard level 1 | 2.761 | 4.644 | 7.266 | 4.333 | 5.80 |
| Multiple rectangles + LZ4 | 2.596 | 6.741 | 7.239 | 4.230 | 5.38 |

### Implications for production

All nine tested encodings met the provisional P95 targets for Now Playing
(50 ms), Application Grid (25 ms), and Mono1 motion (10 ms). On the 10 Hz
Gray8 sequence, PNG, XOR + LZ4, and both 16-pixel tile configurations
exceeded 10 ms. Full Zstandard level 1 met the target on all four workloads
and beat full LZ4 on both payload size and P95 in each.

Use full-frame Zstandard level 1 as the first production transport candidate,
with raw fallback. It kept P95 below 5 ms on all four workloads, required
no previous framebuffer for reconstruction, and used 2.94 MiB peak RSS in
the Now Playing decoder process. Validate this result with a broader artwork
and motion corpus before changing PPFB. For localized Now Playing updates,
16-pixel Zstandard tiles averaged 578 bytes versus 14,961 bytes for full
Zstandard and reduced P95 from 4.682 to 2.988 ms. They raised peak decoder
RSS to 5.80 MiB and exceeded the 10 Hz target on Gray8 motion. Treat them as
a later workload-specific experiment that also needs recovery and
synchronization testing.

This was one physical run with procedural motion and fake-provider artwork.
Other host-tested codec levels and tile sizes, network and physical refresh
latency, battery impact, and repeated device-run variance remain open.

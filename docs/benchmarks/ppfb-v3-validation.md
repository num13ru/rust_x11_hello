# PPFB v3 validation

## Expanded corpus and host measurements

These standalone benchmark results supplement the [original compression
report](framebuffer-compression.md). The benchmark runs on macOS ARM64 with
Rust 1.97.1. Each case uses one warm-up pass and three timed passes in release
mode. All decoded files were checked byte-for-byte against the source corpus.

The benchmark uses the repository's existing 960×640 JPEG fixture. The
Now Playing sequence uses the PaperSpoon renderer and fake playback clock,
with darker, brighter, and higher-contrast variants, quick artwork swaps,
and a missing-artwork interval. The motion sequences scroll the PaperSpoon
application grid, then cut to a panning image and a dense inverted scene.
Mono1 uses packed thresholded pixels and Gray8 retains luminance. These are
deterministic workloads using one photograph and scripted motion.

| Workload | Codec | Mean bytes/frame | Host encode P95 (ms) | Host decode P95 (ms) |
|---|---|---:|---:|---:|
| JPEG Now Playing | Raw | 2,065,756 | 0.030 | 0.030 |
| JPEG Now Playing | Zstandard 1 | 333,964 | 1.094 | 0.596 |
| JPEG Now Playing | Zstandard 3 | 306,074 | 4.098 | 0.562 |
| JPEG Now Playing | Zstandard 6 | 288,969 | 6.046 | 0.510 |
| Mono1 scrolling at 10 Hz | Raw | 258,244 | 0.006 | 0.003 |
| Mono1 scrolling at 10 Hz | Zstandard 1 | 15,803 | 0.135 | 0.074 |
| Mono1 scrolling at 10 Hz | Zstandard 3 | 17,003 | 0.192 | 0.081 |
| Mono1 scrolling at 10 Hz | Zstandard 6 | 15,877 | 0.874 | 0.068 |
| Gray8 scrolling at 10 Hz | Raw | 2,065,756 | 0.033 | 0.029 |
| Gray8 scrolling at 10 Hz | Zstandard 1 | 892,593 | 2.762 | 1.488 |
| Gray8 scrolling at 10 Hz | Zstandard 3 | 821,928 | 5.762 | 1.547 |
| Gray8 scrolling at 10 Hz | Zstandard 6 | 759,696 | 16.958 | 1.578 |

Level 1 used the least host encoding time among the tested Zstandard levels
on every workload. Level 6 reduced the mean payload by a further 13.5% for
JPEG Now Playing and 14.9% for Gray8 motion. In this run, level 1 used 16.2%
of raw bandwidth for JPEG Now Playing versus 0.72% for the previous fake-artwork
run. The results support keeping level 1 as the initial candidate. Two complete Kindle passes for the expanded workloads and higher levels are
recorded below.

The baseline PaperPad executable passed the repository's static ARMv7-A
hard-float ELF check at 1,230,900 bytes unstripped and 789,052 bytes after
`llvm-strip --strip-all`. Use the same pinned Rust 1.97.1 `rust-lld` build
and stripping method when comparing the integrated build.

## Reproduction

From the repository root, use an output directory with at least 1 GiB
available. For local artwork, `now-playing-artwork` accepts up to four JPEGs.
Each scrolling scenario accepts one JPEG. Files are kept outside Git.

```sh
cargo build --locked --release -p framebuffer-bench
out=$(mktemp -d)
bench=target/release/framebuffer-bench
art=tools/paperspoon/assets/grayscale-example.jpg
for scenario in now-playing-artwork scroll-mono-10hz scroll-gray-10hz; do
    "$bench" generate "$scenario" "$out/$scenario.fbcp" "$art" || exit 1
    for codec in raw zstd-1 zstd-3 zstd-6; do
        payload="$out/$scenario-$codec"
        "$bench" encode "$out/$scenario.fbcp" "$codec" "$payload.fbce" \
            --warmup 1 --iterations 3 --csv > "$payload.encode.csv" || exit 1
        "$bench" verify "$out/$scenario.fbcp" "$payload.fbce" || exit 1
        "$bench" decode "$payload.fbce" --warmup 1 --iterations 3 --csv \
            > "$payload.decode.csv" || exit 1
    done
done
```

For the device comparison, build `framebuffer-bench` with
`--release --no-default-features --target armv7-unknown-linux-musleabihf` in
the pinned Nix shell. Copy the statically verified binary and the `.fbce`
files to the Kindle. Run `framebuffer-bench decode FILE --warmup 1
--iterations 3 --csv` in three separate processes per file. Record available
memory, readable thermal sensor values, and background activity. Compare
the run-level P95 range, process CPU time, and peak RSS for each case.

## Integrated desktop and ARMv7 validation

On macOS ARM64, PaperView's network worker received the application grid,
Mono1 pointer press and release frames, a Gray8 diagnostic gradient, and live
fake-provider Now Playing updates from PaperSpoon over loopback. The test
covered v2, v3 Raw, and v3 with optional and required Zstandard. The grid and
gradient pixels and their PNG exports matched across all four sessions.
Loopback results measure PPFB bytes and local write timing.

| Frame at 1272×1624 | Raw message (bytes) | Zstandard level 1 message (bytes) |
|---|---:|---:|
| Application Grid at connection | 258,244 | 963 |
| Gray8 diagnostic gradient | 2,065,756 | 1,197 |
| Initial fake Now Playing | 2,065,756 | 15,003 |

The default Nix build produced and verified a static ARMv7-A hard-float
PaperPad executable. It occupied 1,510,688 bytes unstripped and 1,001,772
bytes stripped, increases of 279,788 and 212,720 bytes respectively over
the baseline built with the same toolchain. PaperPad uses the same Zstandard
Rust backend as the benchmark. Other benchmark codecs stay in the benchmark
package. Raw v2 remains the default while rollout validation continues.

### Kindle staging

On November 28, 2026, the canonical MTP update installed the integrated
PaperPad build. Independent USB readback matched the verified binary
at SHA-256 `5c82749d8b53b848c3e03eb168c48998348507619a0f715a7c51409550581795`.
The rollback readback matched the artifact read before the update.
The verified KUAL menu offers a separate PPFB v3 DU action.

The standalone decoder comparison was transferred separately. All twelve
expanded-corpus payloads passed MTP readback verification. Its KUAL action
runs three repeats and saves the result CSV, available memory, and readable
thermal values with a completion status. The repeat launcher published two complete passes and two rows of the third.

### Live Paperwhite 6 session

The operator confirmed that the Application Grid and Now Playing widget
displayed correctly. PaperSpoon and PaperPad negotiated v3 with Raw and
Zstandard over Wi-Fi. The host and device independently recorded the same
PPFB byte count for all 144 received frames. Every received frame used
Zstandard level 1 at 1272×1624. The raw reference column shows the equivalent
28-byte-header frame size for the same pixel format.

| Workload | Received frames | Raw reference (bytes/frame) | Mean measured wire bytes/frame | Host encode P95 (ms) | PW6 decode P95 (ms) | PW6 receive + decode P95 (ms) |
|---|---:|---:|---:|---:|---:|---:|
| Application Grid | 107 | 258,244 | 996 | 0.121 | 1.756 | 1.781 |
| Fake-provider Now Playing | 37 | 2,065,756 | 14,430 | 1.588 | 14.387 | 14.548 |

The Gray8 session used 0.70% of the equivalent raw PPFB frame bytes. These
counts cover application messages over the live connection. The raw sizes
are calculated from the framebuffer layout. A subsequent paired run
measured interface traffic with Raw and Zstandard.
[Matched Wi-Fi results](#matched-wi-fi-sessions-on-paperwhite-6)

On the Kindle, Now Playing decode P50 was 13.874 ms, P99 was 14.523 ms, and
the maximum was 14.523 ms. The production timer includes allocating the
owned output buffer. Final pixel validation was timed separately and had a
P95 of 3 µs. The decode P95 met the 50 ms Now Playing target. It was higher
than the original standalone benchmark's 4.682 ms, which reused its output
buffer and used a separate sequence. Compare identical payloads and
allocation modes on the same device before attributing the difference.

Native evdev input reached PaperSpoon. Pause, Play, and Next completed with
the fake provider, and the device submitted subsequent Gray8 frames. The
MXCFB log showed three GC16 artwork submissions and 40 DU submissions for
Gray8 UI updates. Local Exit released the evdev grab, restored the original
framebuffer snapshot, and produced a stopped status with exit code 0. The
log verifies requested waveforms and the cleanup path. The paired session
below adds seeking and a short memory check. Physical panel completion and
battery measurements remain open.


### Expanded corpus on Paperwhite 6

The decoder launcher recorded 26 of 36 planned rows. Its status remained
`RUNNING` with no progress during the follow-up observation. The device
manifest verified the binary and all 12 inputs. The 26 saved rows matched
the host scenario, codec, frame count, and payload size statistics. The
table uses the first two complete passes for a balanced comparison and
excludes the two extra rows from the incomplete third pass. Those two
complete passes contain 5,016 timed reconstructions. Available memory was
645,920 and 648,976 KiB at the start of the two passes. Thermal readings are
unattributed to a specific sensor.

| Workload | Encoding | Decode P50 (ms) | P95 (ms) | P99 (ms) | Max (ms) | Process CPU (s) | Peak RSS (MiB) |
|---|---|---:|---:|---:|---:|---:|---:|
| JPEG Now Playing | Raw | 1.79–1.80 | 2.35–2.35 | 2.56–2.68 | 4.89–5.80 | 7.14–7.16 | 4.39–4.39 |
| JPEG Now Playing | Zstandard 1 | 11.24–11.27 | 11.52–11.53 | 11.75–12.02 | 13.39–14.03 | 8.44–8.54 | 3.31–3.31 |
| JPEG Now Playing | Zstandard 3 | 11.67–11.69 | 12.46–12.50 | 12.96–13.02 | 13.19–13.74 | 8.62–8.67 | 3.28–3.28 |
| JPEG Now Playing | Zstandard 6 | 11.74–11.78 | 12.17–12.18 | 12.67–12.86 | 13.62–14.82 | 8.52–8.57 | 3.26–3.26 |
| Gray8 scrolling at 10 Hz | Raw | 2.17–2.17 | 2.70–2.75 | 2.89–2.99 | 3.01–3.14 | 1.88–1.89 | 4.50–4.50 |
| Gray8 scrolling at 10 Hz | Zstandard 1 | 30.22–30.24 | 31.68–31.73 | 32.20–32.65 | 36.20–38.26 | 4.82–4.82 | 4.23–4.23 |
| Gray8 scrolling at 10 Hz | Zstandard 3 | 33.06–33.10 | 36.00–36.19 | 37.24–38.84 | 46.66–46.98 | 5.16–5.30 | 4.12–4.12 |
| Gray8 scrolling at 10 Hz | Zstandard 6 | 39.28–39.40 | 41.63–41.72 | 42.14–42.80 | 43.13–49.92 | 5.67–5.68 | 4.02–4.02 |
| Mono1 scrolling at 10 Hz | Raw | 0.24–0.24 | 0.26–0.27 | 0.34–0.65 | 0.34–0.66 | 0.24–0.24 | 1.05–1.05 |
| Mono1 scrolling at 10 Hz | Zstandard 1 | 0.98–0.98 | 1.09–1.11 | 1.21–1.29 | 1.27–1.47 | 0.26–0.26 | 1.23–1.23 |
| Mono1 scrolling at 10 Hz | Zstandard 3 | 1.11–1.13 | 1.18–1.25 | 1.29–1.50 | 1.62–1.66 | 0.28–0.28 | 1.23–1.23 |
| Mono1 scrolling at 10 Hz | Zstandard 6 | 1.00–1.02 | 1.13–1.21 | 1.28–1.32 | 1.37–1.46 | 0.27–0.27 | 1.23–1.23 |

Level 1 stayed below 12 ms P95 for JPEG Now Playing and below 1.2 ms for
Mono1 scrolling. On dense Gray8 scrolling it took 31.68–31.73 ms P95,
above the 10 ms animation target. Levels 3 and 6 took longer on that
sequence. These results support keeping level 1 and retaining compression
as opt-in while dense Gray8 workloads and the production session are
investigated further.

## Decoder and transport follow-up

The shared decoder now writes directly into the output allocation when its
capacity equals the validated pixel length. It retains a bounded slice for
a larger reused allocation. Both paths enforce the same decoded size and
clear scratch after a codec failure. The frame still owns its pixels before
presentation. Including the opt-in measurement diagnostics, the static
ARMv7 build passed verification at 1,518,072 bytes unstripped and 1,007,340
bytes after `llvm-strip --strip-all`.

### Identical-payload decoder comparison

The baseline (`63a8c84`) and revised decoder read the same level-1 `.fbce`
files on macOS ARM64. Each cell is the median of five per-process P95
measurements. Each process used one warm-up and five timed passes. The
fresh-buffer mode exercises the allocation pattern used by the production
frame decoder.

| Workload | Baseline fresh P95 (µs) | Revised fresh P95 (µs) | Baseline reused P95 (µs) | Revised reused P95 (µs) |
|---|---:|---:|---:|---:|
| Fake Now Playing | 257.38 | 241.92 | 242.83 | 235.17 |
| JPEG Now Playing | 633.12 | 612.46 | 609.21 | 591.25 |
| Gray8 scrolling | 1,680.04 | 1,618.67 | 1,528.54 | 1,517.25 |

Some run-level ranges overlap. The host results show a small reduction and
leave the larger observed Kindle latency difference unexplained. The same
inputs and two independently built static ARMv7 decoders can repeat this
comparison on Paperwhite 6. Run one workload at a time to save a complete
CSV for each case.

### Matched PaperSpoon loopback sessions

Each of four PPFB v3 sessions sent the same application grid, repository
JPEG rendered as Gray8, and Gray8 diagnostic gradient at 1272×1624. Each
workload contributed 30 measured frames after two warm-up cycles. The
receiver counted complete PPFB messages and verified that the decompressed
pixels matched the raw session. Each cell shows bytes per frame and the
sender's encode P95. Encode timing starts after JPEG acquisition and includes
diagnostic generation for the gradient. These are application bytes sent
over TCP loopback. Wi-Fi interface traffic and device latency are measured
separately.

| Workload | Raw bytes / encode P95 (ms) | Zstandard 1 | Zstandard 3 | Zstandard 6 |
|---|---:|---:|---:|---:|
| Application Grid | 258,244 / 0.007 | 963 / 0.029 | 1,100 / 0.034 | 1,093 / 0.092 |
| JPEG diagnostic | 2,065,756 / 0.034 | 741,297 / 1.667 | 681,840 / 4.474 | 641,061 / 10.036 |
| Gray8 gradient | 2,065,756 / 1.037 | 1,197 / 1.181 | 592 / 2.904 | 529 / 1.722 |

Live fake Now Playing accepted three loopback pointer seeks, including a
backward seek. Automated tests verified Raw fallback between two Zstandard
frames on one PaperSpoon connection and PaperPad renegotiation after a
truncated compressed frame.

The paired Kindle measurement actions record process CPU and memory alongside
network interface counters.
[Kindle measurement instructions](../ppfb-v3.md#matched-kindle-measurements)

### Identical-payload decoder comparison on Paperwhite 6

All three comparisons completed. USB readback matched both distinct
ARMv7 executables to their host builds. The device verified each input
checksum before decoding. Each cell is the range of two per-process
P95 results, with one warm-up pass and one timed pass per process.
The `fresh` rows allocate a new output buffer for each frame, as the
production receiver does. Both builds used the same level-1 payloads.

| Workload | Buffer | Baseline P95 (ms) | Revised P95 (ms) |
|---|---|---:|---:|
| Fake Now Playing | Fresh | 6.43–6.55 | 5.00–5.02 |
| Fake Now Playing | Reused | 4.90–4.92 | 3.97–3.97 |
| JPEG Now Playing | Fresh | 13.19–13.21 | 12.10–12.15 |
| JPEG Now Playing | Reused | 11.55–11.58 | 10.79–10.80 |
| Gray8 scrolling | Fresh | 32.91–33.23 | 33.52–33.53 |
| Gray8 scrolling | Reused | 31.72–31.72 | 30.98–30.99 |

Eliminating the extra zero fill improved the two Now Playing workloads.
Dense Gray8 remained above the 10 ms target in both buffer modes.
The fresh-buffer change helps explain part of the allocation cost. The live
Now Playing renderer uses a different sequence of frames, so its production
decode timings should be compared within the paired run below. This evidence
supports retaining Zstandard level 1 and keeping compression opt-in.

The refreshed PaperPad build and paired measurement menu were deployed by
the canonical MTP update. Independent readback matched the binary SHA-256
`638d9b15aa2a6653b6846f2856a4fa9e919245e4d2e7694a542f8b561b38ca38`,
and the retained rollback matched the previous deployment.

### Matched Wi-Fi sessions on Paperwhite 6

On 2026-11-28, the same deployed PaperPad binary used PPFB v3 with forced Raw
and with Zstandard level 1. Both sessions used the same 1272×1624 viewport,
fixture, fake Now Playing provider, and DU with GC16 for grayscale. Each
session cycled through the same JPEG, gradient, and Application Grid stimuli
six times, then streamed the live widget. The live comparison uses the first
60 received widget frames in each run, spanning 59.27 s for Raw and 59.19 s
for Zstandard. PaperSpoon and PaperPad logged identical PPFB byte counts for
all 108 Raw and 377 Zstandard frames received over the full sessions.

The table uses mean PPFB bytes per frame and nearest-rank P95 timings. Receive
and decode starts when the first frame bytes are available to the device reader
and excludes idle time before the frame. The six-frame P95s are sample maxima.

| Workload | Frames per mode | Raw bytes/frame | Zstd bytes/frame | Raw receive + decode P95 (ms) | Zstd receive + decode P95 (ms) | Zstd decode P95 (ms) |
|---|---:|---:|---:|---:|---:|---:|
| JPEG diagnostic | 6 | 2,065,756 | 741,297 | 260.98 | 416.71 | 31.07 |
| Gray8 gradient | 6 | 2,065,756 | 1,197 | 290.07 | 6.65 | 6.63 |
| Application Grid | 6 | 258,244 | 963 | 35.61 | 1.68 | 1.66 |
| Fake Now Playing | 60 | 2,065,756 | 14,997 | 321.02 | 13.59 | 13.51 |

Now Playing used 99.27% fewer PPFB bytes per frame. Five of the six compressed
JPEG frames finished receiving and decoding in 76–110 ms. One took 416.71 ms, including 23.64 ms
of decode. Repeat runs are needed to characterize JPEG tail latency.

Traffic and CPU time are deltas from measurement start to the first sample
after 60 s, including the scripted frames. Current and peak RSS are the
values at that sample. The `wlan0` counters include other interface traffic.

| Measurement | Raw | Zstandard level 1 |
|---|---:|---:|
| Sample elapsed (s) | 60.652 | 60.817 |
| `wlan0` RX (bytes) | 114,209,926 | 5,414,233 |
| `wlan0` TX (bytes) | 1,210,871 | 129,413 |
| PaperPad user + system CPU time (s) | 5.45 | 5.36 |
| Current RSS (KiB) | 6,900 | 7,916 |
| Peak RSS (KiB) | 13,056 | 14,184 |

Zstandard ran for 364.69 s and received 358 live widget frames. Sampled RSS
from 30 s onward ranged from 7,720 to 7,944 KiB and ended at 7,752 KiB.
Peak RSS rose to 15,912 KiB at the 214 s sample and held there through exit.
Raw ran for 110.20 s and ended with a peak RSS of 13,064 KiB. The unequal
run lengths limit the peak-memory comparison. The six-minute run is a short
stability check. A longer matched run and controlled battery and thermal
measurements remain open.

Five physical seek releases reached the fake provider and succeeded, including
backward seeks. On the device's monotonic clock, the first subsequent
`frame_present_return` was 46–220 ms after input receipt. The frame IDs were
correlated by event order. This timing covers software presentation return.
A Raw seek comparison and camera-observed panel completion remain open.

Both runs logged DU and GC16 submissions and stopped with status 0 after
releasing evdev and restoring the 2,157,312-byte framebuffer snapshot. PPFB v2
stays the default, with level 1 as the initial Zstandard option, while dense
Gray8 exceeds its decode target.

### Kindle reconnect after a PaperSpoon restart

On 2026-11-28, the Paperwhite 6 ran **Run Paperpad DU with PPFB v3 (5m)** over
Wi-Fi using the same verified PaperPad build as the matched sessions. PaperSpoon
required PPFB v3 with Zstandard. It sent a checkerboard, stopped for five
seconds, and restarted on the same port while PaperPad stayed open. The Kindle
logged the connection indicator change from connected to disconnected and back.
PaperSpoon logged a fresh v3 Hello offering Raw and Zstandard on each TCP
connection.

The Kindle recorded matching Zstandard frame sizes and software presentation
returns for the 1,737-byte checkerboard before restart and the 27,044-byte
Gray8 bars after reconnect. The operator confirmed the bars were visible.
Local Exit returned status 0, released evdev, and restored the framebuffer
snapshot. The replacement PaperSpoon process started in its default application
state. A separate test covered replay from a running process.
[Retained frame replay after a TCP disconnect](#retained-frame-replay-after-a-tcp-disconnect)

### Retained frame replay after a TCP disconnect

On 2026-11-28, the Paperwhite 6 ran **Run Paperpad DU with PPFB v3 (5m)** on the
same verified PaperPad build. A temporary TCP relay let the host close one
Wi-Fi connection and refuse new connections for five seconds. The test copy of
PaperSpoon advertised the relay port through normal discovery. Its frame sender
and process stayed running throughout.

PaperSpoon and PaperPad negotiated PPFB v3 and Zstandard on both connections.
PaperSpoon sent a 27,044-byte Gray8 bars frame before the disconnect. On the
second Hello, it replayed the retained frame with a fresh ID and a byte-identical
compressed payload. The Kindle recorded both frame receptions and software
presentation returns, with the connection indicator going from connected to
disconnected and back. The display backend recognized the replay as unchanged
and updated its cache. The operator confirmed the bars were visible after
reconnect. Local Exit returned status 0, released evdev, and restored the
framebuffer snapshot.

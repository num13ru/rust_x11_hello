# MXCFB benchmarks

This document holds the maintained MXCFB benchmark method and latest compact
result set. Dated trials, artifact hashes, failures, and earlier approaches are
kept in [the device evidence archive](archive/mxcfb-device-evidence.md).

## Current result

The latest results were recorded on 2026-10-26 on a Kindle Paperwhite 6
(Sangria / Bellatrix4), firmware 5.17.1.0.4, with a 1272 x 1624 update region.
Each KUAL action performed one uncounted warmup followed by 12 serialized,
alternating full-black and full-white measured updates.

| Requested waveform | Kernel-timed throughput | p50 | p95 / max | 60 updates/s software budget |
| --- | ---: | ---: | ---: | --- |
| A2 | 9.006 updates/s | 106.337 ms | 121.368 ms | Not met |
| DU4 | 6.836 updates/s | 144.166 ms | 153.626 ms | Not met |
| DU | 6.707 updates/s | 151.797 ms | 158.576 ms | Not met |
| GC16 | 3.718 updates/s | 271.397 ms | 275.620 ms | Not met |

Under these conditions, serialized full-screen 60 FPS is infeasible. A2 was
the fastest tested request at approximately 9 completed updates/s. These are
kernel-timed completion results, not camera-verified visual FPS: completion
markers do not establish when visible pigment stopped moving, pixel fidelity,
or acceptable ghosting.

Across the nine marker-specific launches in the cumulative device log, all 108
measured samples completed. There were no missing or foreign-marker failures,
immediate returns, timeout-range waits, or cleanup failures. Each run restored
the 2,157,312-byte framebuffer snapshot and submitted a full-screen GC16
cleanup refresh. The operator confirmed that the original screen visibly
returned after every benchmark.

## Dirty-region sweep

The next tuning pass keeps the proven A2 marker-specific completion path but
limits both framebuffer writes and submitted updates to centered 512 x 512,
256 x 256, and 64 x 64 rectangles. These results must be reported with their
region sizes and must not replace the full-screen baseline above. The KUAL
actions are present, but no physical-device result has been recorded yet.

The measured region remains wholly inside the remote viewport. The local Exit
strip is unchanged, while cleanup still restores the complete 2,157,312-byte
snapshot and performs a full-screen GC16 refresh. An invalid, zero-sized, or
out-of-viewport region fails before framebuffer bytes are changed.

## What is measured

The benchmark:

1. validates and snapshots the readable and writable visible framebuffer;
2. opens a full-screen X11 InputOutput window for touch, local Exit, and
   clipping of lower Kindle windows, without requesting an X11 background or
   creating a graphics context;
3. draws and waits for the local system strip;
4. performs one uncounted warmup update;
5. alternates solid black and white over the remote viewport;
6. submits one update and calls marker-specific
   `MXCFB_WAIT_FOR_UPDATE_COMPLETE` before preparing the next measured frame;
7. restores the snapshot and submits a full-screen GC16 cleanup refresh.

Each sample records `prepare_write_submit_us`, `completion_wait_us`, and
`total_us`. `software_60fps_budget_met` is true only if every measured total is
at most 16,667 microseconds and aggregate throughput is at least 60 updates/s.
`visual_60fps_verified` remains false without camera evidence.

The matching driver returns zero for a completed marker, a missing marker, and
its five-second timeout. PaperPad therefore rejects waits shorter than 1 ms as
ambiguous immediate or missing returns and waits of at least 4.5 seconds as
ambiguous timeout-range returns. Only elapsed waits between those bounds are
classified as attributable kernel waits.

## Running it

KUAL exposes separate full-screen **Benchmark MXCFB GC16**, **DU**, **A2**, and
**DU4** actions, plus A2 actions for 512 x 512, 256 x 256, and 64 x 64 centered
regions. Direct command-line use on the Kindle is:

```sh
PAPERPAD_MXCFB_BENCHMARK_WAVEFORM=a2 \
PAPERPAD_MXCFB_BENCHMARK_REGION=256x256 \
PAPERPAD_MXCFB_BENCHMARK_FRAMES=60 \
rust_x11_hello --benchmark-mxcfb
```

The benchmark works only when `/dev/fb0` reports the validated readable and
writable 8-bit `hwtcon_v2` layout, the marker-specific MediaTek completion
ioctl matches the source-derived ABI, and the X11 input window matches the
framebuffer dimensions. Unsupported metadata, snapshot allocation failure,
an invalid or out-of-viewport region, ioctl rejection, geometry change, or a
non-attributable wait stops the run instead of producing a throughput result.

Snapshot restoration is attempted after normal completion, local Exit,
ordinary errors, and forwarded `HUP`, `INT`, or `TERM`. `SIGKILL`, power loss,
or a driver hang cannot run userspace cleanup. The launcher allows 15 seconds
after `TERM` before escalating to `KILL`.

## Waveform limitations

The waveform label is the request sent by PaperPad, not independently observed
panel state. In the matching 5.17.1.0.4 driver, a sufficiently large DU request
is automatically replaced with full GLR16 unless the global fast-mode flag is
set; the 1272 x 1624 viewport crosses that threshold. Fast mode bypasses the
global wait-any marker store, but this benchmark uses the marker-specific wait.
The DU result therefore cannot be described as pure DU timing without further
driver evidence.

The next useful tuning experiments are smaller dirty regions and pipelined
submissions. Pipelined submission throughput must be reported separately from
completed-update throughput and camera-observed visual FPS.

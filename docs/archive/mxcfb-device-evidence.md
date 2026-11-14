# Paperwhite 6 framebuffer evidence

## Nonexclusive evdev event trace (2026-11-13)

The deployed trace binary independently matched the host artifact at SHA-256
`86d4f053ff475b148969c2d3a96360cef95b8be94b6d849b0ec67d6d118b1608`.
The operator launched **Trace input events (15s; tap)** and made ordered
single-finger taps near top-left, center, and bottom-right. The diagnostic
opened the capability-selected `pt_mt` device without `EVIOCGRAB`.

The retrieved cumulative log SHA-256 is
`275fcd92ec4fe6ac64dd1b652f7411c49d58b02cd01567039524086928aa1fb9`.
Its second trace block, dated `Fri Nov 13 18:57:22 GMT+3:0100 2026`, captured
three complete primary contacts centered approximately at `(215,180)`,
`(639,810)`, and `(1149,1480)`. This ordering establishes that raw X increases
left-to-right and raw Y increases top-to-bottom in the tested portrait
orientation. The evdev ranges therefore map one-to-one to the 1272×1696
physical coordinate space for this orientation; no swap or inversion was
observed.

Each observed contact used the type-B multitouch sequence below:

1. `ABS_MT_TRACKING_ID=0`, followed by X/Y and sometimes
   `ABS_MT_TOUCH_MAJOR`, then `SYN_REPORT`;
2. zero or more position/touch-major changes, each committed by `SYN_REPORT`;
3. `ABS_MT_TRACKING_ID=-1`, then `SYN_REPORT` for release.

The device reused tracking ID 0 for later contacts. It emitted no `BTN_TOUCH`,
`BTN_TOOL_FINGER`, `SYN_MT_REPORT`, or `ABS_MT_SLOT` record in these
single-contact traces; slot 0 is the implicit initial slot. The capability
probe still establishes support for slots 0 and 1. No `SYN_DROPPED` or
secondary-contact sequence was exercised.

Because the trace was deliberately nonexclusive and created no X11 window, the
operator saw the original Kindle UI and the same taps invoked that UI. Neither
of two trace blocks contains the normal completion/footer line, and a later MTP
size check showed the log stable at 133,386 bytes. Do not describe this as a
successfully bounded KUAL completion: the raw contacts were captured, but the
underlying UI interaction terminated or detached the diagnostic lifecycle.
This result confirms shared delivery without a grab and shows that a future
interactive trace needs lifecycle isolation if its completion itself matters.
It does not establish `EVIOCGRAB` safety.

## Read-only evdev capability probe (2026-11-13)

The deployed ARM binary was independently read back before the run and matched
the host artifact at SHA-256
`733ba1d2808391db4e8d8a1e093c82e82711ddb6f0fb8dffb7a79add7f54eea0`.
The operator ran **Inspect input devices** through KUAL. The latest block in the
retrieved cumulative log is dated `Fri Nov 13 18:43:30 GMT+3:0100 2026`, reports
`event reads=false eviocgrab=false`, and ended with probe exit status 0. The
retrieved log SHA-256 is
`9c591b88826cfe78478c856949725e38493f9e336b30af76934382d7878a99d5`.

The probe found two readable event nodes:

- `/dev/input/event0`, `bd71828-pwrkey`, exposes `EV_SYN` and `EV_KEY` and is
  not a touch candidate.
- `/dev/input/event1`, `pt_mt`, has the direct-input property and exposes
  `EV_SYN`, `EV_KEY`, `EV_REL`, and `EV_ABS`. It exposes no `BTN_TOUCH` or
  `BTN_TOOL_FINGER`, but it has a complete multitouch capability set.

`pt_mt` reported these absolute axes; every reported fuzz, flat, and resolution
value was zero:

| Axis | Range |
| --- | --- |
| `ABS_MT_SLOT` | 0..1 |
| `ABS_MT_TOUCH_MAJOR` (`0x30`) | 0..255 |
| `ABS_MT_POSITION_X` | 0..1271 |
| `ABS_MT_POSITION_Y` | 0..1695 |
| `ABS_MT_TOOL_TYPE` (`0x37`) | 0..2 |
| `ABS_MT_TRACKING_ID` | 0..65535 |

The position ranges numerically match the validated 1272×1696 framebuffer.
Capability metadata alone does not establish axis orientation, inversion,
rotation, touch/release encoding, `SYN_REPORT` grouping, slot transition order,
secondary-contact behavior, or whether `EVIOCGRAB` is safe. No event records
were read and no exclusive grab was requested in this run.

The separately retrieved `rust_x11_hello.status` SHA-256 is
`86d5c79e6082ec9b83e0e795f4daba78ebca99a5a16087dab2cc19fec8fd6336`,
but its `STOPPED status=143 reason=watchdog` value belongs to the preceding
normal PaperPad launch. The standalone capability probe does not update that
status file, so it is not probe-result evidence.

## Backgroundless InputOutput occlusion trial (2026-11-13)

The deployed ARM binary was independently read back over MTP before the run and
matched the host artifact at SHA-256
`a4530f3a069f69d41ce32c3a824ac2d445b0bc750c41efa845b63511b2da3779`.
The preceding InputOnly build allowed the Kindle shelf to repaint sharply over
an already rendered PaperSpoon grid after the one-shot startup reassertion.

This build replaced that input window with a full-screen X11 InputOutput window
that requests no background pixel or pixmap and creates no graphics context.
PaperSpoon still owns application pixels and PaperPad still presents them only
through MXCFB; the X11 window owns stacking, clipping, touch, and Expose
notification. The operator reported the requested physical validation sequence
passed: PaperSpoon grid interaction remained usable, the delayed Kindle shelf
did not reappear over it, and local Exit completed normally.

MTP was unavailable after the run, so no post-run Kindle log or status snapshot
was retrieved. The visual result is operator evidence tied to the pre-run
readback checksum; this entry does not claim log-confirmed window class, dirty
update rectangles, or timing.

## Marker-specific completion and restoration trial (2026-10-26)

The deployed ARM binary was independently read back over MTP and matched the
host artifact at SHA-256
`01a8593f9b76e837ea13812d8615bbd4cfdf1b4e71454038a0d3a27d698c552c`.
The retrieved cumulative log has SHA-256
`4d054cf9e936542fd92e0f739fe413551e927f6b70d88d7f873c659658285c4a`;
the final status file has SHA-256
`2fead5455234aee9d2909f8525bc5dab7bc5190893b73ddc8681f97309cc15b1`
and reports `STOPPED status=0 reason=process_exit`.

All nine marker-specific launches completed all 12 measured samples, for 108
of 108 samples. No launch reported a missing or foreign marker, immediate
return, timeout-range wait, or cleanup failure. Every launch restored the
2,157,312-byte framebuffer snapshot and submitted a full-screen GC16 cleanup
refresh whose marker-specific wait took approximately 251 ms. The operator
confirmed that the original screen visibly returned after every benchmark.

The latest completed run for each requested waveform was:

| Requested waveform | Kernel-timed throughput | p50 | p95 / max |
| --- | ---: | ---: | ---: |
| A2 | 9.006 updates/s | 106.337 ms | 121.368 ms |
| DU4 | 6.836 updates/s | 144.166 ms | 153.626 ms |
| DU | 6.707 updates/s | 151.797 ms | 158.576 ms |
| GC16 | 3.718 updates/s | 271.397 ms | 275.620 ms |

Every sample exceeded the 16.667 ms software budget for 60 updates/s. These
figures time serialized full-viewport submissions and marker-specific kernel
waits; they are not camera-verified visual FPS. The waveform names describe
PaperPad's requests, not independently observed panel state. In particular,
the matching driver may substitute full GLR16 for a sufficiently large DU
request. The maintained interpretation and current compact results are in
[`docs/mxcfb-benchmarks.md`](../mxcfb-benchmarks.md).

## InputOnly marker-contention trial (2026-10-25)

The deployed ARM binary read back over MTP matched the host artifact at
SHA-256 `54571e4922115e50f575ee46fbcaf6d1c13c229974867aa92a0bca9869553107`.
The retrieved cumulative log has SHA-256
`b312a6f02063a8b51fa7a9a0b2fa7602a7913f6d59fd742d4e0c864aeee4ecf0`;
the status file has SHA-256
`a1a81a1df41da6d7e608a414e63dc894191e4ac0eae0a49532d35aaf125f7ddd`
and reports `STOPPED status=1 reason=process_exit`.

Every new run logged `class=input-only`. Operator observation found that the
earlier X11/event flicker disappeared, while the log retained core-X11
`ButtonPress`/`ButtonRelease` events and a local Exit at `(719,1667)`. The
benchmarks did not complete: each stopped after zero to two samples because
the global wait-any completion store returned foreign markers or omitted the
submitted marker across its bounded retries. This confirms global marker
contention, not a zero-FPS result. After exit, the final direct-framebuffer
image remained visible because destroying an InputOnly window creates no X11
damage that would repaint the underlying screen. The marker-specific wait and
snapshot restoration implemented in the following iteration were not yet
deployed when this evidence was recorded.

## Four-mode completion benchmark trial (2026-10-25)

The deployed benchmark binary was read back over MTP and matched the host ARM
artifact at SHA-256
`280b0c9bb2e51ae02c77ae93118023e6f6f2f2afcb73e6ff466193c3483cc3a6`.
The retrieved cumulative Kindle log is saved as
`rust_x11_hello.device.log` with SHA-256
`aa768f3b0c5ceeb6f00f94a223d95156eacc8e8b252e362cd190233a45fdddc1`;
the final status file hashes to
`a1a81a1df41da6d7e608a414e63dc894191e4ac0eae0a49532d35aaf125f7ddd`.

The operator launched all four 12-frame KUAL actions. A2, DU, DU4, and GC16
each passed the wait-any feature query and configured the expected 1272 x 1624
remote region. Only DU4 completed: 12 samples in 1,724,235 microseconds,
6.959 updates/s end to end, 139,444-microsecond p50, and
157,400-microsecond p95 and maximum. Every DU4 sample exceeded the 16,667
microsecond 60 FPS budget, so `software_60fps_budget_met=false`; no camera
evidence exists, so `visual_60fps_verified=false`.

A2, DU, and GC16 each drained an unrelated completion marker while waiting for
the local system-strip marker, then received zero markers on the first wait for
their warmup frame and exited with status 1 before recording a sample. GC16
was the final action, so the retrieved KUAL status was
`STOPPED status=1 reason=process_exit`. These are incomplete benchmarks, not
zero-FPS measurements.

Amazon's matching `Kindle_src_5.17.1.0.4_4351970007` GPL archive confirms in
`hwtcon_extra_feature.c` that wait-any blocks for at most 2,000 ms and returns
zero bytes when no markers are stored by then. It also confirms that large DU
requests are automatically changed to full GLR16 unless global fast mode is
enabled, while fast mode bypasses completion-marker storage. The benchmark was
subsequently changed to retry zero-byte waits up to a total eight-second bound,
polling local Exit between waits and continuing to require the exact submitted
marker. That retry change has host tests but no physical-device result yet.

## Read-only mapping check (2026-09-26)

The deployed probe binary was read back over MTP and matched the host artifact
at SHA-256 `d74b5b0118e23e68494939073eb658abeecff6a2f9fb68c691c58e168f96e947`.
The latest **Inspect framebuffer metadata** log block reported the geometry
below, `read-only mmap accepted length=2157312`, and probe exit status 0. The
KUAL status file reported `STOPPED status=0 reason=process_exit`. The probe
mapped the validated visible span with `PROT_READ`, then unmapped it without
reading pixel bytes. This verifies that this mapping operation was accepted
on this device; pixel access, byte polarity, framebuffer writes, and e-ink
update submission remain unverified.

## Read-only boundary-byte check (2026-09-26)

The deployed binary again matched the host artifact, this time at SHA-256
`e8a57d826c4ab0d2063d4ed0211363fef0281113866787a2450ed522a798698c`.
The latest probe log reported
`read-only mmap and boundary-byte reads accepted length=2157312` and exit
status 0. The KUAL status file reported `STOPPED status=0 reason=process_exit`.
This verifies that the first and last bytes of the mapped visible span were
readable on this device. Their values were neither logged nor interpreted;
pixel polarity, writes, and panel refresh remain unverified.

## Writable mapping check without writes (2026-09-26)

The deployed probe binary matched the host artifact at SHA-256
`ab597e85e93190c837c353102cc1eda1a70d0e18cc0699fd068941505b5532c6`.
The latest probe log reported
`writable mmap accepted without pixel writes length=2157312` and exit
status 0. The KUAL status file reported `STOPPED status=0 reason=process_exit`.
The probe opened `/dev/fb0` read-write, mapped the validated visible span with
`PROT_READ | PROT_WRITE` and `MAP_SHARED`, then unmapped it without accessing
pixel bytes through that mapping. This verifies mapping permission, not a
framebuffer write, color polarity, or e-ink update submission.

## Read-only HWTCON panel-info query (2026-09-26)

The deployed probe binary matched the host artifact at SHA-256
`a0681232b6e682bb768bdc98ce75a332a75ac6ceb82498aedac45aceb4d90be5`.
The latest probe log reported `read-only GET_PANEL_INFO_MTK accepted` and exit
status 0. The KUAL status file reported `STOPPED status=0 reason=process_exit`.
The ioctl request and 112-byte C layout came from the pinned FBInk
`hwtcon_ioctl_cmd.h` transcription attributed to PW6 FW 5.17.1.0.4. The
returned bytes were discarded. This verifies acceptance of this read-only
query on this device, not the HWTCON update-data layout, update submission,
or physical panel output.

Read-only `--inspect-framebuffer` probe run on the project's Paperwhite 6
(Sangria / Bellatrix4, FW 5.17.1.0.4) on 2026-09-26. The deployed binary was
read back over MTP and matched the host artifact's SHA-256:
`c526ca10a6e041d2e565c09b5b5bc8a7b02f7b3b1c3627caee36afd8b1e8517f`.
The probe exited with status 0 in `/extensions/rust_x11_hello/rust_x11_hello.log`.

Kernel-reported values:

| Field | Value |
| --- | --- |
| Kernel | `5.15.41-lab126` |
| `/proc/fb`, fixed ID | `hwtcon_v2` |
| Visible resolution | 1272 × 1696 |
| Virtual resolution | 1272 × 3392 |
| Offset | (0, 0) |
| Bits per pixel | 8 |
| Grayscale | 1 |
| Fixed type, visual | 0, 1 |
| Line length | 1272 bytes |
| Framebuffer memory length | 4,314,624 bytes |
| Red, green, blue fields | each offset 0, length 8, `msb_right` 0 |
| Alpha field | offset 0, length 0 |

The reported memory length equals `line_length × virtual_height`. The visible
height also equals the current 1624-pixel remote viewport plus PaperPad's
72-pixel local Exit strip. These are observations, not permission to hard-code
the resolution, stride, or format; future runs must still query them.

The Linux UAPI names fixed visual value 1 `FB_VISUAL_MONO10`, but the probe did
not write pixels, so actual byte-to-panel polarity remains physically
unverified. The `hwtcon_v2` driver identity means classic Freescale MXCFB
update structures must not be assumed. [FBInk's MediaTek Kindle header](https://github.com/NiLuJe/FBInk/blob/886f25f13368859ad8a899b88d04c26e19cda32e/eink/mtk-kindle.h)
states that it was updated from the PW6 kernel for FW 5.17.1.0.4; it is a
promising ABI reference, not a substitute for testing each operation on this
device.

## Experimental MXCFB runtime trial (2026-09-26)

The operator reported this regression sequence on the Paperwhite 6: launched
PaperPad MXCFB, saw the grid, tapped grid buttons, confirmed the resulting
actions were received in PaperSpoon, and exited with the in-window Exit button.
The deployed binary was read back over MTP and matched the host
ARM artifact at SHA-256
`0b8df0d7d1faaca41984b483f1610cd3e8617319bdbe367b68d0290a6651b20c`.
The retrieved log contains two MXCFB runs. In the latest run, the 1272 × 1696
X11 input window matched the 8-bit `hwtcon_v2` framebuffer (1272-byte stride).
The kernel accepted an Exit-strip update for `(0,1624 1272x72)` and one remote
frame update for `(0,0 1272x1624)`. PaperSpoon discovery and connection
succeeded; X11 delivered button press/release events and PaperPad queued
viewport-relative pointer events. A tap at `(719,1667)` activated the local
Exit action. The X11 window was unmapped and destroyed, the binary exited with
status 0, and the KUAL status file reported `STOPPED status=0
reason=process_exit`. The first MXCFB run also submitted Exit and remote
updates and exited normally after a local Exit tap; discovery initially retried
before connecting.

The operator report establishes the visible grid, grid-to-PaperSpoon action
flow, and usable local Exit for this run. The Kindle log independently confirms
update submission and X11 pointer queuing; PaperSpoon action receipt is based
on the operator's observation, not a retrieved PaperSpoon log. Neither source
measures exact panel pixels or update-completion timing. At that point, no
captured evidence isolated black/white
polarity, the rightmost and bottom remote pixels, replacement frames within
one run, reconnection, X11 repaint interference, or sleep/wake behavior.

## Manual diagnostic-frame validation (2026-09-26)

The operator reported manual validation on the same Paperwhite 6 and ran
PaperSpoon with `./target/release/paperspoon 5581 /tmp/paperspoon-mxcfb.log`.
The deployed binary was read back over MTP and again matched the host artifact
at SHA-256
`0b8df0d7d1faaca41984b483f1610cd3e8617319bdbe367b68d0290a6651b20c`.
The first 24 lines of the PaperSpoon log have SHA-256
`254cdb615f6234bb54565e2d8dffcb831b5ba8ca28bb85bc9a9d6caac7c2e765`;
the retrieved Kindle log has SHA-256
`0099260e7e8faa88bd57beef9721e8af7bb4ec6d2b07d171d8678bb4a3414eea`.

PaperSpoon logged three `Hello` sessions, with diagnostic frames sent in this
order: `corners`, `border`, `border`, `checkerboard`, `horizontal`, `black`,
`black`, `white`. All used the 1272 × 1624 viewport. The Kindle log records
three MXCFB runs with matching remote frame IDs 1–9 accepted for the
`(0,0 1272x1624)` region, and a separate local Exit-strip submission in each
run. The first two runs ended at the 90-second watchdog (`status=143`); the
last run logged a local Exit activation and ended with `status=0`, matching
the KUAL status file's `STOPPED status=0 reason=process_exit`.

This is evidence of repeated frame delivery, update submission, and the final
Exit path. The operator reports that the frame sequence and grid behavior
looked the same as their X11 experience. They also clarified that the
reconnects were new PaperPad sessions after watchdog endings, not an
in-process reconnect.
The subsequently appended PaperSpoon log records six grid-button semantic
actions (button IDs 4–9) with `dispatch=forwarded`. MTP was unavailable when
checked after those additional runs, so no newer Kindle log or status was
retrieved for the grid-action sequence.

The report and logs establish the tested frame sequence and grid interaction,
but do not isolate exact edge pixels or polarity. Mismatched-frame rejection,
in-process reconnect, and sleep/wake or X11 repaint behavior remain unverified.

## Final host and artifact audit (2026-09-26)

`make check`, `make build verify`, and `git diff --check origin/master...HEAD`
passed on the branch. Two consecutive ARM builds produced the same static ELF
at SHA-256 `cd4c95b83c49b159f917f7d8162bd270a70d4a6d7adba8377a783fec9b207429`.
This full-file hash differs from the device-tested binary's hash recorded
above. The only Rust-source changes since that run were comments; no
executable Rust code changed between that revision and this audit.

For a stronger artifact comparison, temporary copies of both binaries were
fully stripped with `arm-linux-gnueabihf-strip --strip-all`; both then hashed
to `157247dbbe862d1b6aee0b196749fee6bb662b5e7e2e1535d7ad4f1b4387613e`.
Their `.text` and `.rodata` sections also matched independently. The current
unstripped artifact was not redeployed or physically rerun; its full-file hash
must not be substituted for the hash recorded for the actual Kindle trial.

## Gray8 generated-frame validation (2026-09-27)

The operator reported the generated Gray8 checkpoint verified on the same
Paperwhite 6 through the experimental MXCFB backend. The deployed PaperPad
binary matched the host artifact at SHA-256
`1f4c28415d15e5267e9ddb05b0c26fb149aedab1a8d82507428c88f3227be3d9`.
This establishes that a generated Gray8 frame was visibly presented through
the negotiated Gray8 path; it does not establish that all 256 values are
individually distinguishable on the panel.

One host send reported `Broken pipe` immediately after PaperPad disconnected.
The corresponding device run had reached the 90-second watchdog and ended with
status 143. The error did not reproduce: later Gray8 sends succeeded, and a
subsequent run ended through the local Exit control with status 0. The available
evidence therefore does not identify a Gray8 encoding or presentation fault,
but it also does not rule out unrelated transport disconnects.

## Arbitrary-path JPEG validation (2026-09-28)

After PaperSpoon gained `frame jpeg <path>`, the operator reported the
checkpoint verified on the physical device. JPEG decoding, grayscale
conversion, resizing, and letterboxing occur in PaperSpoon; only the resulting
PPFB Gray8 pixels cross the network. The PaperPad binary remained unchanged at
the SHA-256 recorded above.

The report establishes successful operator acceptance of one arbitrary-path
JPEG flow on the device. The exact source path and separate observations for
EXIF orientation, aspect ratio, centering, polarity, midtone detail, cached
reconnect, and Exit-strip isolation were not reported, so those details are not
claimed as independently evidenced by this entry. Host tests cover contain
scaling, white letterboxing, malformed and missing files, retained-pixel
reconnects, and connection/viewport races.

## Phase 2 runtime extraction smoke verification (2026-11-14)

The Phase 2 PaperPad build was deployed to the Paperwhite 6 after host checks,
ARMv7 musl build, and static verification passed. Independent MTP readback
matched the host artifact at SHA-256
`34b4f8330d9f135e632ad790d30141c430376f72ff063a7270927f42f58491ff`.
The operator subsequently reported the build verified on device.

No detailed device checklist or new runtime logs accompanied that report, so
this entry records smoke acceptance only. It does not establish native evdev
input or X11-free screen ownership; this intermediate build still uses X11 for
event delivery and window lifecycle.

## Dormant evdev decoder build smoke verification (2026-11-14)

The first Phase 3 decoder build passed host checks, ARMv7 musl build, and static
verification before deployment. Independent MTP readback matched the host
artifact at SHA-256
`bc7a01667e43eb116524a5923f7b51232d126ba39e994bd59b79f8d3caffe06e`.
The operator subsequently reported the build verified on device.

The decoder was not connected to normal input in this build, so this report
establishes only that the preparatory code did not break the existing X11-driven
runtime smoke test. Native evdev event delivery, cancellation, and device-loss
behavior remain unverified on hardware.

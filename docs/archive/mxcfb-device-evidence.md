# Paperwhite 6 framebuffer evidence

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

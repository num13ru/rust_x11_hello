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

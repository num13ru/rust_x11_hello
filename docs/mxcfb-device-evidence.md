# Paperwhite 6 framebuffer evidence

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

# PaperSpoon font sources

These font files are pinned inputs for host-side PaperSpoon rendering. They are
not installed on or parsed by PaperPad.

| File | Upstream | Revision | SHA-256 |
| --- | --- | --- | --- |
| `IBMPlexMono-Medium.ttf` | <https://github.com/IBM/plex> | `bc6a7e4b37634eb79785d59781893db63ee3edcb` | `98fbd727aae340b236955879dabed4d991aac9e8e90b3b2a67ce4a59221cc97c` |
| `InterVariable.ttf` | <https://github.com/rsms/inter> | `353b61b9f4430d5f420d56605a6e7993e0941470` | `4989b125924991b90d05b2d16e0e388c48f7d5bb8b30539bbf9c755278d0ccaf` |

IBM Plex Mono is copied from
`packages/plex-mono/fonts/complete/ttf/`; Inter is copied from
`docs/font-files/InterVariable.ttf`. Both projects distribute these files
under the SIL Open Font License 1.1. The complete notices are retained as
`LICENSE-IBM-Plex.txt` and `LICENSE-Inter.txt`.

Stage 0 compared IBM Plex Mono Regular and Medium at 14, 16, 18, 20, and
22 pixels. Medium was retained because the 18-pixel, threshold-128 host sample
kept continuous stems without the lighter appearance of Regular. This is not
evidence of physical e-ink legibility.

`src/mxcfb/exit_label_bitmap.rs` is a thresholded bitmap derived from the same
pinned IBM Plex Mono Medium file for PaperPad's device-local Exit label. It is
reproducible with `cargo run --release --package paperspoon --example
bake_exit_label`; the generated Rust source and PBM preview stay under the
project-local ignored `.cache/typography-spike/` directory unless an explicit
output path is supplied.

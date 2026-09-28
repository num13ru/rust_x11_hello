# MXCFB physical-validation sequence

Target: Paperwhite 6 (Sangria / Bellatrix4), FW 5.17.1.0.4. This is a manual
panel test, not a host-test substitute. Record observations in
`mxcfb-device-evidence.md`; do not infer visible pixels from an accepted update
ioctl alone. PaperSpoon renders the diagnostic frames and owns application
actions. PaperPad must not synthesize application UI for this test.

## Before each run

1. Confirm the previous PaperPad process has ended and its window is gone.
   MTP status alone cannot establish that. Keep the 90-second KUAL watchdog.
2. Match the read-back SHA-256 of the deployed binary to the host artifact.
   Retain the prior log before clearing it for a clean evidence run.
3. Have an operator-controlled PaperSpoon process and its stdin available.
   Do not launch or restart it implicitly as part of PaperPad testing.

## A/B diagnostic frames

Run **Run Paperpad (90s)** once as the X11 reference, then **Run Paperpad MXCFB
(90s, experimental)** with the same PaperSpoon diagnostic commands, in order:

```text
frame corners 1272x1624
frame border 1272x1624
frame checkerboard 1272x1624
frame horizontal 1272x1624
frame black 1272x1624
frame white 1272x1624
```

For both backends, record what is physically visible after each command.
Check orientation and black/white polarity, the rightmost and bottom remote
pixels, and that no remote content covers the 72-pixel local Exit strip. The
`corners` blocks should occupy their corresponding corners; `border` should
reach the final remote column and row. Confirm each new frame replaces the
previous one. Check that Exit remains visible and works at the end of each run.
Diagnostic frames deliberately disable PaperSpoon application hit-testing; use
`ui 1272x1624` before testing grid actions.

## Rejection, reconnect, and interference

1. With a valid `border` frame visible, send `frame white 1272x1623`. Record
   whether the mismatched frame is rejected without replacing the last valid
   display. Then send `frame white 1272x1624` to confirm valid replacement.
   PaperSpoon's normal commands do not emit malformed PPFB messages; record
   malformed-frame handling as unverified until a separate controlled input
   test is performed.
2. In a separate run, have the operator disconnect and later restore
   PaperSpoon. Check that the local Exit remains usable while disconnected and
   that an authoritative remote frame returns after reconnect.
3. In a separate run, exercise sleep/wake or another X11 Expose while the app
   remains open. Check for direct-framebuffer loss, unexpected X11 repaint,
   cached-frame restoration, and a still-usable Exit button.

If Exit is not visible or touch does not work, do not attempt a blind tap:
allow the watchdog to end the run, then confirm the window is gone. Do not
deploy an update while a run may still be active.

## Evidence to retain

For each run, record backend, firmware, deployed SHA-256, command order,
physical observations (photos if available), and whether Exit or the watchdog
ended it. Retrieve `rust_x11_hello.log` and `rust_x11_hello.status` after the
run; retain PaperSpoon's command/action log separately if available. The
Kindle log proves update submission and X11 input delivery, not exact panel
pixels or receipt of semantic actions by PaperSpoon.

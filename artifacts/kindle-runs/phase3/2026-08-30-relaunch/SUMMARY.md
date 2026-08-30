# Kindle evidence summary: 2026-08-30 Phase 3 relaunch

## Artifact and log identity

- Host artifact: `kindle-extension/rust_x11_hello/bin/rust_x11_hello`
- Phase 3 deployed readback: `rust_x11_hello.phase3.deployed` (intentionally ignored by Git)
- SHA-256 of host artifact, Phase 0 readback, and Phase 3 readback: `873550a9518940c308e392d85811c7d5ae54f1124c9c905bbd4df3760c858d5d`
- Size of each binary: 853,596 bytes
- Full retrieved log: `rust_x11_hello.phase3.full.log`, 15,999 bytes, 177 lines
- Retrieved status: `rust_x11_hello.phase3.status`
- Committed Phase 0 log: 10,352 bytes, 116 lines
- Prefix proof: SHA-256 of the Phase 0 log and the first 10,352 bytes of the full Phase 3 log is `bd3561704a0bf81089eeea667c22649024ff9e24b347545f9d48fb42d2755fb5` in both cases.

The retrieved file therefore preserves the earlier evidence byte-for-byte and appends exactly one complete 61-line session.

## Device and session

- Model: Amazon Kindle Paperwhite (MTP-reported; serial intentionally omitted)
- Firmware: `Kindle 5.17.1.0.4 (435197 007)`
- Physical orientation: not recorded
- X11 screen: `1272 x 1696`, root `0x51`, depth `8`
- Window: `0x1c00000`, position `(80,120)`, size `760 x 360`, border `2`
- Launcher start: `Sun Aug 30 08:05:17 GMT+3:0100 2026`
- Launcher end: `Sun Aug 30 08:06:47 GMT+3:0100 2026`
- Child PID: `6413`
- Stop: watchdog `TERM`, exit status `143`

## Touch evidence

The new session contains 13 balanced primary `detail=1` press/release pairs. Their sequence matches a center tap, four inset corners, a sustained contact, a horizontal drag, and then six short taps. Two balanced auxiliary pairs also appear: `detail=9` during the sustained contact and `detail=6` during the drag.

All 30 input timestamps are nondecreasing. Every record targets the mapped app window, reports `same_screen=true`, and has root coordinates consistent with the window position and border. No panic, warning, X11 connection failure, or unexpected exit was found.

See `TEST_RECORD.md` for per-action coordinates, durations, and discrepancies. In particular, the hold lasted 4,023 ms rather than about two seconds, and six post-drag taps were logged rather than the requested five.

## Cleanup evidence

The retrieved status is:

```text
STOPPED status=143 reason=watchdog
```

The final read-only MTP inventory contains the package directory, log, status, and no `rust_x11_hello.pid`, `rust_x11_hello.watchdog`, or `rust_x11_hello.lock`.

## Assessment and operating conditions

Core-X11 touch translation after relaunch is verified on this Kindle Paperwhite, firmware build, X11 server, fixed window geometry, and exact binary checksum. It works when the KUAL launcher can access `DISPLAY=:0.0`, the installed extension remains unchanged, touches land inside the mapped window, and the 90-second watchdog is allowed to finish.

This evidence does not establish behavior on another Kindle model or firmware, in another physical orientation, outside the app window, for multitouch, or after package replacement. The full Phase 3 metadata gate remains pending because physical orientation was not explicitly recorded; the six-versus-five T5 discrepancy is preserved rather than attributed without evidence.

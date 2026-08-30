# Kindle evidence summary: 2026-08-30 geometry-aware rendering smoke

## Artifact identity

- Feature commit: `9cb0880`
- MTP updater compatibility fix: `ef5b7b8`
- Host and active-device readback SHA-256: `9c31acc52256a21fe4b2b4a65db94061f51e7a5cb9f6b72a7ae315d7e7c0140e`
- Active binary size: 853,252 bytes
- Retained `.previous` and prior Phase 3 readback SHA-256: `873550a9518940c308e392d85811c7d5ae54f1124c9c905bbd4df3760c858d5d`
- Retained prior binary size: 853,596 bytes

Both active and rollback binaries were independently downloaded after deployment and match their expected host/evidence artifacts byte-for-byte.

## Append-only log proof

- Prior Phase 3 full log: 15,999 bytes, 177 lines
- Phase 4 full log: 17,496 bytes, 212 lines
- SHA-256 of the prior log and the first 15,999 Phase 4 bytes: `07a91f28b636738c811c95daae85ca9c59ab8d3456f575bc2698a393c3aa8e40` in both cases
- New append: exactly one complete 35-line session, lines 178-212

## New session

- Start: `Sun Aug 30 08:44:05 GMT+3:0100 2026`
- End: `Sun Aug 30 08:45:35 GMT+3:0100 2026`
- Child PID: `8698`
- X11 screen: `1272 x 1696`, root `0x51`, depth `8`
- Window: `0x1c00000`, position `(80,120)`, size `760 x 360`, border `2`
- Input: two balanced `detail=1` press/release pairs at the requested center and lower-right positions
- Stop: watchdog `TERM`, exit status `143`
- Errors/warnings/panics: none

The tester reported the two nested rectangles and four complete text lines appeared without stale or clipped drawing. This verifies physical initial rendering at the unchanged default geometry. The log contains no `ConfigureNotify`; scaled redraw after a real runtime resize remains host-tested but not physically exercised.

## Deployment compatibility finding

This firmware rejects the updater's former MTP rename activation with `InvalidDevicePropValue`. The failure did not replace the active binary. Commit `ef5b7b8` changes update activation to verified readback/copy/replacement with a verified rollback attempt, and adds filesystem-backed mock success and forced-activation-failure tests. The repaired flow was used successfully on this device.

## Final state and conditions

The retrieved status is `STOPPED status=143 reason=watchdog`. MTP inventory contains the active binary and retained `.previous`, and contains no `.new`, PID, watchdog, or lock object.

This result applies to the tested Kindle Paperwhite, firmware, portrait orientation, fixed KUAL window geometry, exact active checksum, and core X11 server. It does not prove rendering after a runtime resize, landscape behavior, another Kindle/firmware, or multitouch behavior.

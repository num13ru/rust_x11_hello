# Kindle evidence summary: 2026-08-30 first canonical run

## Artifact identity

- Host artifact: `kindle-extension/rust_x11_hello/bin/rust_x11_hello`
- Deployed readback: `rust_x11_hello.phase0.deployed`
- SHA-256 (both): `873550a9518940c308e392d85811c7d5ae54f1124c9c905bbd4df3760c858d5d`
- Format: ELF 32-bit LSB ARM EABI5, statically linked
- Runtime path: `/mnt/us/extensions/rust_x11_hello/bin/rust_x11_hello`

The downloaded deployed binary matches the verified host artifact byte-for-byte by SHA-256.

## Device metadata

- Model: Amazon Kindle Paperwhite (MTP-reported; serial number intentionally omitted)
- Firmware: `Kindle 5.17.1.0.4 (435197 007)` from MTP `/system/version.txt`
- X11 screen geometry: `1272 x 1696`
- X11 orientation: portrait geometry; physical orientation was not explicitly recorded by the tester
- MTP storage: writable internal storage

## Remote cleanup state

The post-run MTP inventory contained the expected package, log, status, and binary. It did not contain:

- `rust_x11_hello.pid`
- `rust_x11_hello.watchdog`
- `rust_x11_hello.lock`

Final status:

```text
STOPPED status=143 reason=watchdog
```

## Session evidence

The retrieved log contains two complete sessions.

### Session 1

- Start: `Sun Aug 30 07:36:34 GMT+3:0100 2026`
- End: `Sun Aug 30 07:38:04 GMT+3:0100 2026`
- Child PID: `3862`
- X11 screen: `1272 x 1696`, root `0x51`, depth `8`
- Window: `0x1c00000`, position `(80,120)`, size `760 x 360`, border `2`
- Stop: watchdog `TERM`, exit status `143`
- Input: 27 `ButtonPress`/`ButtonRelease` pairs

Input validation passed with zero discrepancies:

- every press is immediately followed by one release;
- all 54 events use `detail=1`, `window=0x1c00000`, and `same_screen=true`;
- timestamps are nondecreasing;
- event coordinates vary with touch position: `event_x=147..611`, `event_y=73..324`;
- `root_x - event_x = 82` and `root_y - event_y = 122` for every event, consistent with window position plus the two-pixel border;
- no panic, X11 server error, launcher error, or warning was found.

### Session 2

- Start: `Sun Aug 30 07:38:44 GMT+3:0100 2026`
- End: `Sun Aug 30 07:40:14 GMT+3:0100 2026`
- Child PID: `3986`
- The same X11 screen and window geometry mapped successfully.
- Stop: watchdog `TERM`, exit status `143`
- Input: no press/release records

This session proves clean stop/relaunch and a second persistent mapping. It does not prove touch behavior after relaunch because no touch events were recorded in it.

## Gate assessment

- Canonical MTP installation and readback: **PASS**
- Deployed checksum identity: **PASS**
- KUAL launch and persistent 90-second lifetime: **PASS**
- Watchdog termination and PID/watchdog/lock cleanup: **PASS**
- Stop/relaunch and second mapping: **PASS**
- Phase 0 recoverable baseline: **PASS**
- Phase 1 persistent event loop: **PASS**
- Phase 2 core-pointer subscription/no-motion-flood: **PASS**
- Core X11 touchscreen translation on the first session: **VERIFIED**
- Full annotated Phase 3 input matrix: **PENDING**
- Touch behavior after relaunch: **PENDING**

The model and firmware have now been recovered through read-only MTP metadata. The physical orientation remains inferred from X11 geometry, and the second session contains no touch records, so the complete Phase 3 milestone is not yet finished.

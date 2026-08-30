# Phase 6 logical hit-testing record

## Fixed metadata

- Device: Amazon Kindle Paperwhite (serial omitted)
- Firmware: `Kindle 5.17.1.0.4 (435197 007)`
- Physical orientation: portrait
- Feature commit: `613a0b4` (`feat: add release-based touch hit testing`)
- Active binary SHA-256: `e34c6ad03e21ba06e1fc4929bf5a5b35994e9fc8f2f37217a72862cde68ef258`
- Retained geometry-build SHA-256: `9c31acc52256a21fe4b2b4a65db94061f51e7a5cb9f6b72a7ae315d7e7c0140e`
- X11 screen: `1272 x 1696`
- App window: `(80,120)`, `760 x 360`, border `2`
- Grid at default geometry: columns `[20,260)`, `[260,500)`, `[500,740)`; rows `[60,200)`, `[200,340)`

## Deployment state

- Active and `.previous` device objects were independently downloaded after update and match the expected checksums.
- The pre-geometry `.previous` object was removed only after its Phase 4 local readback was verified; it remains recoverable locally.
- The rollback slot now contains the physically tested geometry build.
- No `.new` staging object remains.

## Physical action matrix

Do not redeploy or launch another session before evidence retrieval.

1. Disconnect USB if MTP mode blocks KUAL.
2. Select **Run Rust X11 Hello (90s)**.
3. Confirm the title and six outlined buttons appear as:

   ```text
   1  2  3
   4  5  6
   ```

4. T1: tap buttons `1`, `2`, `3`, `4`, `5`, `6` once each, in that order, with a short pause between taps.
5. T2: tap the title area above the grid once. It is inside the window but outside every logical button and must not activate a button.
6. T3: press inside button `1`, drag into button `2`, and release there. It must not activate either button.
7. T4: press and hold inside button `4` for about two seconds, then release inside button `4`. It must activate button `4` exactly once even if the Kindle emits an auxiliary button pair during the hold.
8. Allow the watchdog to stop the process; confirm the window disappears and Kindle UI returns.
9. Reconnect MTP and report completion without another run.

Expected logical activation sequence:

```text
1, 2, 3, 4, 5, 6, 4
```

## Results

The tester replied `done` after the requested sequence and reported no grid/title rendering problem. Action attribution uses the prescribed order; the distinctive grid coordinates and exact logical sequence make the mapping unambiguous.

- Session: full-log lines 213-272, `Sun Aug 30 09:03:48` through `09:05:18 GMT+3:0100 2026`, child PID `10306`.
- Grid/title rendering: **PASS (TESTER-REPORTED)**.
- T1 six ordered activations: **PASS** — primary pairs 1-6 occupy buttons 1-6 respectively and produce exactly `1,2,3,4,5,6`.
- T2 outside-grid cancellation: **PASS** — primary pair 7 is at title coordinates `(123,27)` to `(121,26)` and produces no activation.
- T3 cross-button release cancellation: **PASS** — primary pair 8 starts in button 1 at `(151,123)` and releases in button 2 at `(397,136)`, producing no activation. A balanced auxiliary `detail=6` pair occurs during the drag and does not activate or corrupt the primary contact.
- T4 held primary contact/auxiliary tolerance: **PASS** — primary pair 9 remains in button 4 from `(161,264)` to `(158,279)` for 4,322 ms and produces exactly one button-4 activation. A balanced auxiliary `detail=9` pair occurs during the hold without cancelling the armed primary contact.
- Complete activation sequence: **PASS** — seven lines, exactly `1,2,3,4,5,6,4`.
- Raw pointer diagnostics preserved: **PASS** — 22 input lines: 11 presses and 11 releases; nine balanced primary pairs plus balanced `detail=6` and `detail=9` pairs.
- Event invariants: **PASS** — timestamps are nondecreasing; every record targets window `0x1c00000`, reports `same_screen=true`, and preserves root offset `(82,122)`.
- Errors/warnings/panics: **NONE**.
- Watchdog/UI recovery and cleanup: **PASS** — status is `STOPPED status=143 reason=watchdog`; MTP inventory contains no PID, watchdog, lock, or `.new` object.
- Complete post-run log/status retrieval: **PASS** — the Phase 4 log is a byte-identical prefix and exactly one complete 60-line session was appended.

## Gate assessment

- Six logical buttons position-dependent: **VERIFIED ON PHYSICAL KINDLE**
- Release-based same-button activation: **VERIFIED**
- Outside-grid and cross-button cancellation: **VERIFIED**
- Auxiliary Kindle button events ignored without losing primary state: **VERIFIED**
- No external/network side effect: **VERIFIED BY IMPLEMENTATION SCOPE**
- Phase 6 logical hit-test proof: **PASS**

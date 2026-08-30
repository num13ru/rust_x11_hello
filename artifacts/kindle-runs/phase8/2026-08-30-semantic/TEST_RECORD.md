# Phase 8 semantic action-id record

## Fixed metadata

- Device: Amazon Kindle Paperwhite (serial omitted)
- Firmware: `Kindle 5.17.1.0.4 (435197 007)`
- Physical orientation: portrait
- Feature commit: `5f72988` (`feat: add semantic action ids to activations`)
- Active binary SHA-256: `db717400cb7606ea5225e539f41520aee5988bb7c6dc0e7e581c6e22368b8589`
- Retained phase 7 rollback SHA-256: `65ec6913ef8139892a4316836acdafea278b82a0b52ef02e7d95425788aa1e13`
- X11 screen: `1272 x 1696`
- App window: `(80,120)`, `760 x 360`, border `2`
- Grid at default geometry: columns `[20,260)`, `[260,500)`, `[500,740)`; rows `[60,200)`, `[200,340)`

## Deployment state

- The phase 7 retained `.previous` object was downloaded to the host and removed from the device before the update, per the deploy script's backup-slot rule; the phase 7 binary was then retained as the new `.previous` after activation.
- The phase 6 (pre-phase-7) binary is recoverable at the host temp path used during the update, matching SHA-256 `e34c6ad03e21ba06e1fc4929bf5a5b35994e9fc8f2f37217a72862cde68ef258`.
- Active and `.previous` device objects were independently downloaded after update and match the expected checksums.
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
9. Reconnect MTP and report completion without another run.Expected logical activation sequence:

```text
1, 2, 3, 4, 5, 6, 4
```

## Results

The tester replied `done` after the requested sequence. Action attribution uses the prescribed order; the distinctive grid coordinates and exact logical sequence make the mapping unambiguous.

- Session: full-log lines 333-392, `Sun Aug 30 09:42:16` through `09:43:46 GMT+3:0100 2026`, child PID `12548`.
- Grid/title rendering: **PASS (TESTER-REPORTED)** — the tester ran the sequence without reporting a rendering problem.
- T1 six ordered activations: **PASS** — primary pairs 1-6 occupy buttons 1-6 respectively and produce exactly `1,2,3,4,5,6`.
- T2 outside-grid cancellation: **PASS** — primary pair 7 is at title coordinates `(136,15)` to `(126,31)` and produces no activation.
- T3 cross-button release cancellation: **PASS** — primary pair 8 starts in button 1 at `(91,137)` and releases in button 2 at `(417,159)`, producing no activation. A balanced auxiliary `detail=6` pair occurs during the drag and does not activate or corrupt the primary contact.
- T4 held primary contact/auxiliary tolerance: **PASS** — primary pair 9 remains in button 4 from `(135,271)` to `(135,286)` for 2,320 ms and produces exactly one button-4 activation. A balanced auxiliary `detail=9` pair occurs during the hold without cancelling the armed primary contact.
- Semantic id emission: **PASS** — every activation line carries exactly one `semantic=` value matching the documented mapping: `media.play_pause`, `media.next`, `media.previous`, `terminal.new_window`, `tmux.work`, `zoom.toggle_mute`.
- Complete activation sequence: **PASS** — seven lines, exactly `1,2,3,4,5,6,4`.
- Raw pointer diagnostics preserved: **PASS** — 22 input lines: 11 presses and 11 releases; nine balanced primary pairs plus balanced `detail=6` and `detail=9` pairs.
- Event invariants: **PASS** — timestamps are nondecreasing; every record targets window `0x1c00000`, reports `same_screen=true`, and preserves root offset `(82,122)`.
- Errors/warnings/panics: **NONE**.
- Watchdog/UI recovery and cleanup: **PASS** — status is `STOPPED status=143 reason=watchdog`; MTP inventory contains no PID, watchdog, lock, or `.new` object.
- Complete post-run log/status retrieval: **PASS** — the Phase 7 log is a byte-identical prefix and exactly one complete 60-line session was appended.

## Gate assessment

- Semantic action ids render and attach to every activation: **VERIFIED ON PHYSICAL KINDLE**
- Regression: render, six-button activation, outside/cross-button cancellation, auxiliary tolerance: **VERIFIED**
- Raw diagnostic format and event invariants unchanged: **VERIFIED**
- No transport, no external/network side effect: **VERIFIED BY IMPLEMENTATION SCOPE**
- Phase 8 semantic action-id proof: **PASS**
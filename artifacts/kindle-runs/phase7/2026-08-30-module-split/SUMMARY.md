# Kindle evidence summary: 2026-08-30 Phase 7 module split

## Artifact identity

- Refactor commit: `64b2cec` (`refactor: split X11 and UI into modules`)
- Host and active-device readback SHA-256: `65ec6913ef8139892a4316836acdafea278b82a0b52ef02e7d95425788aa1e13`
- Active binary size: 859,292 bytes
- Retained phase 6 rollback SHA-256: `e34c6ad03e21ba06e1fc4929bf5a5b35994e9fc8f2f37217a72862cde68ef258`
- Retained rollback size: 854,152 bytes

Independent post-deployment readback matches the expected checksum byte-for-byte. No `.new` staging object remains.

## Append-only log proof

- Prior Phase 6 full log: 22,069 bytes, 272 lines
- Phase 7 full log: 26,642 bytes, 332 lines
- SHA-256 of the prior log and the first 22,069 Phase 7 bytes: `c2385f011e964be36eb94963d92aaf4019999ee2106f1189b73858b20153a39d` in both cases
- New append: exactly one complete 60-line session, lines 273-332

## Physical result

The tester reported `done` after the phase 6 sequence. The new session contains:

- six normal taps at coordinates inside buttons 1-6, producing `ui action=activate` IDs `1,2,3,4,5,6`;
- one title-area tap outside the grid, producing no activation;
- one primary contact from button 1 to button 2, producing no activation despite a nested balanced `detail=6` pair;
- one 2,689 ms primary hold inside button 4, producing exactly one button-4 activation despite a nested balanced `detail=9` pair.

The complete logical activation sequence is exactly `1,2,3,4,5,6,4`.

## Input and lifecycle invariants

- 22 raw input records: 11 presses and 11 releases
- Nine balanced `detail=1` primary pairs
- One balanced `detail=6` pair and one balanced `detail=9` pair
- Nondecreasing X11 timestamps
- Window `0x1c00000`, `same_screen=true`, and root offset `(82,122)` for every input record
- No panic, warning, connection failure, or `ConfigureNotify`
- Watchdog `TERM`, exit status `143`
- No PID, watchdog, lock, or `.new` object after the run

## Conclusion and conditions

Phase 7 module split is verified on the physical Kindle for the exact active binary, Kindle Paperwhite firmware, portrait orientation, fixed `760 x 360` override-redirect window, and core-X11 `detail=1` primary touch translation.

The refactor is behavior-identical to phase 6: same window, same geometry, same raw diagnostics, same release-based activation and cancellation semantics. It does not prove landscape/scaled-grid behavior, simultaneous multitouch, another Kindle/firmware, or any network action; no network action exists in this milestone.

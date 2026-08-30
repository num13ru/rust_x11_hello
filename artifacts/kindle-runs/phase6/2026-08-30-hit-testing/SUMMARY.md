# Kindle evidence summary: 2026-08-30 Phase 6 logical hit testing

## Artifact identity

- Feature commit: `613a0b4`
- Host and active-device readback SHA-256: `e34c6ad03e21ba06e1fc4929bf5a5b35994e9fc8f2f37217a72862cde68ef258`
- Active binary size: 854,152 bytes
- Retained geometry-build rollback SHA-256: `9c31acc52256a21fe4b2b4a65db94061f51e7a5cb9f6b72a7ae315d7e7c0140e`
- Retained rollback size: 853,252 bytes

Independent post-deployment readbacks match both expected checksums byte-for-byte. No `.new` staging object remains.

## Append-only log proof

- Prior Phase 4 full log: 17,496 bytes, 212 lines
- Phase 6 full log: 22,069 bytes, 272 lines
- SHA-256 of the prior log and the first 17,496 Phase 6 bytes: `8b06ca5423a96d2fe70f20335dc7c50b52f606f9a3cff10c7010b98507f803dd` in both cases
- New append: exactly one complete 60-line session, lines 213-272

## Physical result

The tester reported the title and six-button 2×3 grid rendered without a visual problem. The new session contains:

- six normal taps at coordinates inside buttons 1-6, producing `ui action=activate` IDs `1,2,3,4,5,6`;
- one title-area tap outside the grid, producing no activation;
- one primary contact from button 1 to button 2, producing no activation despite a nested balanced `detail=6` pair;
- one 4,322 ms primary hold inside button 4, producing exactly one button-4 activation despite a nested balanced `detail=9` pair.

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

Phase 6 logical hit testing is verified on the physical Kindle for the exact active binary, Kindle Paperwhite firmware, portrait orientation, fixed `760 x 360` override-redirect window, and core-X11 `detail=1` primary touch translation.

This proves release-based same-button activation, outside/cross-button cancellation, and tolerance of the observed auxiliary details. It does not prove landscape/scaled-grid behavior, simultaneous multitouch, another Kindle/firmware, or any network action; no network action exists in this milestone.

# Kindle evidence summary: 2026-08-30 Phase 8 semantic action ids

## Artifact identity

- Feature commit: `5f72988` (`feat: add semantic action ids to activations`)
- Host and active-device readback SHA-256: `db717400cb7606ea5225e539f41520aee5988bb7c6dc0e7e581c6e22368b8589`
- Active binary size: 860,212 bytes
- Retained phase 7 rollback SHA-256: `65ec6913ef8139892a4316836acdafea278b82a0b52ef02e7d95425788aa1e13`
- Retained rollback size: 859,292 bytes

Independent post-deployment readback matches the expected checksum byte-for-byte. No `.new` staging object remains.

## Append-only log proof

- Prior Phase 7 full log: 26,642 bytes, 332 lines
- Phase 8 full log: 31,387 bytes, 392 lines
- SHA-256 of the prior log and the first 26,642 Phase 8 bytes: `26bc224169859e9ea2a5cf09c887c395ae534e19a3d97d8a11e03dc78fb99647` in both cases
- New append: exactly one complete 60-line session, lines 333-392

## Physical result

The tester reported `done` after the phase 6 sequence. The new session contains:

- six normal taps at coordinates inside buttons 1-6, producing `ui action=activate` IDs `1,2,3,4,5,6`;
- one title-area tap outside the grid, producing no activation;
- one primary contact from button 1 to button 2, producing no activation despite a nested balanced `detail=6` pair;
- one 2,320 ms primary hold inside button 4, producing exactly one button-4 activation despite a nested balanced `detail=9` pair.

The complete logical activation sequence is exactly `1,2,3,4,5,6,4`, and every activation now carries its stable semantic action id:

| Button | Semantic id observed |
| ------ | -------------------- |
| 1 | `media.play_pause` |
| 2 | `media.next` |
| 3 | `media.previous` |
| 4 | `terminal.new_window` |
| 5 | `tmux.work` |
| 6 | `zoom.toggle_mute` |

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

Phase 8 semantic action ids are verified on the physical Kindle for the exact active binary, Kindle Paperwhite firmware, portrait orientation, fixed `760 x 360` override-redirect window, and core-X11 `detail=1` primary touch translation.

The semantic layer is behavior-identical to phase 7 for all input semantics: same window, same geometry, same raw diagnostics, same release-based activation and cancellation. The new `semantic=` field appears exactly once per activation with the correct documented id. It does not prove landscape/scaled-grid behavior, simultaneous multitouch, another Kindle/firmware, any transport, or any network action; no transport or network action exists in this milestone.
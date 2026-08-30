# Phase 3 relaunch-touch record

## Fixed metadata

- Device: Amazon Kindle Paperwhite (serial omitted)
- Firmware: `Kindle 5.17.1.0.4 (435197 007)`
- Physical orientation: **PORTRAIT** (tester-confirmed after the run)
- X11 geometry: `1272 x 1696`
- App window: `(80,120)`, `760 x 360`, border `2`
- Binary SHA-256: `873550a9518940c308e392d85811c7d5ae54f1124c9c905bbd4df3760c858d5d`
- MTP extension: `/extensions/rust_x11_hello`
- Runtime extension: `/mnt/us/extensions/rust_x11_hello`

## Requested physical actions

The tester was asked to use the already verified installed binary, without redeploying it, and perform:

1. T1: one center tap.
2. T2: one inset tap near each visible corner, in top-left, top-right, bottom-left, bottom-right order.
3. T3: press and hold near center for about two seconds, then release.
4. T4: press near center, drag a short distance, then release.
5. T5: five moderately paced center taps.
6. Allow the watchdog to stop the process, reconnect MTP, and do not start another run before retrieval.

The tester reported the sequence complete. The action-to-event mapping below is inferred from the requested order and the strongly distinctive coordinates/timing; no independent per-action timestamps were recorded.

## Retrieved results

- Run header: `Sun Aug 30 08:05:17 GMT+3:0100 2026`, child PID `6413`
- Tested session: full-log lines 117-177; input lines 141-170
- Mapped window: `0x1c00000`
- T1 center tap: **PASS** — primary pair 1, `(383,177)` to `(379,177)`, 186 ms.
- T2 four inset corners: **PASS** — primary pairs 2-5:
  - top-left: `(11,20)` to `(11,21)`, 261 ms;
  - top-right: `(748,18)` to `(741,14)`, 273 ms;
  - bottom-left: `(31,336)` to `(28,338)`, 273 ms;
  - bottom-right: `(743,323)` to `(728,328)`, 286 ms.
- T3 hold/release: **PASS WITH TIMING DISCREPANCY** — primary pair 6 lasts 4,023 ms, from `(349,145)` to `(370,175)`. It is longer than the requested approximately two seconds. A balanced auxiliary `detail=9` press/release pair appears 500 ms after the primary press; there is no repeated primary press.
- T4 drag/release: **PASS** — primary pair 7 lasts 1,416 ms and moves from `(339,172)` to `(581,179)`. A balanced auxiliary `detail=6` press/release pair appears at `(447,175)` during the contact; the log does not flood.
- T5 five taps: **RECORDED DISCREPANCY** — primary pairs 8-13 show six ordered, balanced taps rather than five. Durations are 135, 149, 161, 125, 161, and 99 ms. The evidence cannot determine whether the extra pair was an accidental tester contact or device behavior.
- Watchdog/UI recovery: **PASS** — tester reported completion after the requested watchdog/reconnect sequence; retrieved status is `STOPPED status=143 reason=watchdog`.
- Post-run retrieval validation: **PASS** — no subsequent session is present, and the final MTP inventory contains no PID, watchdog, or lock file.

## Event invariants

- 30 input records: 15 presses and 15 releases.
- Primary `detail=1`: 13 ordered, balanced press/release pairs.
- Auxiliary events: one balanced `detail=6` pair and one balanced `detail=9` pair.
- Timestamps are nondecreasing; the two auxiliary pairs each use one timestamp for their press and release.
- Every event targets window `0x1c00000`, has `same_screen=true`, and preserves the fixed root offset: `root_x = event_x + 82`, `root_y = event_y + 122`.
- No panic, warning, connection failure, or unexpected exit is present in the retrieved log.

## Gate assessment

- Touch events after a stop/relaunch cycle: **VERIFIED**
- Position-dependent core-X11 `ButtonPress`/`ButtonRelease`: **VERIFIED**
- Watchdog termination and transient-file cleanup: **PASS**
- Exact requested T5 cardinality: **DISCREPANCY RECORDED**
- Complete device metadata: **PASS**
- Overall Phase 3 milestone: **VERIFIED ON PHYSICAL KINDLE**, with the exact T5 cardinality discrepancy retained above.

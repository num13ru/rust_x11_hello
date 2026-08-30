# Geometry-aware rendering physical smoke record

## Fixed metadata

- Device: Amazon Kindle Paperwhite (serial omitted)
- Firmware: `Kindle 5.17.1.0.4 (435197 007)`
- Physical orientation: portrait
- Feature commit: `9cb0880` (`feat: redraw X11 layout on geometry changes`)
- MTP updater fix commit: `ef5b7b8` (`fix: update Kindle binary without MTP rename`)
- Active binary SHA-256: `9c31acc52256a21fe4b2b4a65db94061f51e7a5cb9f6b72a7ae315d7e7c0140e`
- Retained prior binary SHA-256: `873550a9518940c308e392d85811c7d5ae54f1124c9c905bbd4df3760c858d5d`
- X11 screen: `1272 x 1696`
- App window: `(80,120)`, `760 x 360`, border `2`

## Deployment record

The first guarded update verified the staged binary and support files, then the Kindle rejected the first MTP rename with `InvalidDevicePropValue`. The old binary remained active and the verified new binary remained as `.new`; the application was not launched in that mixed state.

The updater was changed to avoid MTP rename operations. It now:

1. verifies a staged `.new` upload;
2. downloads the active binary to a guarded host temporary directory;
3. uploads and verifies that prior binary as `rust_x11_hello.previous`;
4. activates the new binary with `put --replace --verify`;
5. attempts a verified rollback from the downloaded prior binary if activation fails;
6. removes `.new` with explicit `--yes` after successful activation.

Independent post-update readbacks match both expected checksums. Final device inventory contains the active binary and `.previous`, with no `.new`.

## Requested physical actions

1. Launch **Run Rust X11 Hello (90s)** through KUAL.
2. Confirm two nested rectangles and four complete text lines appear without stale or clipped drawing.
3. Tap once near the center and once near the lower-right inside the window.
4. Allow the watchdog to stop the process and confirm the Kindle UI returns.
5. Reconnect MTP without another launch.

The tester replied `done` and reported no visual problem. Visual conclusions below are therefore tester-reported; no screenshot was captured.

## Retrieved results

- Visual default-layout smoke: **PASS (TESTER-REPORTED)** — both rectangles and all four text lines appeared without a reported clipping or stale-pixel defect.
- Center tap: **PASS** — `(362,173)` to `(361,175)`, `detail=1`, 259 ms.
- Lower-right tap: **PASS** — `(731,353)` to `(731,353)`, `detail=1`, 336 ms.
- Event invariants: **PASS** — two ordered, balanced primary pairs; nondecreasing timestamps; window `0x1c00000`; `same_screen=true`; root offset `(82,122)` for every event.
- Watchdog/UI recovery: **PASS** — tester reported completion and retrieved status is `STOPPED status=143 reason=watchdog`.
- Cleanup: **PASS** — final MTP inventory contains no PID, watchdog, lock, or `.new` object.
- Runtime resize/`ConfigureNotify` redraw: **NOT EXERCISED** — the session retained `760 x 360` and contains no `ConfigureNotify` record.

## Assessment

- Physical initial rendering at the verified default geometry: **PASS**
- Touch logging regression check: **PASS**
- Watchdog and transient-file cleanup regression check: **PASS**
- Geometry calculation and update-decision unit tests: **PASS**
- Physical runtime resize behavior: **UNVERIFIED** because this fixed override-redirect KUAL run provides no resize action.

The physical smoke establishes that the geometry-aware implementation preserves the verified default Kindle appearance and input lifecycle. It does not establish scaled rendering after an actual X11 resize.

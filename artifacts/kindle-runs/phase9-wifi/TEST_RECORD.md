# Phase 9 Wi-Fi semantic transport evidence

## Fixed metadata

- Device: Amazon Kindle Paperwhite 6 (Sangria / Bellatrix4), serial `GN433X11518401E8`
- Firmware: `Kindle 5.17.1.0.4 (435197 007)`
- Network: Wi-Fi, RT-5GPON-9AC3 LAN
  - Kindle: `192.168.0.4` (wlan0, ARP-verified `54:2b:1c:fd:2b:6`)
  - Mac: `192.168.0.12` (en0)
- Active binary SHA-256 (host artifact): `7e1d12aa673e2c7552e458a118de3ba062d2d1cce86769127722ed68fbd535b1`
- Commit: `c6d1f47` (`fix: resolve companion host inside run.sh`)

## Companion

- Listener: Rust `tools/companion` (std-only), `./target/release/companion 5581 /tmp/companion_wifi_run.log`
- Evidence: `artifacts/kindle-runs/phase9-wifi/companion.wifi.log`

## Proven session (PID 15480)

- Launcher: KUAL → **Run Rust X11 Hello (WiFi)**, `Companion host: 192.168.0.12`
- Window: `(80,120)` `760 x 360`, border 2, X11 `1272 x 1696`, root `0x51`
- Run window: `Sun Aug 30 11:58:51` → `12:00:21` (watchdog TERM, status 143)

### Activation ↔ transport correlation

| # | Button | Device log (`ui action=activate`) | Companion line (from `192.168.0.4`) |
|---|--------|-----------------------------------|--------------------------------------|
| 1 | 1 | `button=1 semantic=media.play_pause` | `1788080159 … event action=media.play_pause;` |
| 2 | 2 | `button=2 semantic=media.next` | `1788080160 … event action=media.next;` |
| 3 | 3 | `button=3 semantic=media.previous` | `1788080160 … event action=media.previous;` |
| 4 | 4 | `button=4 semantic=terminal.new_window` | `1788080160 … event action=terminal.new_window;` |
| 5 | 5 | `button=5 semantic=tmux.work` | `1788080161 … event action=tmux.work;` |
| 6 | 6 | `button=6 semantic=zoom.toggle_mute` | `1788080161 … event action=zoom.toggle_mute;` |

- Transport errors in session: **none**.
- Companion validation: `OK semantic action:` for all six.
- Physical pointer diagnostics: 12 `input` lines (6 balanced primary pairs), no auxiliary pairs in this session.
- Event invariants: all records target window `0x1c00000`, `same_screen=true`, root offset `(82,122)`.
- Clean end: status `STOPPED status=143 reason=watchdog`; no PID/watchdog/lock objects remain.

## Failed-connectivity contrast session (PID 15247)

Same target, but no listener was bound on the Mac (Rust companion not yet started; port free):
`transport error: failed to connect to companion: connection timed out` after buttons
1–4, then button 5/6 sent without error once the listener came up. Proves the
bounded (150 ms) failure path does not break the X11 event loop or activation
logging; the on-device activation sequence was logged regardless.

## Transport conclusion

- Kindle → Mac over Wi-Fi, one `event action=<id>;` line per activation, exact 1:1 with
  the device-log activations: **VERIFIED ON PHYSICAL KINDLE**
- Companion down costs bounded time and never breaks the loop: **VERIFIED** (PID 15247)
- No listening socket on Kindle; only the action id leaves the device: **VERIFIED BY SOURCE**
- Phase 9 Wi-Fi transport proof: **PASS**
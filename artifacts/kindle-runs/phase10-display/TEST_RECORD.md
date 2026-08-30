# Phase 10 persistent-companion and display-command evidence

## Fixed metadata

- Device: Amazon Kindle Paperwhite 6 (Sangria / Bellatrix4), serial `GN433X11518401E8`
- Firmware: `Kindle 5.17.1.0.4 (435197 007)`
- Network: Wi-Fi, RT-5GPON-9AC3 LAN (Kindle `192.168.0.4`, Mac `192.168.0.12`)
- Active binary SHA-256 (host artifact): `019d3cdfcb313229c3bb5133055ab4d1a282f79d7966a14c50aeb961a95a27ff`
- Commit: `ccb5c4e` (`feat: persistent companion session with display commands`)
- Companion: Rust `tools/companion`, `./target/release/companion 5581 /tmp/companion.log`

## Persistent transport — proven (session PID 17488)

- Launcher: KUAL → **Run Rust X11 Hello (WiFi)**, `Companion host: 192.168.0.12`
- `transport: connected to companion` at startup — single persistent connection
- Companion log: all six actions from **one peer port** (`192.168.0.4:37242`),
  proving one connection reused for the run (not per-activation connects):

  ```
  1788081361 192.168.0.4:37242 event action=media.play_pause;
  1788081362 192.168.0.4:37242 event action=media.next;
  1788081362 192.168.0.4:37242 event action=media.previous;
  1788081364 192.168.0.4:37242 event action=terminal.new_window;
  1788081364 192.168.0.4:37242 event action=tmux.work;
  1788081366 192.168.0.4:37242 event action=zoom.toggle_mute;
  ```

## Display command (Mac -> Kindle) — pending device confirmation

The stdin-forwarding path is implemented and locally smoke-tested (companion
forwarded `display hello from host` to a fake reader), but no `display:` line
appeared in the PID 17488 device log because no display command was typed
while a Kindle connection was live:

1. First companion instance was killed by the harness watchdog after the
   session; its six received actions are the transport evidence above.
2. Second companion (PID 65093) never saw a connection: it was started after
   the Kindle run had already watchdog-ended, so `display hello world` typed
   at its stdin had no socket to forward over.

### Repeat for confirmation

With a companion up and the Kindle run live (within the 90s watchdog):

```text
display hello world
```

Expected: device log `display: hello world`, window status area shows the text.

## Transport conclusion

- Persistent connection, one per run, reused for all activations: **VERIFIED ON PHYSICAL KINDLE**
- All six semantic ids delivered over it, zero errors: **VERIFIED**
- Companion down at startup is non-fatal (bounded reconnect on next activation): **VERIFIED BY DESIGN + FAILURE-PATH OBSERVATION**
- Display command render: **PENDING DEVICE CONFIRMATION** (code + local smoke test done)
- Phase 10 persistent transport: **PASS**; display direction: **PENDING**
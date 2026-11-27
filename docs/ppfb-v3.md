# PPFB v3 wire format

This specification covers independent frame transport. PPFB version 2
retains its existing framing and raw framebuffer payload.

## Opt-in validation

Run the updated PaperSpoon with compression enabled and URL actions disabled
for a safe local test. These settings use level 1 and the fake media provider.

```sh
cargo run --release -p paperspoon -- 5581 /tmp/paperspoon-v3.log \
    --compression=auto --now-playing-fake --no-forward-url
```

For PaperView, open a second terminal and run:

```sh
cargo run -p paperview -- --transport v3-auto --transport-diagnostics --interactive
```

For PaperPad, deploy the verified ARMv7 build, unplug USB, open KUAL, and tap
**PaperPad → Run Paperpad DU with PPFB v3 (5m)**. The connection indicator should
fill and the application grid should appear. Enter `nowplaying start` in
PaperSpoon, try play/pause and seeking, then tap **Exit** on the Kindle. The
launcher records transport events and the final cleanup status locally.

PaperSpoon defaults to `--compression=raw`. `auto` allows a legacy v2 or raw
v3 session and prefers Zstandard when negotiated. `zstd` requires a peer
that offers Zstandard. All v3 sessions retain per-frame raw fallback.
`--compression-level=1|3|6` selects the host encoder setting.

PaperView's `--transport` and PaperPad's `PAPERPAD_TRANSPORT` take `v2`,
`v3-raw`, `v3-auto`, or `v3-zstd`. Both default to `v2`. Select `v3-raw`
for a raw v3 diagnostic session and `v3-zstd` to require the host's Zstandard
capability. The KUAL v3 entry selects `v3-auto`. Use the regular **Run
Paperpad DU (5m)** KUAL entry for a legacy raw comparison. A v3 acknowledgment
error from a v2-only server requires an explicit switch to `v2`.

The [validation report](benchmarks/ppfb-v3-validation.md) tracks desktop and
device evidence separately.

## Connection setup

The client chooses v2 legacy mode or opts into v3 and sends Hello first.
A dual-version host selects the session version from that first header.
Version 2 sends no acknowledgment. Version 3 requires a HelloAck before any
frame or control message. The acknowledgment confirms a subset of the
client's advertised encodings. Raw is required. Zstandard is optional. The
intersection is valid for one TCP connection and is negotiated again on
reconnect. A strict Zstandard request requires Zstandard in the acknowledgment.

| Host | Client selection | Result |
|---|---|---|
| v2 | v2 legacy | Raw v2 |
| v2 + v3 | v2 legacy | Raw v2 |
| v2 | v3 | Unsupported version. Select v2 legacy and reconnect. |
| v2 + v3 | v3 | Raw v3 with optional Zstandard |

An endpoint closes the connection on unsupported versions, invalid handshake
or frame metadata, or decompression failure. A new connection starts with
Hello. Retain the previous validated framebuffer until a new frame succeeds.
When a viewport changes while an older v3 frame is in flight, the receiver can
close that session and renegotiate the new viewport on reconnect.

## Message header

All PPFB integers use network byte order. Each v3 message starts with the
12-byte header `PPFB`, `u8` version `3`, `u8` type, `u16` zero flags, and
`u32` payload byte length. The maximum payload is 16 MiB.

| Type | ID | Direction |
|---|---:|---|
| Hello | 1 | Client to host |
| PointerDown | 2 | Client to host |
| PointerUp | 3 | Client to host |
| ViewportChanged | 4 | Client to host |
| Frame | 5 | Host to client |
| HelloAck | 6 | Host to client |

Hello contains `u8` version `3`, `u8` pixel format mask, `u8` encoding mask,
`u8` reserved zero, `u16` width, and `u16` height. The pixel format mask
uses bit 0 for Mono1 and bit 1 for Gray8. HelloAck contains a `u8` encoding
mask followed by three reserved zero bytes. In both masks bit 0 is Raw and
bit 1 is Zstandard. The host ignores unknown offered encoding bits. The
client rejects unknown or unadvertised acknowledged bits. A zero viewport
dimension indicates the client has no remote viewport. Pointer and viewport
payloads use the existing v2 layout under the selected header version.

## Frame payload

Each Frame starts with `u64` frame ID, `u16` width, `u16` height, `u8` pixel
format (`1` Mono1, `2` Gray8), `u8` encoding (`0` Raw, `1` Zstandard), and
`u16` reserved zero. Raw pixel bytes follow immediately. Zstandard adds a
`u32` decoded byte count followed by exactly one Zstandard frame. The outer
payload length specifies the encoded length after this prefix.

The receiver checks negotiated formats, dimensions and encoding before
decompression. The decoded size must match the checked canonical stride
times height and fit in a raw message under the same 16 MiB payload cap.
Mono1 uses MSB-first bits with unused tail bits zero. Gray8 uses one
luminance byte per pixel. Zstandard requires the exact content size in its
frame header, a window of at most 16 MiB, and no external dictionary.
Skippable frames, concatenated frames, and trailing bytes are invalid. No
additional PPFB checksum is included. Zstandard frames can carry their usual
optional internal checksum.

The sender compares the complete raw and compressed message sizes. A tie,
expansion, or pre-write compression failure selects raw. If a transport
write fails after bytes may have entered the stream, close the connection.

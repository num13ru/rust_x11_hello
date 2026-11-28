# PPFB v3 wire format

This specification covers independent frame transport. PPFB version 2
retains its existing framing and raw framebuffer payload.

## Running PaperPad and PaperView

Start PaperSpoon with the fake provider and URL actions disabled for a local
check. The default encoding level is 1.

```sh
cargo run --release -p paperspoon -- 5581 /tmp/paperspoon-v3.log \
    --now-playing-fake --no-forward-url
```

For PaperView, open a second terminal and run:

```sh
cargo run -p paperview -- --transport-diagnostics --interactive
```

For PaperPad, deploy the verified ARMv7 build, unplug USB, open KUAL, and tap
**PaperPad → Run Paperpad DU (5m)**. The connection indicator should
fill and the application grid should appear. Enter `nowplaying start` in
PaperSpoon, try play/pause and seeking, then tap **Exit** on the Kindle. The
launcher records transport events and the final cleanup status locally.

PaperSpoon defaults to `--compression=auto`. It serves v2 clients with Raw
and negotiates Zstandard with capable v3 clients. Use `--compression=raw`
to serve Raw only, or `--compression=zstd` to require v3 clients that offer
Zstandard. Each v3 session can fall back to Raw per frame. The host's
`--compression-level=1|3|6` override remains available.

PaperView's `--transport` and PaperPad's `PAPERPAD_TRANSPORT` default to
`auto`. Use `v2` to force legacy Raw, `v3-raw` to force Raw under v3,
`v3-auto` to require v3 with either codec, or `v3-zstd` to require v3 and
Zstandard capability. In KUAL, **Run Paperpad DU with PPFB v2 (5m)** forces
legacy Raw transport.

The [validation report](benchmarks/ppfb-v3-validation.md) tracks desktop and
device evidence separately.

### Matched Kindle measurements

Start PaperSpoon with the fake provider and `--compression=auto` as above.
In KUAL’s **PaperPad** group, use **Measure PPFB v3 Raw (15m)** for one pass
and **Measure PPFB v3 Zstandard (15m)** for the other. Both use the DU display policy and the
same watchdog. Start fresh fake Now Playing for each pass by entering
`nowplaying stop` and then `nowplaying start` in PaperSpoon. Use the same
playback duration, controls, and network conditions on both runs. Tap
**Exit** at the end of each pass and retrieve the logs before the next one.

The measurement actions set `PAPERPAD_MEASURE_TRANSPORT=1`. PaperPad logs
process CPU time, current and peak resident memory, and network interface
counters at the beginning and end of the run and on active runtime wakeups
at least 30 seconds apart. The interface counters include other traffic.
Compare their deltas alongside `wire_bytes` from the PPFB receive records.
Use device monotonic timestamps for receiver and presentation timings.

## Connection setup

In default `auto` mode the client sends a v3 Hello first. If the host closes
or resets the connection before sending any HelloAck bytes, the client opens
a fresh socket to the same endpoint and sends a v2 Hello. Explicit v3 modes
report handshake errors. A malformed or partial reply fails the handshake.
Use `v2` explicitly if a legacy peer leaves the v3 probe open until timeout.

A dual-version host chooses the session version from the first Hello header.
v2 sends no acknowledgment. v3 requires a HelloAck before any frame or
control message. Its encoding mask is a subset of the offer. Raw is required
and Zstandard is optional. A strict Zstandard request requires Zstandard in
the acknowledgment. Version and encodings are negotiated on every reconnect.

| Host | Client selection | Result |
|---|---|---|
| v2 | `auto` or `v2` | Raw v2 |
| v2 | explicit v3 | Handshake fails |
| v2 + v3 | `v2` | Raw v2 |
| v2 + v3 | `auto` or `v3-auto` | v3 with Zstandard if available, otherwise Raw |
| v2 + v3 | `v3-raw` | Raw v3 |

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

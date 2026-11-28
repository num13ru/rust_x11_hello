# PPFB v3 wire format

This specification covers independent frame transport. PPFB version 2
retains its existing framing and raw framebuffer payload.

## Production settings

**PaperPad → Run Paperpad DU** uses automatic negotiation. PaperView defaults
to the same mode. PaperSpoon offers Zstandard level 1 by default, with per-frame
Raw fallback when compression does not reduce the payload.

- Use `PAPERPAD_TRANSPORT=v2`, PaperView's `--transport v2`, or **Run
  Paperpad DU with PPFB v2** in KUAL for legacy Raw transport.
- Use `v3-raw` on either client to require PPFB v3 with Raw frames.
- Use `v3-auto` to require v3 with either encoding, or `v3-zstd` to require
  v3 with Zstandard capability.
- PaperSpoon accepts `--compression=raw` for Raw only or `--compression=zstd`
  to require a v3 peer that offers Zstandard.

See the [validation report](benchmarks/ppfb-v3-validation.md#production-conclusion)
for measured benefits and limitations.

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

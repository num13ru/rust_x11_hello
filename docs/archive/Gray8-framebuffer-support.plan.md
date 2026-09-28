Implement the next milestone: **Gray8 framebuffer support end-to-end**, while preserving the current PaperSpoon/PaperPad ownership model and keeping Mono1 fully supported.

Repository:
`https://github.com/num13ru/rust_x11_hello`

Base branch:
`master`

Suggested branch:
`gray8-framebuffer`

## Goal

Extend the current framebuffer pipeline from Mono1-only to support both:

- `Mono1`
- `Gray8`

Gray8 is the new transport/application representation for grayscale content.

Do not replace Mono1. Both formats should remain first-class and selectable/negotiated through PPFB v2.

The target architecture remains:

```text
PaperSpoon
  application / diagnostic renderer
        ↓
  protocol framebuffer
        ↓
PPFB v2
        ↓
PaperPad
        ↓
DisplayBackend
   ├── X11
   └── MXCFB
```

PaperSpoon must continue to own application rendering and image decoding.

PaperPad must continue to receive already-rendered framebuffer pixels. Do not add JPEG decoding or general image rendering logic to the Kindle side.

## Scope

This PR should implement:

1. transport-independent Gray8 framebuffer representation
2. PPFB v2 Gray8 capability negotiation
3. Gray8 frame encoding and decoding
4. format-aware `RemoteFrame`
5. Gray8 diagnostic frames
6. Gray8 support in MXCFB
7. explicit X11 behavior for Gray8
8. physical-device validation path
9. one host-side JPEG example/fixture after Gray8 works
10. documentation and tests

Do not introduce raylib, animation, compression, delta updates, dirty rectangles, Gray4, or broad renderer rewrites in this PR.

---

## 1. Generalize framebuffer representation

The protocol crate currently assumes that a remote framebuffer is always `Mono1Frame`.

Introduce a format-aware framebuffer model without overengineering it.

The important properties of a remote frame are:

```text
width
height
pixel_format
stride
pixels
```

Support:

```text
Mono1
Gray8
```

Recommended semantics:

### Mono1

- row-major
- MSB-first
- `0 = white`
- `1 = black`
- stride = `ceil(width / 8)`
- unused trailing bits in the last byte of an odd-width row must remain white

### Gray8

- row-major
- one byte per pixel
- `0 = black`
- `255 = white`
- values between them represent increasing luminance
- stride = `width`
- no padding between rows

Be explicit about this polarity difference in documentation.

Do not silently reinterpret existing Mono1 semantics.

Create format-specific validation helpers.

Example conceptual structure:

```text
PixelFormat
├── Mono1
└── Gray8

Frame
├── width
├── height
├── pixel_format
├── stride
└── pixels
```

The exact Rust type design is up to the implementation, but avoid making device-specific framebuffer details part of `paper-protocol`.

## 2. Extend PPFB v2 capability negotiation

`V2Hello` currently advertises Mono1 only.

Extend the pixel-format capability mask so the Kindle can advertise supported remote formats.

Add:

```text
Mono1
Gray8
```

Requirements:

- preserve Mono1 compatibility
- keep reserved bits rejected
- unknown/unsupported advertised format bits should remain a protocol error unless there is a strong reason to change that behavior
- `V2Hello::supports(...)` must work for both formats
- tests must cover:
  - Mono1 only
  - Gray8 only if constructible by the protocol model
  - Mono1 + Gray8
  - unsupported bits
  - reserved bytes

If current startup logic always supports Gray8 only when the selected display backend supports it, capability reporting should reflect actual runtime capability rather than unconditional protocol support.

Do not advertise Gray8 if the selected backend cannot correctly present it.

## 3. Extend PPFB Frame messages

The current frame metadata already carries a pixel-format byte.

Use it rather than introducing a new message type.

Support:

```text
pixel_format = Mono1
pixel_format = Gray8
```

Decode frame payloads according to their declared format.

Validation must happen before the frame crosses into the display backend.

For Gray8:

```text
expected_payload_len = width * height
stride = width
```

For Mono1 retain all existing validation.

Tests must include:

- valid Mono1 roundtrip
- valid Gray8 roundtrip
- zero dimensions
- truncated Gray8 payload
- oversized Gray8 payload
- invalid Mono1 padding
- unsupported pixel format
- exact payload-size enforcement

## 4. Make `RemoteFrame` format-aware

`RemoteFrame` currently validates Mono1 and effectively assumes:

```text
stride = ceil(width / 8)
```

Remove that assumption.

`RemoteFrame` should expose:

```text
frame_id
width
height
pixel_format
stride
pixels
receive_decode_us
```

Validation should already have occurred at construction.

Do not make display backends infer format from stride.

The cache must also preserve pixel format.

A cached Gray8 frame must be redrawn as Gray8, not reconstructed as Mono1.

Add tests for:

- Mono1 cache metadata
- Gray8 cache metadata
- redraw preserving format
- invalid format/stride combinations rejected before backend entry

## 5. Preserve Mono1 behavior exactly

Mono1 is the known-good baseline.

Do not regress:

- protocol bytes
- odd-width handling
- diagnostic patterns
- cached redraw
- X11 upload
- MXCFB upload
- local Exit
- reconnect behavior

Existing Mono1 tests should remain valid wherever possible.

Avoid rewriting Mono1 code merely for symmetry unless the new shared abstraction clearly simplifies correctness.

## 6. Add Gray8 diagnostic patterns in PaperSpoon

Before adding JPEG support, create deterministic generated patterns.

At minimum implement:

### A. Horizontal gradient

```text
black → white
0 → 255
```

across the viewport width.

### B. 16-level grayscale bars

Use fixed values spanning the full range.

For example:

```text
0
17
34
...
255
```

The exact pattern may be horizontal or vertical, but it must be visually easy to inspect.

### C. Solid values

Provide at least:

```text
gray 0
gray 64
gray 128
gray 192
gray 255
```

Integrate these into the existing PaperSpoon diagnostic command path.

Suggested commands:

```text
frame gray-gradient WIDTHxHEIGHT
frame gray-bars WIDTHxHEIGHT
frame gray 128 WIDTHxHEIGHT
```

Exact CLI syntax may follow the repository's existing diagnostic style.

Do not remove existing Mono1 diagnostics.

## 7. Add Gray8 support to MXCFB

This is the primary target backend for grayscale.

Inspect the actual framebuffer metadata and existing MXCFB pixel conversion path.

The current device framebuffer has already been observed/modelled as:

```text
bits_per_pixel = 8
grayscale = 1
```

Do not assume that host Gray8 bytes can always be blindly memcpy'd without checking:

- framebuffer line length
- x/y offsets
- visible vs virtual resolution
- polarity
- device-specific grayscale interpretation
- region bounds

Implement Gray8 presentation through the existing MXCFB backend abstractions.

Requirements:

- remote viewport only
- local Exit strip remains device-owned
- line padding is respected
- remote frame never overwrites the local system UI region
- cached Gray8 frame can be restored
- frame cache updates only after successful presentation/update submission
- mismatched dimensions are rejected
- failure remains non-fatal to the main runtime

If the Kindle framebuffer uses direct 8-bit grayscale values, document the confirmed mapping.

If inversion is required, implement that explicitly and document it.

Do not guess silently.

## 8. Define X11 behavior deliberately

Do not force a large X11 grayscale implementation if it adds disproportionate complexity.

Choose one of these approaches based on the existing X11 capabilities:

### Preferred if straightforward

Support Gray8 through an X11 image path and preserve X11 as a functional grayscale backend.

### Acceptable for this PR

Keep X11 Mono1-only and make capability negotiation/backend selection reflect that.

If X11 is Mono1-only:

- Gray8 frames must be rejected cleanly by the X11 display backend
- no crash
- no cache replacement
- clear diagnostic
- PaperPad should not advertise Gray8 when running the X11 display backend

Do not silently convert Gray8 to Mono1 in PaperPad.

Any grayscale-to-Mono1 fallback belongs on the host side if added later.

## 9. Add host-side JPEG support only after generated Gray8 patterns work

Once Gray8 transport and MXCFB presentation are working, add one JPEG example.

Location:

```text
tools/paperspoon/assets/grayscale-example.jpg
```

If it is used strictly as a test fixture instead, prefer:

```text
tools/paperspoon/tests/assets/grayscale-example.jpg
```

Keep the asset reasonably small.

Use an image that contains:

- dark regions
- bright regions
- smooth gradients
- fine detail
- midtones

Avoid a nearly binary image.

### JPEG processing belongs entirely in PaperSpoon

Pipeline:

```text
JPEG
↓
decode
↓
RGB/YUV → luminance
↓
resize / fit to current viewport
↓
Gray8 frame
↓
PPFB v2
↓
PaperPad
```

Do not send JPEG bytes over PPFB.

Do not decode JPEG on Kindle.

Use a maintained Rust image-decoding dependency with minimal necessary features.

Avoid enabling unrelated codecs if the dependency allows feature selection.

## 10. Define image fit behavior

Use deterministic viewport behavior.

Recommended first implementation:

```text
fit-contain
```

Rules:

- preserve aspect ratio
- center image
- fill unused area with white
- never stretch
- never crop silently

Document the behavior.

Tests should cover:

- landscape image in portrait viewport
- portrait image in portrait viewport
- exact-fit image
- very small image
- odd viewport dimensions

Do not add configurable fit modes in this PR.

## 11. Keep application rendering Mono1 for now

Do not convert the existing application grid renderer to Gray8 unless required by the implementation.

The existing UI may continue to use:

```text
Mono1
```

while diagnostics/images use:

```text
Gray8
```

This is desirable because it proves both formats can coexist.

A later PR can decide whether regular application UI should move to Gray8 for antialiasing, images, and richer typography.

## 12. Frame deduplication must remain format-aware

The existing application-frame deduplication logic must not accidentally treat equal byte payloads in different formats as the same frame.

Frame identity/comparison must include at least:

```text
pixel_format
dimensions
pixels
```

A Mono1 payload and Gray8 payload must never compare equal solely because byte buffers happen to match.

Reconnect/new `Hello` must still force an authoritative frame.

## 13. Device validation sequence

Before testing JPEG, validate deterministic grayscale first.

Run on the physical Paperwhite 6 using MXCFB.

Check:

### Gradient

Confirm:

- left/right polarity is correct
- transition is monotonic
- there are visible intermediate gray levels
- no obvious wrapping or row corruption
- viewport boundaries are correct
- local Exit strip remains intact

### 16-level bars

Confirm:

- bars appear in expected order
- adjacent levels are distinguishable where the panel/waveform permits
- black and white endpoints are correct

### Solid values

Check:

```text
0
64
128
192
255
```

Confirm that brightness ordering is correct.

Only after those pass, test the JPEG fixture.

For JPEG verify:

- orientation
- aspect ratio
- centering
- dark/light polarity
- midtone detail
- no overwrite of Exit strip
- reconnect/cached redraw where practical

Record actual observed behavior.

Do not claim all 256 levels are physically distinguishable merely because Gray8 transport works.

## 14. Waveform handling

Do not redesign waveform policy in this PR unless grayscale cannot be displayed correctly without it.

Use the existing MXCFB update path initially.

If grayscale requires a different waveform/update mode:

- make the smallest explicit change required
- document it
- keep it localized to MXCFB
- record why the existing mode was insufficient

Do not introduce adaptive waveform heuristics yet.

A future PR can address:

```text
text / fast UI waveform
image / grayscale waveform
animation policy
ghosting management
```

## 15. Documentation

Update:

```text
README.md
AGENTS.md
```

Document:

- Mono1 and Gray8 are supported protocol framebuffer formats
- exact pixel semantics
- capability negotiation
- backend capability differences
- JPEG decoding is host-side only
- PaperPad remains format-aware but application-semantic-free
- MXCFB grayscale validation status
- X11 Gray8 status
- current waveform limitations

Do not document experimental behavior as guaranteed until confirmed on device.

## 16. Validation

Run:

```sh
make check
cargo test --workspace
make build
make verify
```

Also run any relevant targeted tests for:

```text
paper-protocol
paperspoon
mxcfb
display cache
```

If the repository already includes equivalent checks in `make check`, avoid redundant commands unless useful for diagnosing failures.

Separate results into:

- host/unit tests
- static ARM validation
- physical Kindle validation

## Definition of done

This PR is complete when:

- Mono1 continues to work unchanged.
- Gray8 is a first-class framebuffer format in `paper-protocol`.
- PPFB v2 negotiates Gray8 capability.
- Gray8 frames roundtrip through encode/decode.
- `RemoteFrame` and the frame cache preserve pixel format.
- MXCFB can present generated Gray8 patterns on the Kindle.
- Gray8 cannot overwrite the local Exit strip.
- backend capability reporting is truthful.
- X11 Gray8 behavior is explicit, supported or cleanly rejected.
- generated gradient and gray bars pass physical validation.
- one JPEG can be decoded by PaperSpoon, converted/resized to Gray8, transmitted, and displayed on the Kindle.
- JPEG bytes never reach PaperPad.
- all validation passes.
- documentation matches actual behavior.

## Out of scope / follow-ups

Do not implement these now:

- Gray4
- compression
- JPEG transport
- PNG support unless required by the chosen library
- raylib
- animations
- dirty rectangles
- delta frames
- adaptive waveforms
- general-purpose image gallery
- album-cover integration into the main UI
- large project-wide refactor

When finished, report:

1. architecture changes
2. protocol changes
3. backend support matrix
4. files changed
5. tests/checks run
6. physical Kindle observations
7. JPEG fixture behavior
8. remaining limitations
9. non-blocking follow-up ideas
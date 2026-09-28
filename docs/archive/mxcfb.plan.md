# Task: add a second PaperPad display backend using Kindle MXCFB

Repository:

`https://github.com/num13ru/rust_x11_hello`

## Goal

Add a second device-side display backend for PaperPad that writes the existing host-rendered framebuffer directly to the Kindle framebuffer using the Lab126/Kindle MXCFB interface.

The existing X11 backend must remain functional.

The intended architecture is:

```text
PaperSpoon
    |
    | PPFB v2 / Mono1Frame
    v
PaperPad application/runtime
    |
    +---------------------+
    | DisplayBackend      |
    +---------------------+
       |              |
       v              v
   X11 backend    MXCFB backend
```

This work must **not** change the PaperSpoon rendering architecture.

PaperSpoon remains responsible for:

- application layout
- application rendering
- application hit testing
- semantic action mapping
- application state

PaperPad remains responsible for:

- displaying the remote framebuffer
- forwarding input
- device-local Exit UI
- connection lifecycle
- display backend lifecycle

The protocol continues to carry backend-independent `Mono1Frame` data.

Do not introduce grayscale yet. The MXCFB backend should initially display the same Mono1 framebuffer currently used by the X11 backend.

---

# 1. Audit the existing display path

Before modifying code, identify every responsibility currently coupled to X11.

In particular inspect:

- `src/x11/`
- `src/x11/framebuffer.rs`
- application/event-loop code
- framebuffer caching
- redraw handling
- viewport geometry
- local Exit rendering
- X11 window geometry
- X11 input handling

Separate these concepts mentally into:

### Backend-independent

- accepted `Mono1Frame`
- current remote viewport dimensions
- cached successfully displayed frame
- application frame validation
- frame ID
- local/remote geometry policy
- connection lifecycle

### X11-specific

- X11 connection
- X11 window
- GC
- `XYBitmap`
- `PutImage`
- X11 `Expose`
- X11 image byte order
- X11 bitmap bit order
- X11 scanline padding
- X11 maximum request size

### MXCFB-specific

- `/dev/fb0`
- framebuffer metadata
- framebuffer memory mapping
- Kindle framebuffer pixel format
- Lab126 MXCFB ioctls
- update regions
- waveform/update mode
- update completion

Do not make MXCFB depend on X11 conversion types such as `X11BitmapAdapter`.

---

# 2. Introduce a display-backend boundary

Create a small device-side abstraction representing the operations PaperPad actually needs.

Keep it narrow.

For example, the conceptual API should provide operations equivalent to:

```text
DisplayBackend
    dimensions()
    display_remote_frame(...)
    redraw_cached_frame(...)
    draw_system_ui(...)
```

Do not blindly implement exactly this interface if the current ownership model suggests a cleaner shape.

Prefer an interface based on behavior rather than X11 concepts.

Avoid backend-neutral names containing:

- X11
- drawable
- GC
- PutImage
- Expose
- MXCFB ioctl terminology

The application/runtime should not know whether pixels are ultimately sent through X11 or `/dev/fb0`.

If framebuffer caching can cleanly live outside both backends, keep it backend-independent.

If successful presentation semantics differ enough that caching belongs to the backend, document that decision.

---

# 3. Keep X11 and MXCFB as sibling implementations

Do not model `MxcfbBackend` as a derivative, wrapper, or architectural extension of `X11BitmapAdapter`.

The two display paths must converge only at the backend-independent frame representation:

```text
                    Mono1Frame
                    /        \
                   /          \
                  v            v
          X11BitmapAdapter   MXCFB converter
                  |            |
                  v            v
              XYBitmap      /dev/fb0
                  |            |
                  v            v
              PutImage      MXCFB update
```

Everything below `Mono1Frame` is backend-specific.

In particular:

- do not reuse X11 scanline encoding for MXCFB
- do not make MXCFB aware of X11 image byte order
- do not expose X11 request-size concepts to MXCFB
- do not put MXCFB waveform/update concepts into the X11 backend
- do not create a shared abstraction merely because two low-level operations happen to manipulate bytes

Share only semantics that are genuinely backend-independent.

This separation is important because the X11 backend will remain the reference implementation for the same host-rendered frame.

---

# 4. Preserve X11 as the first implementation

Refactor the existing X11 display code to implement the new backend interface.

This should initially be behavior-preserving.

Do not combine the backend extraction with unrelated X11 cleanup.

The following existing X11 behavior must remain unchanged:

- accepted Mono1 frames render correctly
- `0 = white`
- `1 = black`
- remote content does not overwrite the local Exit strip
- framebuffer dimensions must match the negotiated remote viewport
- failed uploads do not replace the successful frame cache
- cached frames survive X11 `Expose`
- viewport changes invalidate incompatible cached frames
- X11 XYBitmap conversion still handles:
  - bit order
  - byte order
  - scanline unit
  - scanline padding
  - maximum request size

Keep the current X11 path available for regression testing.

---

# 5. Treat `/dev/fb0` and MXCFB as separate layers

Do not treat "writing the framebuffer" and "refreshing the e-ink panel" as one operation.

They are separate responsibilities.

The intended conceptual layering is:

```text
Mono1Frame
    ↓
native framebuffer conversion
    ↓
write pixels into /dev/fb0 mapping
    ↓
dirty/update region
    ↓
MXCFB ioctl
    ↓
e-ink controller refresh
```

`/dev/fb0` provides framebuffer memory.

MXCFB controls when and how the physical e-ink panel updates that framebuffer content.

Structure the code so these remain visibly separate.

A useful conceptual split is:

```text
FramebufferMemory
    - mmap
    - dimensions
    - line length
    - native pixel writes

MxcfbController
    - submit update
    - update region
    - update mode
    - waveform
    - update marker
    - wait for completion
```

Do not force this exact type structure if another implementation is cleaner, but preserve the responsibility boundary.

This matters for future grayscale, partial refresh, waveform selection, and ghosting policy.

---

# 6. Investigate the actual Kindle framebuffer ABI before implementing it

Do not guess Kindle MXCFB constants, structs, framebuffer format, waveform IDs, ioctl numbers, or update flags.

Do not copy `mxcfb.h` from an arbitrary Kindle project and assume it matches this device.

Kindle MXCFB ABI details have differed across device generations and kernels.

The implementation must be based on ABI information appropriate for the actual target Kindle.

The target device is the Paperwhite 6 used by this project.

Before implementing MXCFB:

1. inspect `/dev/fb0`
2. query framebuffer information with standard Linux framebuffer ioctls:
   - `FBIOGET_FSCREENINFO`
   - `FBIOGET_VSCREENINFO`
3. determine:
   - physical resolution
   - virtual resolution
   - line length
   - bits per pixel
   - pixel layout
   - framebuffer memory size
4. identify the MXCFB header/API supported by this Kindle kernel
5. obtain the exact definitions for:
   - update-region struct
   - update-data struct
   - waveform enum/constants
   - update-mode enum/constants
   - update marker
   - submit-update ioctl
   - wait-for-update-complete ioctl if supported

Prefer, in order:

1. headers from the actual Kindle/kernel environment
2. headers corresponding exactly to the device kernel/source release
3. previously verified definitions from the same Kindle generation/kernel
4. other external sources only as cross-checks

If this information is not available in the repository, add a small device inspection utility or documented shell/C probe rather than inventing constants.

Keep raw FFI/ioctl definitions isolated in one low-level module.

Suggested conceptual structure:

```text
src/display/
    mod.rs
    x11.rs
    mxcfb/
        mod.rs
        abi.rs
        framebuffer.rs
        update.rs
```

The exact layout may differ if another structure better matches the existing repository.

Document the source of every device-specific ioctl/ABI definition.

---

# 7. Implement safe framebuffer access

The MXCFB backend should open `/dev/fb0` and obtain framebuffer metadata.

Use the kernel-reported values rather than assuming the Paperwhite resolution.

Map only the required framebuffer memory.

Validate all calculations before creating slices or writing pixels.

Check for overflow when calculating:

```text
offset
row offset
line_length * height
x/y region offsets
mapped framebuffer size
```

Do not assume:

```text
line_length == width * bytes_per_pixel
```

because framebuffer rows may include padding.

Do not assume the framebuffer format from resolution alone.

Return descriptive errors when the reported framebuffer format is unsupported.

Any `unsafe` code should be isolated behind a small safe wrapper and have comments explaining the invariant required by each unsafe operation.

---

# 8. Implement Mono1 -> native framebuffer conversion

Keep the network/wire representation unchanged:

```text
Mono1Frame
0 = white
1 = black
MSB-first
packed bits
```

The MXCFB backend must convert this into whatever native framebuffer representation `/dev/fb0` exposes.

Do not reuse the X11 XYBitmap encoding.

Create a backend-specific conversion path.

Initially support only the framebuffer format actually verified on the target Kindle.

Fail explicitly on unsupported formats instead of silently producing incorrect pixels.

The converter must respect:

- framebuffer `line_length`
- framebuffer dimensions
- remote viewport origin
- local system strip
- clipping rules
- Mono1 odd-width padding bits
- black/white polarity

No remote application frame may overwrite PaperPad's local Exit area.

---

# 9. Keep the first MXCFB implementation Mono1-only

Do not introduce grayscale as part of this backend migration.

The purpose of this change is to prove that the **same host-rendered framebuffer** can be presented through two independent device backends without changing PaperSpoon.

The architecture after this work should be:

```text
PaperSpoon renderer
        |
        v
    Mono1Frame
      /     \
     /       \
    v         v
  X11       MXCFB
```

This gives the project a useful A/B reference.

If the same `Mono1Frame` displays correctly through X11 but incorrectly through MXCFB, the problem is likely in:

- native framebuffer conversion
- framebuffer addressing
- MXCFB ABI usage
- refresh submission

rather than:

- PaperSpoon rendering
- PPFB
- application layout

Do not weaken this diagnostic property by introducing Gray4/Gray8 in the same change.

Grayscale should be a separate follow-up after the direct framebuffer backend is known to work correctly.

---

# 10. Keep the local system UI device-owned

The current bottom 72-pixel Exit strip remains PaperPad-owned.

The MXCFB backend must render it locally.

Do not send the Exit control from PaperSpoon.

Maintain the same physical/remote partition:

```text
physical screen
┌──────────────────────────┐
│                          │
│ PaperSpoon framebuffer   │
│ remote viewport          │
│                          │
├──────────────────────────┤
│ PaperPad local Exit      │
└──────────────────────────┘
```

Where possible, reuse backend-independent system UI geometry.

The rendering implementation itself may be backend-specific.

---

# 11. Add explicit backend selection

Do not silently replace X11.

Provide an explicit way to choose the display backend.

Prefer a startup configuration such as:

```text
--display-backend=x11
--display-backend=mxcfb
```

or an equivalent environment/configuration mechanism consistent with the repository.

During this migration:

- keep X11 as the default unless there is a strong existing reason otherwise
- fail clearly when MXCFB is selected but `/dev/fb0` or required MXCFB capabilities are unavailable
- log the selected display backend at startup

Do not silently fall back from MXCFB to X11 after an MXCFB initialization failure.

A fallback would hide backend bugs during device testing.

---

# 12. Implement MXCFB update submission

Writing framebuffer memory alone is not sufficient for e-ink.

After updating framebuffer pixels, submit an MXCFB refresh for the affected region.

Initially prioritize correctness over sophisticated refresh optimization.

Start with a conservative update policy suitable for static Mono1 UI.

The update region should be explicit.

For remote frames, refresh only the remote viewport unless a full-screen refresh is required by verified device behavior.

For local Exit redraw, refresh only its region where practical.

Use update markers correctly if required by the target API.

If waiting for update completion is needed to safely reuse state, implement it.

Do not invent an asynchronous refresh scheduler in this PR.

---

# 13. Separate framebuffer write from e-ink refresh policy

Avoid coupling raw framebuffer mutation directly to one hard-coded waveform.

Structure the backend so these concepts remain distinguishable:

```text
pixel conversion
    ↓
framebuffer write
    ↓
dirty region
    ↓
MXCFB update request
    ↓
waveform/update policy
```

The first implementation may use only one conservative policy, but future work should be able to introduce:

- fast monochrome UI updates
- higher-quality grayscale updates
- partial updates
- full refreshes
- ghosting management

without rewriting framebuffer access.

Do not implement those future policies now.

---

# 14. Preserve frame success semantics

The current architecture distinguishes a received frame from a successfully displayed frame.

Keep that property.

The MXCFB equivalent should be approximately:

```text
receive Frame
    ↓
validate dimensions
    ↓
convert
    ↓
write framebuffer memory
    ↓
submit MXCFB update
    ↓
success
    ↓
replace cached successful frame
```

A frame that fails during:

- validation
- conversion
- framebuffer write
- update submission

must not become the authoritative cached frame.

Document what "successfully displayed" means when the kernel accepts an asynchronous MXCFB update but physical panel completion has not yet been observed.

If necessary, distinguish:

```text
submitted successfully
```

from:

```text
update completed
```

internally.

Do not make stronger physical-display guarantees than the kernel API can provide.

---

# 15. Handle redraw semantics without X11 Expose

MXCFB has no X11 `Expose`.

Define explicitly what causes redraws for the MXCFB backend.

At minimum consider:

- first startup
- first successful remote frame
- remote frame replacement
- viewport change
- local system UI state change
- process returning after sleep/wake if the framebuffer contents cannot be assumed valid

Do not carry `Expose` as a backend-neutral concept.

Instead, expose a semantic operation such as:

```text
redraw
restore
present_cached_frame
```

if such an abstraction is actually required.

---

# 16. Investigate X11 input + MXCFB display coexistence explicitly

This is one of the main unknowns of the migration.

The current architecture gets both:

- window lifecycle
- touchscreen pointer events

from X11.

The MXCFB backend changes display output, but it does not automatically provide an input replacement.

Therefore explicitly investigate whether this intermediate architecture works:

```text
input backend:   X11
display backend: MXCFB
```

This is a valid and potentially desirable intermediate architecture.

Do not migrate to evdev merely because direct framebuffer output makes a fully non-X11 architecture look cleaner.

First verify:

- whether the fullscreen X11 window can remain mapped and continue receiving pointer events
- whether direct writes to `/dev/fb0` remain visible while the X server is running
- whether X11 causes repaints that overwrite MXCFB framebuffer contents
- whether MXCFB refreshes cause X11-visible state to become inconsistent
- whether X11 `Expose` or Kindle framework redraws interfere with direct framebuffer rendering
- whether the fullscreen X11 window is still required for touch ownership
- whether direct framebuffer output survives normal Kindle UI activity
- whether sleep/wake changes any of these assumptions

If `X11 input + MXCFB display` works reliably, keep it for this change.

If it does not work, document the concrete observed failure before expanding scope.

An evdev input backend should normally be a separate follow-up.

---

# 17. Keep input backend changes out of scope

This task is about the **display backend**.

Do not simultaneously migrate touchscreen input from X11 to evdev unless direct framebuffer operation makes the current architecture impossible to test or operate.

Do not:

- move application hit testing to evdev
- redesign touch semantics
- change PPFB pointer messages
- replace current contact cancellation logic
- introduce a generic input backend solely for symmetry with `DisplayBackend`

If a minimal refactor is required to separate X11 input from X11 display, keep it narrowly scoped to that separation.

---

# 18. Preserve an A/B reference path

Treat the existing X11 backend as the known comparison path for MXCFB development.

The same `Mono1Frame` should be runnable through either backend.

During implementation and debugging, preserve the ability to compare:

```text
PaperSpoon
    |
    v
same Mono1Frame
    |
    +----> X11
    |
    +----> MXCFB
```

When debugging incorrect output, use this distinction:

### Wrong in both X11 and MXCFB

Investigate:

- PaperSpoon renderer
- Mono1 generation
- protocol/frame dimensions
- shared geometry

### Correct in X11, wrong in MXCFB

Investigate:

- framebuffer format
- stride
- native pixel conversion
- offset calculation
- MXCFB ABI
- update region
- waveform/update submission

### Correct pixels in framebuffer memory but wrong physical panel output

Investigate:

- MXCFB update request
- waveform
- update mode
- region
- update completion
- Kindle display-controller behavior

Keep the code structured so this comparison remains possible after the PR.

---

# 19. Logging

Add concise backend-aware diagnostics.

Examples of useful events:

```text
display backend=x11
display backend=mxcfb
mxcfb framebuffer width=... height=... bpp=... line_length=...
mxcfb frame submitted id=... region=...
mxcfb frame failed id=... error=...
mxcfb cache updated id=...
mxcfb update completed marker=...
```

For ABI discovery/debugging, it may also be useful to log once:

```text
mxcfb abi=...
fb visual=...
fb xres=...
fb yres=...
fb xres_virtual=...
fb yres_virtual=...
fb bits_per_pixel=...
fb line_length=...
```

Do not log one line per framebuffer row or pixel operation.

Logs should remain suitable for retrieval through the existing Kindle/MTP debugging workflow.

---

# 20. Host-testable code

Keep as much logic as possible testable without a Kindle.

Extract pure functions for:

- Mono1 → native framebuffer pixel conversion
- stride handling
- framebuffer-region calculation
- clipping/validation
- remote viewport offset calculation
- dirty-region calculation
- update-region validation

Do not make unit tests call `/dev/fb0`.

The actual:

- `open`
- `mmap`
- `ioctl`

boundary should be thin.

Add unit tests for conversion with deliberately awkward dimensions such as:

```text
1x1
7x1
8x1
9x1
1271x1
1272x1
```

Also test:

- odd Mono1 widths
- padded framebuffer line lengths
- first and last pixels
- black/white polarity
- remote viewport bottom boundary
- prevention of writes into the Exit strip
- integer-overflow rejection

---

# 21. Physical-device validation

Do not claim MXCFB support is complete from cross-compilation alone.

A real Kindle run is required.

Create a documented manual test sequence.

At minimum display these existing PaperSpoon diagnostic frames:

```text
frame corners 1272x1624
frame border 1272x1624
frame checkerboard 1272x1624
frame horizontal 1272x1624
frame black 1272x1624
frame white 1272x1624
```

Verify physically:

1. orientation is correct
2. black and white polarity is correct
3. rightmost remote pixel is visible
4. bottom remote row is visible
5. no remote pixel overwrites the Exit strip
6. Exit remains visible
7. Exit remains usable
8. subsequent remote frames replace previous frames
9. reconnect still restores an authoritative frame
10. malformed/mismatched frames do not destroy the last valid display
11. sleep/wake behavior is understood
12. X11 and MXCFB do not unexpectedly fight over screen contents
13. touchscreen events still arrive when using MXCFB as the display backend
14. X11 repaint/expose behavior does not unexpectedly destroy direct framebuffer output

Capture the relevant device logs.

If possible during debugging, display the same diagnostic frame once through X11 and once through MXCFB and compare the results.

---

# 22. Documentation

Update `AGENTS.md` and README to document the new boundary.

Document clearly:

```text
PaperSpoon renders backend-independent Mono1 frames.

PaperPad can present those frames through multiple device display backends.

X11:
Mono1 -> X11 XYBitmap -> PutImage

MXCFB:
Mono1 -> native framebuffer pixels -> /dev/fb0 -> MXCFB update
```

Also document that:

- X11 and MXCFB are sibling display backends
- `X11BitmapAdapter` is not a base abstraction for MXCFB
- `/dev/fb0` framebuffer writes and MXCFB panel updates are separate layers
- the first MXCFB implementation intentionally remains Mono1-only
- X11 remains a reference backend for diagnosing framebuffer issues
- the initial implementation may use X11 for input while using MXCFB for display
- evdev migration is separate future work unless proved necessary
- MXCFB ABI definitions must be tied to the tested Kindle/kernel

State device-specific limitations and what was actually verified on the physical Kindle.

Do not generalize Paperwhite 6 findings to all Kindle generations.

---

# 23. Validation

Run the existing repository gates:

```sh
make check
make build
make verify
git diff --check
```

Run:

```sh
cargo test --workspace
```

where not already covered.

The existing X11 tests must continue to pass.

Add tests for backend-independent display logic and MXCFB pixel conversion.

Clearly separate final validation into:

```text
Host tests
ARM/static build
Physical Kindle MXCFB test
```

Do not report the physical section as passing unless it was actually performed.

---

# Definition of done

This task is complete when:

- X11 remains a working display backend.
- MXCFB exists as a second selectable display backend.
- PaperSpoon and PPFB do not know which backend is active.
- Both backends consume the same `Mono1Frame`.
- X11 and MXCFB are sibling implementations below the `Mono1Frame` boundary.
- MXCFB does not depend on `X11BitmapAdapter`.
- X11-specific XYBitmap logic remains isolated from MXCFB.
- `/dev/fb0` framebuffer-memory operations are separated from MXCFB e-ink update submission.
- MXCFB ABI definitions are based on verified Kindle/kernel interfaces rather than guessed or arbitrarily copied constants.
- `/dev/fb0` geometry and stride are discovered at runtime.
- remote application pixels cannot overwrite PaperPad's local Exit area.
- framebuffer writes use the actual device format.
- framebuffer changes are followed by correct MXCFB update submission.
- failed presentations do not replace the last successful frame state.
- backend selection is explicit.
- the X11 backend remains available as a fallback/reference implementation.
- the first MXCFB implementation remains Mono1-only.
- the same Mono1 frames can be compared through X11 and MXCFB for debugging.
- coexistence of X11 input with MXCFB display is explicitly tested and documented.
- host tests cover pure framebuffer conversion logic.
- ARM/static validation passes.
- physical-device requirements and results are documented accurately.

## Out of scope

Do not implement in this task:

- grayscale protocol
- Gray4/Gray8 rendering
- animations
- raylib
- dirty rectangle transport
- frame compression
- framebuffer deltas
- advanced waveform selection
- automatic ghosting management
- evdev migration unless strictly required to make MXCFB display testing possible
- removal of X11
- architecture-wide refactoring unrelated to the display backend

## Final report

When finished, report:

1. architecture changes
2. files added/changed
3. how `DisplayBackend` is structured
4. how X11 and MXCFB remain independent below `Mono1Frame`
5. verified framebuffer format
6. verified MXCFB ABI source
7. how `/dev/fb0` writes and MXCFB refresh submission are separated
8. backend selection mechanism
9. whether `X11 input + MXCFB display` was verified on the device
10. host tests run
11. ARM/static checks run
12. physical Kindle tests performed
13. A/B comparison results between X11 and MXCFB, if performed
14. known MXCFB limitations
15. follow-up work, clearly separated from this PR

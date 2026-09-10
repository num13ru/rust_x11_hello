# Task: Migrate PaperPad/PaperSpoon to a Host-Rendered Framebuffer Architecture

Work from the current `master` branch of `num13ru/rust_x11_hello`.

The previous structural refactoring is complete. This task is the architectural migration from the current Kindle-rendered semantic-button UI to a host-rendered framebuffer model.

## Assumptions

The current system approximately works like this:

```text
PaperPad / Kindle

X11 events
    ↓
local contact tracking
    ↓
local grid hit testing
    ↓
button ID
    ↓
SemanticAction
    ↓
event action=<semantic-id>;
    ↓
PaperSpoon
    ↓
Hammerspoon
```

Rendering also currently belongs to PaperPad:

```text
PaperPad UI geometry
    ↓
X11 rectangles/text
    ↓
Kindle display
```

The target architecture is:

```text
                    macOS

              ┌─────────────────┐
              │   PaperSpoon    │
              │                 │
              │ UI state        │
              │ layout          │
              │ hit testing     │
              │ rendering       │
              │ action dispatch │
              └───────┬─────────┘
                      │
             frames ↓ │ ↑ pointer events
                      │
              ┌───────┴─────────┐
              │    PaperPad     │
              │                 │
              │ X11 window      │
              │ frame blitter   │
              │ touch adapter   │
              │ local Exit UI   │
              │ transport       │
              └─────────────────┘

                    Kindle
```

PaperPad should ultimately become a thin e-ink terminal, but it must retain a small amount of **device-local system UI**.

PaperPad must continue to own an always-available local **Exit** control.

The Exit control must:

- work without PaperSpoon running;
- work while disconnected from PaperSpoon;
- work during protocol failures;
- work during framebuffer rendering failures;
- never depend on host-generated pixels;
- never require Hammerspoon or any other macOS integration;
- remain usable during physical-device testing;
- terminate PaperPad cleanly.

PaperPad must **not know** about host application controls such as:

- media buttons;
- tmux actions;
- Zoom actions;
- profiles/pages;
- active macOS application;
- host UI layout.

The local Exit control is explicitly outside that rule because it belongs to the PaperPad runtime itself rather than the remote application.

## Primary goal

Complete the migration all the way to the framebuffer architecture.

Do not stop after adding framebuffer support alongside the old semantic system.

Temporary compatibility code is acceptable during intermediate phases, but the final state must remove the obsolete Kindle-side **application** UI and semantic architecture while retaining the local PaperPad Exit control.

---

# Phase 0 — Establish the migration baseline

Before changing behavior:

1. Read `AGENTS.md`, `README.md`, and the current implementation.
2. Run all existing checks.
3. Record the current passing state.
4. Create a dedicated migration branch.
5. Keep the current `master` commit as the known-good semantic-grid baseline.

Inspect specifically:

- `src/app.rs`
- `src/ui/`
- `src/x11/display.rs`
- `src/x11/events.rs`
- `src/x11/render.rs`
- `src/net/`
- `crates/paper-protocol/`
- `tools/paperspoon/`

Do not begin by deleting the existing UI.

The first milestone is proving that PaperPad can reliably display a host-generated framebuffer on the physical Kindle.

Keep the existing local Exit path functional throughout every migration phase.

---

# Phase 1 — Separate system UI from remote application UI

Before introducing framebuffer transport, make the ownership boundary explicit.

The current UI contains both application buttons and the Exit control. Split these concepts:

```text
PaperPad-owned system UI
└── Exit

PaperSpoon-owned application UI
└── everything else
```

Do not treat Exit as another remote semantic action.

Refactor only as much as necessary so that Exit can survive removal of:

- application grid geometry;
- semantic action mappings;
- host-action hit testing;
- host application renderer.

The local Exit implementation should have its own clearly named system-level geometry and handling.

For example conceptually:

```text
SystemUi
└── ExitControl
```

rather than leaving Exit embedded in the host application button array.

Preserve the current visual Exit control initially unless there is a strong reason to change it.

---

# Phase 2 — Define the screen ownership model

Reserve part of the physical display for PaperPad system UI.

Recommended model:

```text
physical Kindle screen
┌──────────────────────────────┐
│                              │
│                              │
│   remote content viewport    │
│   owned by PaperSpoon        │
│                              │
│                              │
├──────────────────────────────┤
│            Exit              │
│       owned by PaperPad      │
└──────────────────────────────┘
```

PaperSpoon should render only the **remote content viewport**, not the entire physical screen.

PaperPad owns:

```text
physical screen dimensions
system UI dimensions
remote content viewport
```

PaperSpoon needs to know only the remote viewport dimensions required for rendering.

For example:

```text
physical_width  = 1272
physical_height = 1696

exit_height     = <existing or chosen value>

content_width   = 1272
content_height  = 1696 - exit_height
```

Do not allow PaperSpoon to draw underneath the Exit area.

This avoids:

- host pixels obscuring Exit;
- visual conflicts;
- host-dependent Exit visibility;
- ambiguous pointer ownership.

The exact Exit height should come from the existing proven PaperPad geometry where practical rather than introducing a gratuitously different size during migration.

---

# Phase 3 — Define coordinate spaces explicitly

There are now two coordinate systems:

```text
physical coordinates
    Kindle/X11 screen

remote viewport coordinates
    PaperSpoon framebuffer
```

PaperPad must translate between them.

For example, if the remote viewport begins at `(0, 0)`:

```text
physical touch (x, y)

if inside local Exit:
    handle locally
else if inside remote viewport:
    send remote pointer (x, y)
else:
    ignore
```

If system UI later moves to another edge, the mapping must still be explicit rather than assumed.

PaperSpoon must receive coordinates relative to the framebuffer it rendered.

Do not send physical Kindle coordinates and require PaperSpoon to understand PaperPad system chrome.

This preserves the architectural boundary:

```text
PaperSpoon framebuffer coordinates
        ↕
PaperPad remote viewport
```

---

# Phase 4 — Define the framebuffer representation

Introduce a transport-independent framebuffer type.

Start with exactly one wire pixel format:

```text
Mono1
```

Define it explicitly.

Recommended logical representation:

```text
width: u16
height: u16
stride: usize
pixels: Vec<u8>
```

Use:

```text
stride = ceil(width / 8)
payload_size = stride * height
```

The dimensions refer to the **remote content viewport**, not necessarily the entire physical Kindle screen.

Define the bit representation precisely and test it.

For example:

```text
row-major
one bit per pixel
MSB-first within each byte
0 = white
1 = black
no inter-row padding in the wire representation
```

The wire format must not be the native X11 framebuffer format.

PaperPad is responsible for adapting the stable Mono1 wire representation to the X11 server's required bitmap representation.

Add tests covering:

- odd widths not divisible by eight;
- stride calculation;
- first/last pixel in a row;
- byte boundaries;
- all-white frame;
- all-black frame;
- checkerboard pattern;
- exact payload length validation.

Do not add grayscale yet.

Do not add PNG/JPEG.

Do not add compression yet.

---

# Phase 5 — Replace the line-oriented application protocol with framed messages

The current protocol is suitable for short textual semantic messages but not arbitrary binary framebuffer payloads.

Introduce protocol v2 with explicit message framing.

Use a small fixed-size header containing enough information to safely decode a TCP stream:

```text
magic
protocol version
message type
flags/reserved
payload length
```

Exact encoding is an implementation decision, but it must be:

- deterministic;
- endian-defined;
- bounded;
- testable without networking;
- able to reject absurd payload lengths before allocating memory;
- able to parse multiple consecutive messages from one TCP stream;
- able to handle partial reads.

Define initial message types.

PaperPad → PaperSpoon:

```text
Hello
PointerDown
PointerUp
ViewportChanged
```

PaperSpoon → PaperPad:

```text
Frame
```

`Hello` should provide at least:

```text
protocol version
remote viewport width
remote viewport height
supported pixel format(s)
```

Optionally include the physical display size and system UI insets for diagnostics, but PaperSpoon must not need those values to lay out its application UI.

A frame should provide at least:

```text
frame ID
width
height
pixel format
pixel payload
```

Pointer events should provide at least:

```text
phase
x
y
```

These coordinates are relative to the remote framebuffer viewport.

Do not put the local Exit control into protocol v2.

Do not send an `Exit` semantic action to PaperSpoon when it is pressed.

Exit is handled completely locally.

Do not put semantic action identifiers into protocol v2.

Do not put UI widget descriptions into the protocol.

Do not invent a remote scene graph.

The protocol is for **pixels and remote input**, not UI semantics.

Keep UDP discovery conceptually separate from framebuffer transport.

---

# Phase 6 — Prove host → Kindle framebuffer transport

Before moving remote input handling, make PaperPad capable of receiving and displaying a framebuffer while the existing semantic application input path still works.

This phase is deliberately transitional.

PaperSpoon should initially generate simple diagnostic frames rather than the full production UI.

Recommended device-test frames:

1. all white;
2. all black;
3. alternating horizontal lines;
4. checkerboard;
5. border around the exact remote viewport;
6. asymmetric corner markers;
7. large regions showing orientation.

During all tests the local PaperPad Exit control must remain visible and functional.

The host-generated diagnostic framebuffer must never overwrite it.

The asymmetric pattern is important for detecting:

- reversed bit order;
- reversed byte order;
- incorrect stride;
- upside-down orientation;
- horizontal/vertical offset;
- clipping.

---

# Phase 7 — Implement the PaperPad X11 framebuffer compositor/blitter

The PaperPad renderer now has two inputs:

```text
host framebuffer
+
local system UI
```

Conceptually:

```text
remote MonoFrame
      ↓
blit into remote viewport
      +
draw local Exit control
      ↓
X11 window
```

Do not make PaperSpoon responsible for compositing the local Exit control.

The X11 layer must be able to redraw both components after an `Expose` event:

```text
cached remote framebuffer
+
local Exit UI
```

Add an X11 framebuffer adapter with a narrow responsibility:

```text
MonoFrame
    ↓
X11-compatible bitmap chunks
    ↓
remote viewport
```

Do not assume the wire layout can be passed directly to X11.

Inspect the actual X11 setup information and account for:

- bitmap scanline padding;
- bitmap bit order;
- byte order;
- drawable characteristics;
- maximum safe request size.

Do not assume the entire frame fits into one X11 request.

Implement row/tile chunking.

Important behavior:

- retain the most recently accepted remote framebuffer locally;
- an X11 `Expose` event must redraw from the cached framebuffer;
- redraw local Exit after framebuffer drawing when necessary;
- `Expose` must not require asking PaperSpoon for another frame;
- incomplete/corrupted network frames must never replace the last valid framebuffer;
- dimension mismatches must be rejected;
- a failed X11 upload must not make Exit unusable;
- X11 errors must produce useful device diagnostics.

Keep the existing X11 fullscreen window lifecycle.

---

# Phase 8 — Physical-device framebuffer gate

Do not proceed to deleting the old application renderer until framebuffer rendering is proven on the actual Paperwhite.

Build and deploy according to `AGENTS.md`.

Verify on device:

```text
PaperSpoon
   ↓
TCP frame
   ↓
PaperPad
   ↓
Mono1 decode
   ↓
X11 upload into content viewport
   ↓
visible Kindle image

PaperPad
   ↓
local Exit renderer
   ↓
visible independently below/around content
```

Check:

- correct remote viewport dimensions;
- correct physical placement;
- correct orientation;
- correct black/white polarity;
- no row corruption;
- no corruption at chunk boundaries;
- correct rightmost pixels;
- correct bottom row of the remote viewport;
- no host pixels written into Exit area;
- repeated frames;
- redraw after Expose;
- reconnect followed by frame display;
- Exit while connected;
- Exit while PaperSpoon is stopped;
- Exit after PaperSpoon disconnects;
- Exit after malformed/failed frame handling;
- watchdog fallback.

Retrieve and inspect Kindle logs.

Do not claim framebuffer rendering works until this gate passes on the physical device.

---

# Phase 9 — Move the existing application UI renderer to PaperSpoon

Once framebuffer display is proven, reproduce the existing PaperPad application controls from PaperSpoon.

The goal at this stage is visual/functional equivalence, not a UI redesign.

Move/reimplement:

```text
application grid geometry
application button rectangles
application labels
remote status
```

Do **not** move Exit.

The host-rendered screen should therefore reproduce only the application area.

Create a host-side UI model approximately like:

```text
UI state
    ↓
layout
    ↓
software rendering
    ↓
MonoFrame
    ↓
send Frame
```

Keep rendering deterministic and independent from the network layer.

At the end of this phase:

- PaperSpoon renders the normal application controls;
- PaperPad renders Exit;
- both appear as a coherent screen;
- the Kindle application renderer may still temporarily exist for migration fallback.

---

# Phase 10 — Move remote pointer input to the host

Target flow:

```text
X11 touch
    ↓
PaperPad
    ├── hit local Exit? → exit locally
    │
    └── inside remote viewport?
            ↓
       normalize coordinates
            ↓
       PointerDown/PointerUp
            ↓
       PaperSpoon
            ↓
       host contact tracking
            ↓
       host hit testing
            ↓
       action
```

The order of handling is important.

PaperPad must perform local system hit testing **before** forwarding remote pointer events.

An Exit press must never also reach PaperSpoon.

PaperPad should remain responsible for Kindle/X11-specific normalization and filtering.

Move remote application concepts to PaperSpoon:

```text
ContactTracker
application hit_button()
application button geometry
action_for_button()
```

Do not move the local Exit hit test to PaperSpoon.

Preserve press/release semantics.

Do not collapse remote input into `click x y`.

Motion events are not required for the initial migration.

---

# Phase 11 — Make local Exit contact handling robust

The Exit control must behave like a real local button rather than an unsafe single-coordinate trap.

Preserve or implement proper local contact semantics:

```text
Down inside Exit
    ↓
arm Exit

Up inside same Exit
    ↓
terminate
```

Do not exit on:

```text
Down outside → Up inside
Down inside → Up outside
auxiliary X11 button detail
unmatched Up
malformed/repeated primary press sequence
```

If an active Exit contact is interrupted by:

- Unmap;
- geometry change;
- X11 lifecycle transition;

cancel it safely.

This local tracker can be tiny and system-specific.

Do not retain the generic application grid tracker on PaperPad solely for Exit.

---

# Phase 12 — Verify coordinate correctness end-to-end

Before deleting semantic actions from PaperPad, verify on the physical Kindle that remote coordinates survive the complete path correctly.

Test:

- center of every host-rendered application button;
- application-area edges;
- gaps between buttons;
- remote framebuffer bottom edge immediately adjacent to Exit;
- Exit itself;
- press remote area / release Exit;
- press Exit / release remote area;
- press inside / release outside;
- rapid repeated taps;
- reconnect.

Explicitly verify:

```text
remote button touch
→ exactly one PaperSpoon action

Exit touch
→ zero PaperSpoon actions
→ PaperPad terminates locally
```

Only after this passes should the old semantic Kindle-side application path be removed.

---

# Phase 13 — Move action dispatch completely to PaperSpoon

PaperSpoon should own:

```text
remote pointer state
remote UI geometry
remote hit testing
semantic action mapping
Hammerspoon dispatch
```

The semantic action identifiers may still exist inside PaperSpoon.

They must no longer cross the PaperPad/PaperSpoon wire protocol.

PaperPad must have no knowledge of host application semantics.

PaperPad still knows about one local operation:

```text
Exit PaperPad
```

This must not be represented as a PaperSpoon semantic action.

---

# Phase 14 — Move remote status/display state completely to PaperSpoon

Remove the old:

```text
display <text>
```

concept.

PaperSpoon updates remote status by rendering a new framebuffer.

PaperPad may still render its own local system UI:

```text
Exit
```

and, if truly required, local fatal/recovery diagnostics.

Do not mix PaperSpoon status rendering back into PaperPad.

---

# Phase 15 — Full cutover: delete Kindle-side application UI

After framebuffer and remote coordinate paths are verified, remove the old application UI.

Delete obsolete PaperPad application code including, as applicable:

```text
application grid geometry
remote application button mappings
SemanticAction mapping on Kindle
application hit testing
application labels/text
application status renderer
old rectangle/text application renderer
```

Retain/restructure whatever minimal UI code is required for:

```text
PaperPad system UI
└── Exit
```

The resulting PaperPad architecture should be approximately:

```text
PaperPad

main
├── discovery
├── connection
├── protocol
├── X11 window lifecycle
├── remote framebuffer cache
├── X11 framebuffer blitter
├── X11 input normalization
├── system UI
│   └── Exit
└── lifecycle
```

There should be no host application-specific buttons left on PaperPad.

---

# Phase 16 — Remove protocol v1

Once v2 is physically proven and both binaries have migrated, delete:

```text
event action=<semantic-id>;
display <text>
parse_action_line()
format_action_line()
parse_display_command()
old stdin display forwarding
semantic wire-protocol tests
```

Do not remove or replace the local Exit path.

The Exit control does not belong to either protocol version.

---

# Phase 17 — Make framebuffer updates state-driven

PaperSpoon should not continuously stream frames.

Use:

```text
UI state changed
      ↓
render
      ↓
frame differs
      ↓
send
```

Continue using full frames initially.

Keep a monotonically increasing `frame_id`.

The local Exit control must not cause host framebuffer regeneration because its visual state belongs to PaperPad.

If pressed-state feedback for Exit is desired, PaperPad should render that locally.

---

# Phase 18 — Reconnect and disconnected behavior

Define reconnect semantics explicitly.

Recommended connected flow:

```text
PaperPad starts
    ↓
local Exit immediately available
    ↓
connects to PaperSpoon
    ↓
Hello(content viewport)
    ↓
PaperSpoon sends current Frame
```

Disconnected flow:

```text
PaperSpoon unavailable
    ↓
PaperPad remains running
    ↓
last framebuffer may remain visible
    ↓
local Exit remains functional
```

If no valid host frame has ever been received, PaperPad may show a minimal local disconnected/blank state.

Whatever is shown, Exit must remain visible and functional.

---

# Phase 19 — Protect against frame backlog

Framebuffer state should be latest-wins.

Do not apply that policy to input.

Remote pointer events must preserve order.

The local Exit path must bypass network queues completely.

Even if:

- the socket is blocked;
- PaperSpoon stops reading;
- frame processing is stalled;

Exit input must be handled locally without waiting for network I/O.

Avoid any design where the X11 event loop can block indefinitely writing a pointer message to PaperSpoon.

---

# Phase 20 — Do not optimize transport prematurely

After the full-frame architecture works reliably, measure:

```text
frame generation time
Mono1 conversion time
TCP transfer time
PaperPad receive time
X11 conversion time
X11 upload time
visible e-ink refresh behavior
```

Only then consider:

```text
dirty rectangles
RLE
general-purpose compression
grayscale
partial e-ink refresh control
```

The Exit control should remain local regardless of future framebuffer optimizations.

---

# Phase 21 — Final ownership model

At the end:

```text
PaperSpoon
├── remote UI state
├── remote layout
├── remote renderer
├── framebuffer
├── remote contact tracking
├── remote hit testing
├── semantic actions
├── Hammerspoon/macOS integration
└── future profiles/pages
```

```text
PaperPad
├── device session
├── framebuffer receiver/cache
├── X11 framebuffer blitter
├── X11 touch normalization
├── remote pointer forwarding
├── system UI
│   └── Exit
└── process/window lifecycle
```

That distinction is intentional.

PaperPad is not a completely dumb display.

It is a thin remote terminal with a small trusted local control plane.

---

# Phase 22 — Tests

`paper-protocol` should test:

- framebuffer protocol framing;
- version validation;
- message type validation;
- partial input;
- concatenated messages;
- malformed lengths;
- maximum payload limits;
- Hello;
- Frame;
- PointerDown;
- PointerUp;
- ViewportChanged;
- Mono1 payload validation.

PaperPad should test:

- wire Mono1 → X11 conversion;
- stride/padding conversion;
- chunk calculation;
- malformed frames;
- dimension mismatch;
- latest valid frame retention;
- primary touch normalization;
- auxiliary detail filtering;
- physical → remote coordinate conversion;
- touches in the Exit region are not forwarded;
- Exit Down/Up activation;
- Exit cancellation cases;
- Exit remains independent from connection state.

PaperSpoon should test:

- remote rendering;
- application geometry;
- remote hit testing;
- remote contact state;
- action dispatch;
- frame generation;
- viewport changes.

---

# Phase 23 — Physical-device gates

Require physical Paperwhite verification at these milestones:

```text
Gate A
host diagnostic framebuffer visible correctly
while local Exit remains usable

Gate B
host-rendered application grid visible correctly
with local Exit alongside it

Gate C
remote coordinates received correctly by PaperSpoon

Gate D
host hit-testing triggers all remote actions correctly
and Exit never reaches PaperSpoon

Gate E
PaperSpoon terminated/disconnected
and local Exit still closes PaperPad

Gate F
old semantic application path removed
and framebuffer-only architecture works

Gate G
disconnect/reconnect restores remote framebuffer
without affecting local Exit
```

Do not substitute host tests for physical-device verification.

---

# Phase 24 — Documentation cleanup

Document the final ownership explicitly:

```text
PaperSpoon → remote framebuffer → PaperPad
PaperPad → remote pointer events → PaperSpoon

PaperPad → local Exit → PaperPad lifecycle
```

Make it clear that the bottom/system Exit area is not part of the host framebuffer.

Document the remote viewport dimensions/insets and coordinate convention.

Do not describe PaperPad as a completely passive framebuffer viewer because it still owns system UI and lifecycle control.

---

# Definition of done

The migration is complete only when:

```text
[ ] PaperSpoon renders the remote application UI.
[ ] PaperSpoon owns remote application layout.
[ ] PaperSpoon owns remote application hit testing.
[ ] PaperSpoon owns remote contact tracking.
[ ] PaperSpoon owns host semantic actions.
[ ] PaperSpoon owns Hammerspoon dispatch.

[ ] PaperPad receives Mono1 frames.
[ ] PaperPad caches the latest valid remote frame.
[ ] PaperPad redraws cached frames on X11 Expose.
[ ] PaperPad sends normalized remote PointerDown/PointerUp coordinates.

[ ] PaperPad locally renders Exit.
[ ] Exit works with PaperSpoon connected.
[ ] Exit works with PaperSpoon disconnected.
[ ] Exit works when PaperSpoon is not running.
[ ] Exit does not produce a remote pointer event.
[ ] Exit does not depend on host framebuffer contents.
[ ] Exit remains usable after protocol/frame errors.

[ ] PaperPad contains no host application-specific grid geometry.
[ ] PaperPad contains no host application-specific semantic actions.
[ ] `display <text>` no longer exists.
[ ] `event action=<...>` no longer exists.
[ ] protocol v1 compatibility code has been removed.

[ ] framebuffer rendering is verified on the physical Paperwhite.
[ ] remote coordinate mapping is verified on the physical Paperwhite.
[ ] local Exit behavior is verified on the physical Paperwhite.
[ ] reconnect produces a fresh authoritative remote frame.
[ ] documented build/test/verification commands pass.
[ ] README and AGENTS.md describe the final architecture.
```

Do not mark the task complete while the old semantic application architecture remains active.

---

# Suggested commit sequence

```text
1. refactor(paperpad): separate local Exit from application UI
2. feat(paperpad): define remote content viewport
3. feat(protocol): add framebuffer protocol primitives
4. feat(paperpad): receive and validate mono frames
5. feat(paperpad): add X11 framebuffer blitter
6. feat(paperpad): composite remote framebuffer with local Exit
7. test(device): verify framebuffer and local Exit on Paperwhite
8. feat(paperspoon): add host-side mono renderer
9. feat(paperspoon): render existing application grid remotely
10. feat(protocol): add remote pointer coordinate events
11. feat(paperpad): forward viewport-relative pointer events
12. feat(paperspoon): move application contact tracking and hit testing to host
13. test(device): verify remote actions and local Exit isolation
14. refactor(paperspoon): own remote status and application UI state
15. refactor(paperpad): remove local application UI and semantic actions
16. refactor(protocol): remove semantic/display protocol v1
17. test(device): verify complete framebuffer architecture
18. docs: document remote viewport and local PaperPad system UI
```

---

# Important considerations

💡 **Treat Exit as trusted local system UI, not application UI.** The architecture should distinguish “PaperSpoon owns application pixels” from “PaperPad owns its lifecycle.”

💡 **Reserve screen space instead of drawing Exit over arbitrary host pixels.** A dedicated local strip gives clear rendering and pointer ownership and avoids requiring PaperSpoon to know where it may not draw.

💡 **Make Exit available before networking starts.** Startup order should never create a period where the fullscreen PaperPad window exists but cannot be closed locally.

💡 **Keep Exit processing independent from socket writes.** A stuck host connection must not make the local button unresponsive.

💡 **Use different coordinate spaces deliberately.** PaperSpoon should work in remote framebuffer coordinates; it should not need knowledge of PaperPad's local system chrome.

💡 **Retain only the minimum local hit-testing required for system controls.** Deleting the application grid does not mean deleting every rectangle test from PaperPad.

💡 **Keep the wire framebuffer independent from X11.** X11 padding, depth and bit order remain device implementation details.

💡 **Cache the remote framebuffer on PaperPad.** Expose/redraw should not require network access, while Exit should always be drawable from local state.

💡 **Do not turn Exit into a protocol command.** If the host must participate in closing PaperPad, the primary reason for keeping a local Exit control has already been lost.

💡 **Future additional local controls should face a high bar.** Exit is justified because it controls the device-side process itself; application functionality should continue moving to PaperSpoon.
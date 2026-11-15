Implement a PaperSpoon **Now Playing framebuffer spike** using the Rust `media-remote` crate and its `NowPlayingPerl` backend.

**Status (2026-09-28):** implemented and verified on the documented physical
Paperwhite 6 using PaperPad's MXCFB backend with Apple Music. The operator
reported the complete Now Playing screen working perfectly. Host validation
also covered retained-frame reconnect behavior and repeated helper cleanup.
The individual paused, track-switch, missing-artwork, and physical-reconnect
checkpoints below were not separately reported.

Repository:
`https://github.com/num13ru/rust_x11_hello`

Base branch:
`master`

Suggested branch:
`nowplaying-frame`

## Goal

Add a PaperSpoon stdin diagnostic command:

```text
frame nowplaying
```

When invoked, PaperSpoon should:

1. read the current macOS Now Playing state
2. obtain metadata and album artwork if available
3. render a static Now Playing screen for the currently connected PaperPad viewport
4. encode it as Gray8
5. send it through the existing PPFB framebuffer pipeline
6. preserve the rendered frame as the authoritative retained frame for reconnect/redraw behavior

This is a **one-shot rendering spike**.

Do not implement continuous subscriptions, automatic track updates, progress animation, or media controls in this PR.

Those should follow only after the basic MediaRemote integration is proven.

---

## Important architecture decision

Do **not** use direct `MediaRemote.framework` access from PaperSpoon.

On macOS 15.4+, direct third-party access is restricted by entitlement checks in `mediaremoted`.

Use:

```rust
media_remote::NowPlayingPerl
```

from:

```text
https://github.com/nohackjustnoobb/media-remote
```

The Perl backend already encapsulates `mediaremote-adapter`.

The mechanism is:

```text
PaperSpoon
    ↓
media_remote::NowPlayingPerl
    ↓
/usr/bin/perl
    ↓
mediaremote-adapter.pl
    ↓
MediaRemoteAdapter.framework
    ↓
MediaRemote.framework
```

The Objective-C framework is an implementation detail of the adapter.

PaperSpoon remains a Rust CLI.

Do not disable SIP.

Do not require users to disable SIP.

Do not add code injection into `mediaremoted`.

Do not call the direct `NowPlaying::new()` MediaRemote backend for this feature.

---

## 1. Add the dependency

Add `media-remote` to the PaperSpoon crate.

Use a pinned version or pinned Git revision rather than an unconstrained wildcard.

Artwork support must remain enabled because this spike explicitly needs:

```rust
NowPlayingInfo::album_cover
```

The upstream crate currently exposes:

```rust
NowPlayingPerl
NowPlayingInfo
```

and its Perl backend embeds/extracts the adapter assets and launches `/usr/bin/perl`.

Do not manually vendor a second copy of `MediaRemoteAdapter.framework` unless inspection of the selected dependency version proves that the crate does not already bundle it.

Before implementation, inspect the exact selected crate revision and confirm:

- `NowPlayingPerl` is exported
- artwork support is enabled
- the bundled adapter assets are present
- the implementation launches `/usr/bin/perl`
- no SIP-disabled path is required

Document the pinned upstream revision/version.

---

## 2. Isolate Now Playing integration

Do not put MediaRemote-specific logic directly into `main.rs`.

Create a small host-side module, for example:

```text
tools/paperspoon/src/now_playing.rs
```

Define a PaperSpoon-owned snapshot type.

Conceptually:

```rust
struct NowPlayingSnapshot {
    title: String,
    artist: Option<String>,
    album: Option<String>,
    is_playing: Option<bool>,
    elapsed_time: Option<f64>,
    duration: Option<f64>,
    bundle_id: Option<String>,
    bundle_name: Option<String>,
    artwork: Option<DynamicImage>,
}
```

The exact ownership/types may follow existing conventions.

The rest of PaperSpoon should not depend directly on `media_remote::NowPlayingInfo`.

Translate external state into PaperSpoon domain state at the integration boundary.

---

## 3. Implement one-shot snapshot acquisition

Implement something equivalent to:

```text
load_now_playing_snapshot()
```

using:

```rust
NowPlayingPerl::new()
```

Important: `NowPlayingPerl` internally starts a `stream` subprocess and receives data asynchronously.

Therefore, do not assume that:

```rust
let np = NowPlayingPerl::new();
np.get_info();
```

will immediately contain data.

Implement a bounded wait for the **first usable snapshot**.

Requirements:

- total wait must be bounded
- no infinite loops
- no permanent background thread/process should be created per invocation
- sleeping/polling at a small interval is acceptable for this first spike
- drop the `NowPlayingPerl` instance after the snapshot is captured
- return a clear error if no usable Now Playing state arrives before the deadline

Choose a conservative small deadline suitable for an interactive CLI command.

Do not add a long-lived MediaRemote service yet.

A usable snapshot should require at least a meaningful title.

Artwork is optional because the adapter documentation explicitly states that artwork can arrive later than basic metadata.

If initial metadata arrives without artwork, allow a short bounded grace period for artwork to appear before rendering.

Do not wait indefinitely for artwork.

---

## 4. Handle no-player and partial states

The command must behave cleanly when:

- nothing is playing
- playback is paused
- metadata is incomplete
- artwork is missing
- artist is missing
- album is missing
- MediaRemote adapter fails
- `/usr/bin/perl` cannot be launched
- adapter assets fail to initialize

Do not crash PaperSpoon.

For no current media, render a useful static screen such as:

```text
Nothing Playing
```

or return a clear diagnostic without replacing the existing authoritative frame.

Choose one behavior and test/document it.

For this spike, prefer **not replacing a valid existing frame on acquisition failure**.

---

## 5. Reuse the existing image pipeline

There is already host-side image handling from:

```text
tools/paperspoon/src/image_frame.rs
```

Do not duplicate resize/luminance conversion logic.

Refactor only as much as necessary so that both:

```text
JPEG path → DynamicImage → Gray8
```

and:

```text
NowPlaying artwork DynamicImage → Gray8
```

can share the same fit/resize/composition primitives.

Avoid turning this into a general image-processing rewrite.

The MediaRemote artwork should never be written to disk merely to reuse `frame jpeg`.

Keep it in memory:

```text
DynamicImage
    ↓
resize / crop / compose
    ↓
Gray8
```

---

## 6. Render a simple Now Playing layout

Do not introduce raylib yet.

Use the existing PaperSpoon software-rendering infrastructure or extend it minimally for Gray8 composition.

Target a static portrait layout approximately like:

```text
┌──────────────────────────┐
│                          │
│       album artwork      │
│                          │
│                          │
├──────────────────────────┤
│ Track title              │
│ Artist                   │
│ Album                    │
│                          │
│       ▶  /  PAUSED       │
│                          │
│ [----------      ]       │
│ 1:23              4:17   │
└──────────────────────────┘
```

This is illustrative, not a pixel-perfect requirement.

Prioritize:

- large artwork
- readable title
- readable artist
- readable album
- play/pause state
- optional static progress indication

Do not add touch controls yet.

Do not make the Now Playing screen part of the existing application button layout yet.

This command is a framebuffer diagnostic/application spike.

---

## 7. Artwork behavior

If artwork is available:

```text
NowPlayingInfo.album_cover
```

compose it directly into the Gray8 framebuffer.

Requirements:

- preserve aspect ratio
- do not stretch
- use deterministic fit behavior
- keep it inside its allocated bounds
- white background outside the image
- no disk roundtrip
- no JPEG re-encoding

If artwork is absent:

- draw a deterministic placeholder
- still render metadata

A simple outlined square or music-note placeholder is sufficient.

Do not fetch artwork from Apple Music, Spotify, or the network.

---

## 8. Text rendering

Reuse the existing font/text renderer initially.

Do not add a font engine or raylib in this PR.

If current Mono1 text primitives cannot draw into Gray8 directly, generalize the minimal relevant drawing primitive so that text can write a luminance value such as:

```text
0 = black
255 = white
```

Avoid duplicating an entire renderer solely for Now Playing.

The final framebuffer must remain Gray8.

---

## 9. Progress rendering

If both are available:

```text
elapsed_time
duration
```

render a static progress bar representing the snapshot at command execution time.

Do not update it continuously.

Clamp invalid values:

```text
0 <= elapsed <= duration
```

Ignore progress if:

- duration is zero
- duration is negative
- values are NaN/infinite
- values are otherwise invalid

Optionally render:

```text
m:ss / m:ss
```

if the current text renderer supports it cleanly.

---

## 10. Add the stdin command

Extend command parsing to support:

```text
frame nowplaying
```

It must take no additional arguments.

Reject:

```text
frame nowplaying extra
```

with clear usage text.

Existing commands must continue to work:

```text
frame jpeg <path>
frame gray-gradient ...
frame gray-bars ...
frame gray ...
frame ...
ui ...
```

---

## 11. Send through the existing framebuffer path

Do not create a special transport message for Now Playing.

The result must simply become:

```text
Gray8Frame
    ↓
encode_v2_frame
    ↓
existing Frame message
```

PaperPad must know nothing about:

```text
Now Playing
album artwork
Music
Spotify
MediaRemote
```

This information remains entirely host-side.

The Kindle still receives only framebuffer pixels.

---

## 12. Respect backend capabilities

`frame nowplaying` requires Gray8.

Before rendering/sending, verify that the connected PaperPad advertises Gray8 support.

If the connected backend is X11/Mono1-only:

- do not send a Gray8 frame
- return a clear diagnostic
- do not silently dither it to Mono1

Do not change the X11 backend in this PR.

---

## 13. Retained-frame behavior

A successfully rendered Now Playing frame should participate in the same retained-authoritative-frame mechanism as the existing JPEG frame.

After:

```text
frame nowplaying
```

a reconnect/new Hello should resend the retained Now Playing framebuffer where current semantics already require retained content to be restored.

Do not rerun MediaRemote acquisition merely because PaperPad reconnects.

The retained object should be the rendered framebuffer, not the original MediaRemote object.

---

## 14. Process lifecycle

Pay particular attention to `NowPlayingPerl`.

The current upstream implementation starts:

```text
/usr/bin/perl ... stream --no-diff
```

in a background Rust thread.

For this one-shot command:

- acquire the snapshot
- stop/drop the provider cleanly
- do not leak Perl processes
- do not accumulate background threads after repeated `frame nowplaying` commands

Add a testable abstraction around snapshot acquisition if necessary.

If upstream `Drop` behavior does not reliably terminate the child process promptly, do not silently accept leaking processes.

Investigate and document it.

If a small local wrapper is needed to own/terminate the subprocess deterministically, keep it narrowly scoped.

Do not fork/reimplement the entire adapter unless necessary.

---

## 15. Security and robustness

Treat MediaRemote/adaptor output as external input.

Do not assume:

- artwork dimensions are reasonable
- artwork allocation sizes are reasonable
- metadata strings are short
- timestamps are sane

Reuse existing image memory/size limits where possible.

Set reasonable limits before allocating very large composed images.

Do not execute any metadata as shell commands.

Do not interpolate metadata into shell command strings.

`NowPlayingPerl` should be invoked through its Rust API, not by constructing shell command lines.

---

## 16. Tests

Add unit tests without requiring an active macOS media player.

Separate:

```text
MediaRemote acquisition
```

from:

```text
NowPlaying rendering
```

so renderer tests can use deterministic fixtures.

Test at least:

1. complete track with artwork
2. complete track without artwork
3. paused state
4. missing artist
5. missing album
6. valid progress
7. invalid/zero duration
8. title too long for available layout
9. portrait viewport
10. odd viewport dimensions
11. Gray8 output dimensions/stride
12. command parser accepts exactly `frame nowplaying`
13. extra arguments are rejected
14. successful frame becomes retained content
15. acquisition failure does not destroy previously retained content
16. Gray8 capability is required

Do not make CI depend on `mediaremoted` returning real data.

---

## 17. Manual spike validation on macOS

Before integrating rendering deeply, validate the provider independently.

On the development Mac:

- start playback in Music
- instantiate `NowPlayingPerl`
- confirm title
- confirm artist
- confirm album
- confirm playing state
- confirm duration/elapsed time
- confirm `album_cover` eventually appears
- print artwork dimensions, not artwork bytes
- repeat with paused playback
- change tracks
- test with another Now Playing source if convenient

Most importantly verify:

```text
SIP remains enabled
```

and PaperSpoon receives data through the Perl adapter.

Do not claim direct MediaRemote access.

Record the macOS version used for this validation.

---

## 18. Physical Kindle validation

With PaperPad using the MXCFB backend:

1. start media playback
2. run:

```text
frame nowplaying
```

3. verify:
   - artwork appears in grayscale
   - black/white polarity is correct
   - title is readable
   - artist/album are readable
   - progress is plausible
   - Exit strip remains intact
   - no content crosses into device-local UI
4. pause playback and run the command again
5. switch track and run it again
6. test a track without artwork if available
7. disconnect/reconnect PaperPad and verify retained-frame behavior

Do not add automatic updates during this validation.

### Recorded physical result (2026-09-28)

The primary physical path passed on the documented Paperwhite 6:

```text
Apple Music
→ media_remote::NowPlayingPerl
→ in-memory Gray8 composition
→ PPFB v2 Frame
→ PaperPad MXCFB
→ Kindle display
```

The operator reported the resulting Now Playing screen working perfectly on
the device. This is direct operator validation of the end-to-end display path;
no separate device log, deployed-binary checksum, photograph, or per-checkpoint
results were captured for this run. Host smoke validation separately proved a
matching reconnect reused retained pixels and repeated one-shot acquisitions
left no helper or `mediaremote-adapter.pl` process running.

---

## Definition of done

The PR is complete when:

- PaperSpoon builds and remains a Rust CLI.
- SIP does not need to be disabled.
- `NowPlayingPerl`/mediaremote-adapter is used instead of direct MediaRemote access.
- `frame nowplaying` obtains the current system Now Playing state.
- metadata is rendered into a Gray8 framebuffer.
- album artwork is rendered directly from memory when available.
- no artwork disk roundtrip is required.
- no AppleScript is used by PaperSpoon for this path.
- no Hammerspoon is required for this path.
- PaperPad remains unaware of media semantics.
- only existing PPFB Gray8 frames cross the wire.
- missing artwork/metadata is handled gracefully.
- repeated invocations do not leak Perl processes/threads.
- existing Mono1, Gray8, JPEG, application, networking, and MXCFB behavior remains intact.
- repository checks pass.
- the screen is verified on the physical Paperwhite 6.

## Out of scope

Do not implement yet:

- automatic subscription-driven redraw
- background Now Playing service
- playback buttons
- next/previous/play/pause dispatch
- seek interaction
- progress animation
- per-second framebuffer updates
- Hammerspoon integration for Now Playing
- direct MediaRemote.framework access
- AppleScript fallback
- JXA fallback
- raylib
- Gray4
- waveform heuristics
- network artwork lookup

## Follow-up after this PR

If the spike is successful, the next PR should convert the one-shot provider into a long-lived PaperSpoon `NowPlayingService`:

```text
MediaRemote stream
      ↓
NowPlayingState
      ↓
state changes only
      ↓
PaperSpoon render
      ↓
Gray8 frame
```

Then map PaperPad touch actions to:

```text
play/pause
next
previous
```

using the same MediaRemote backend.

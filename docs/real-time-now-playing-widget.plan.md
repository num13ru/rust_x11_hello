
# Implementation Plan: Real-Time Now Playing Widget

## 1. Objective

Extend the existing static Now Playing renderer into a persistent, interactive, e-ink-optimized music controller.

The widget must support:

- [ ] Live track title, artist, and album metadata.
- [ ] Album artwork.
- [ ] Playback progress bar.
- [ ] Elapsed and remaining playback time.
- [ ] Previous track.
- [ ] Play/pause.
- [ ] Next track.
- [ ] Seeking through the progress bar.
- [ ] Automatic synchronization with external playback changes.
- [ ] Interactive development and testing through PaperView.
- [ ] E-ink-aware rendering and refresh scheduling.

Preserve the existing Now Playing visual design wherever possible.

## 2. Repository context

**Target repository:**

- https://github.com/num13ru/rust_x11_hello

**Reference implementation:**

- https://github.com/num13ru/floating-fruit
- Local checkout: `/Users/user/GIT/floating-fruit`

Before implementation:

- Read `AGENTS.md` and `README.md`.
- Inspect the existing Now Playing acquisition and rendering implementation.
- Inspect PaperSpoon's event loop, frame publication, connection lifecycle, and input handling.
- Inspect PaperPad's framebuffer comparison, MXCFB update scheduling, and waveform selection.
- Inspect PaperView's interactive input implementation.
- Inspect Floating Fruit's `src/music.rs`, `src/app.rs`, `src/ui.rs`, and `src/texture.rs`.

Floating Fruit already implements AppleScript-based playback control, position tracking, artwork acquisition, caching, and seeking.

Reuse the relevant approaches without introducing egui, eframe, or GPU rendering dependencies into PaperSpoon.

Treat the local Floating Fruit checkout as a reference, not a required build dependency.

## 3. Architectural constraints

Preserve the existing host-rendered framebuffer architecture.

```text
macOS media provider
        │
        ▼
Playback state acquisition
        │
        ▼
Now Playing controller
        │
        ├── Playback clock
        ├── Artwork cache
        ├── Refresh scheduler
        └── Input handling
        │
        ▼
PaperSpoon renderer
        │
        ▼
Existing PPFB v2 transport
        │
        ├── PaperView
        │
        └── PaperPad → MXCFB
```

PaperSpoon must own:

- Media acquisition and playback commands.
- Application state.
- Playback position interpolation.
- Rendering and layout.
- Hit testing and interaction state.
- Frame publication scheduling.

PaperPad must remain unaware of music, tracks, progress bars, and playback commands.

PaperView must continue displaying the same framebuffer received by PaperPad.

Do not introduce:

- A separate music-specific network protocol.
- Application-specific geometry on the Kindle.
- A second renderer for PaperView.
- A dependency on X11.
- A continuously running 60 FPS render loop.
- A mandatory dependency on a sibling repository.

## 4. Media provider

### 4.1 Audit existing implementations

The current static Now Playing implementation uses `media-remote 0.5.2` with the `NowPlayingPerl` backend.

Floating Fruit communicates with Apple Music through AppleScript and `osascript`.

Determine whether the existing media-remote backend can reliably provide:

- Persistent subscriptions.
- Playback state.
- Source application identity.
- Playback position and duration.
- Playback rate.
- Track identity.
- Artwork.
- Playback commands and seeking.
- Recovery after helper failure.

Verify these capabilities with the pinned library version and current macOS version. Do not assume that APIs exposed by other media-remote backends work through `NowPlayingPerl`.

### 4.2 Select a consistent media source

For the initial implementation, prefer Apple Music as the explicitly supported interactive player unless the existing media-remote backend proves sufficient for both acquisition and control.

Metadata and playback commands must target the same media provider.

Never display metadata from Spotify, Safari, or another system player while silently sending playback commands to Music.app.

Preserve the existing one-shot `frame nowplaying` behavior.

Record the provider decision and its limitations in the design documentation.

### 4.3 Provider abstraction

Introduce a small host-side abstraction for:

- Acquiring playback snapshots.
- Receiving playback changes, where supported.
- Executing playback commands.
- Acquiring artwork.
- Reporting provider capabilities and errors.

Provide a deterministic fake provider for automated tests and PaperView development.

Keep platform-specific dependencies outside PaperPad and the shared protocol crate.

## 5. Playback state model

Represent playback state explicitly instead of relying on independent boolean flags.

Suggested conceptual states:

- `Unavailable`
- `Stopped`
- `Playing`
- `Paused`
- `Seeking` — transient, if reported by the provider.
- `Stale` — the last valid state exists, but acquisition has failed.

A playback snapshot should contain, where available:

- Provider and source identity.
- Stable track identity.
- Title.
- Artist.
- Album.
- Artwork identity.
- Duration.
- Sampled playback position.
- Playback rate.
- Monotonic acquisition timestamp.
- Playback capabilities.

Keep pending commands and pointer interaction state separate from authoritative playback state.

Distinguish missing metadata from an unavailable player or failed acquisition.

Do not use track title alone as track identity.

## 6. Playback clock

Do not query the operating system every time the progress bar is rendered.

Maintain a local playback clock based on the last authoritative position sample.

```text
While playing:

estimated position =
    sampled position
    + elapsed monotonic time × playback rate

While paused:

estimated position = sampled position
```

Clamp the estimated position to the known track duration.

Use a monotonic clock rather than wall-clock time.

Resynchronize when:

- The track changes.
- Playback starts or pauses.
- The user seeks.
- Playback position changes externally.
- Playback rate changes.
- A new authoritative position sample arrives.
- The host resumes from sleep.

Prevent indefinite extrapolation when the media provider becomes unavailable.

Handle:

- Unknown duration.
- Zero duration.
- Live streams.
- Invalid or non-finite position values.
- Unexpected backward position changes.
- Track completion.
- Repeat mode.

Test clock behavior using a controllable test clock rather than real-time sleeps.

## 7. Event-driven refresh scheduler

Separate three independent concerns:

1. Media state acquisition.
2. Frame rendering and publication.
3. Physical e-ink updates.

These operations must not share a single unconditional timer.

### Default scheduling policy

| Operation | Initial policy |
|---|---|
| Media notifications | Event-driven |
| Active playback polling fallback | 1000 ms |
| Paused polling fallback | 2000 ms |
| Unavailable player polling | Backoff, up to 5000 ms |
| Progress bar | 1000 ms |
| Elapsed and remaining time | 1000 ms |
| Metadata | On change |
| Artwork | On change |
| Playback controls | On change |
| Touch feedback | Immediate request |
| Seek preview | Optional, up to 10 Hz |
| Idle framebuffer publication | None |

Polling intervals are initial configuration values, not hard-coded architectural assumptions.

Prefer subscriptions where they are reliable. Retain bounded periodic reconciliation when required.

Use one in-flight acquisition request per provider. Do not accumulate polling tasks when a provider is slow.

Coalesce simultaneous changes into a single rendering operation.

Skip publication when the resulting framebuffer has not changed.

While paused or stopped, do not publish periodic frames unless visible content changes.

### Refresh interval configuration

Support at least the following experimental progress update intervals:

- 100 ms.
- 250 ms.
- 500 ms.
- 1000 ms — default.
- 2000 ms.

A 100 ms interval must remain opt-in and must not imply that the physical panel can sustain 10 visible updates per second.

## 8. E-ink-aware rendering

Preserve Gray8 support for the existing Now Playing design.

Divide the rendered widget into logical regions.

| Region | Update frequency |
|---|---|
| Album artwork | Infrequent |
| Track metadata | Infrequent |
| Background and separators | Static |
| Progress bar | Frequent |
| Playback time | Frequent |
| Playback controls | Event-driven |

### 8.1 Static content

- Cache decoded, resized grayscale artwork.
- Reuse the existing typography and image-rendering infrastructure.
- Preserve antialiased metadata rendering.
- Avoid scrolling marquees.
- Use wrapping or ellipsis for long metadata.
- Keep layout stable when playback state changes.

### 8.2 Dynamic content

- Prefer high-contrast black-and-white pixels for frequently updated elements.
- Use fixed-width timer regions and stable digit alignment.
- Quantize progress to physical pixel positions.
- Avoid decorative animations, fading, shadows, and transitions.
- Avoid clearing and redrawing the entire widget when only the playback position changes.

A progress update that does not change any visible pixels must not produce a new frame.

### 8.3 Framebuffer caching

Maintain cached static rendering results where beneficial.

Compose dynamic regions over the static content.

Ensure that shrinking progress bars, changing digits, and seeking backwards correctly restore underlying pixels.

Distinguish renderer invalidation regions from actual physical display update rectangles.

## 9. MXCFB refresh policy

Treat pixel format and physical update waveform as independent concerns.

The initial live widget must work with the existing display backend and conservative GC16 mode.

Do not assume that a fast waveform preserves grayscale artwork.

### Phase A: Existing transport and display behavior

- Preserve PPFB v2.
- Publish complete valid framebuffers when required.
- Reuse the existing framebuffer comparison and dirty-region mechanisms where available.
- Measure physical refresh behavior before changing the protocol or display backend.

### Phase B: Selective waveform experiment

If physical measurements show that GC16 produces unacceptable flashing or latency, investigate content-aware waveform selection.

Proposed policy:

| Content | Candidate waveform |
|---|---|
| Album artwork | GC16 |
| Antialiased metadata | GC16 |
| Binary progress bar | A2 or DU |
| Binary playback timer | A2 or DU |
| Binary button feedback | A2 or DU |
| Grayscale restoration | GC16 |

These are experimental candidates, not guaranteed compatible modes.

Keep waveform decisions generic to the display backend. PaperPad must not know which rectangle represents a music control.

Mixed-waveform updates must be verified for alignment constraints, completion ordering, grayscale preservation, and overlapping regions.

Fall back to GC16 when fast updates cannot safely preserve the intended output.

### Phase C: Ghosting maintenance

Investigate a configurable cleanup policy based on:

- Number of partial updates.
- Elapsed time.
- Accumulated changed area.
- Physical observations.

Do not introduce an arbitrary mandatory full-screen refresh every few seconds.

Document the measured quality and latency trade-offs.

## 10. Playback controls

Reuse the existing viewport-relative pointer input and hit-testing architecture.

Provide the following semantic actions:

- Previous track.
- Play/pause.
- Next track.
- Seek to position.

For normal buttons:

```text
Idle
  → PointerDown inside control
  → Armed
  → PointerUp inside the same control
  → Dispatch command
  → Await authoritative playback state
  → Idle
```

Cancel the interaction when:

- The pointer is released outside the control.
- The active connection changes.
- The viewport changes.
- The widget is deactivated.
- The underlying track or applicable control state becomes invalid.

Do not execute commands directly from the renderer.

Execute media commands outside the network and rendering hot paths.

Use bounded command execution time and explicit error handling.

Request an immediate state reconciliation after command completion.

Do not interpret successful command dispatch as proof that playback actually changed.

Avoid duplicate dispatch through both Hammerspoon and the native media provider.

## 11. Seekable progress bar

Implement tap-to-seek as the initial interaction.

### Initial behavior

- Pointer down inside the progress bar arms the interaction.
- Pointer release resolves the requested position.
- Clamp the requested position to the valid duration.
- Dispatch one seek command.
- Reconcile the resulting playback position.

Disable seeking when the media provider does not support it or the duration is unknown.

Associate each seek interaction with the track identity captured when the interaction began.

Discard the interaction if the track changes before release.

### Optional drag-to-seek

Implement only after tap-to-seek is stable.

Inspect whether the existing native input and PPFB implementation forwards sufficient movement events.

During dragging:

- Update a local preview position.
- Limit preview rendering to the configured refresh interval.
- Do not issue an AppleScript command for every pointer movement.
- Commit one seek operation on release.
- Cancel the preview on disconnect or invalidation.

Prevent periodic playback samples from overwriting the active seek preview.

Do not expand the protocol solely for drag support unless the existing input model is demonstrably insufficient.

## 12. Artwork pipeline

Reuse the existing image decoder and Gray8 rendering infrastructure.

Adapt relevant caching and stale-result protection patterns from Floating Fruit.

Requirements:

- Load artwork asynchronously.
- Decode and resize each artwork only when necessary.
- Cache processed grayscale images.
- Bound the cache by memory usage or entry count.
- Display a placeholder when artwork is unavailable.
- Clean up temporary extraction files.
- Reject artwork results associated with an outdated track or provider.

Ensure that rapid track changes cannot cause an older artwork request to overwrite the current artwork.

Do not use GPU textures as the artwork cache representation.

Keep image decoding limits in place for malformed or unexpectedly large artwork.

## 13. Runtime integration

Introduce a persistent Now Playing application mode.

Suggested commands, subject to the existing command parser conventions:

```text
nowplaying start
nowplaying stop
nowplaying status
nowplaying refresh
```

Preserve:

```text
frame nowplaying
```

The existing command must remain a one-shot diagnostic snapshot.

When persistent mode starts:

1. Acquire an initial playback snapshot.
2. Initialize the playback clock.
3. Render the initial widget.
4. Start media observation and scheduled updates.
5. Enable Now Playing hit testing.

When another application mode replaces Now Playing:

1. Cancel active pointer interactions.
2. Stop unnecessary media observation and scheduled updates.
3. Invalidate pending asynchronous results.
4. Release mode-specific resources.

Define the behavior of diagnostic frame commands while persistent mode is active. They must not be immediately overwritten by an unrelated background update.

On client reconnect, render or publish the latest valid state for the negotiated viewport.

Do not replay playback commands or pointer interactions from the previous connection.

Stop unnecessary frame production when no compatible client is connected.

Preserve the existing PaperPad event-driven runtime.

## 14. Backpressure and asynchronous execution

A slow e-ink client must not cause an unbounded frame queue.

Use the existing latest-frame or coalescing mechanisms where available.

Maintain:

- Bounded pending work.
- One authoritative current playback state.
- One current desired framebuffer.
- Explicit ownership of active media requests.
- Ordered playback commands.
- Cancellation or invalidation of outdated results.

Keep the latest desired frame separate from a successfully transmitted or displayed frame.

If a frame is superseded before transmission, prefer the newer state.

Do not drop semantic playback commands merely because intermediate visual feedback frames can be discarded.

Prevent artwork extraction, AppleScript execution, or a stalled media helper from blocking pointer processing.

Ensure subprocesses are terminated and reaped on timeout or shutdown.

## 15. PaperView development workflow

### Terminal 1 — PaperSpoon

```sh
cargo run -p paperspoon -- \
  5581 \
  /private/tmp/paperspoon-nowplaying.log \
  --no-forward-url
```

### Terminal 2 — PaperView

```sh
cargo run -p paperview -- \
  --host 127.0.0.1 \
  --port 5581 \
  --viewport 1272x1624 \
  --interactive \
  --save-dir /private/tmp
```

Start PaperView without a connected PaperPad session, since PaperSpoon currently serves one active PPFB client.

Add an explicit fake-provider development option to PaperSpoon.

The fake provider must be able to simulate:

- Playing and paused states.
- Multiple tracks.
- Artwork changes.
- Long metadata.
- External seeking.
- Rapid track changes.
- Unknown duration.
- Provider failure and recovery.
- Slow commands.
- Delayed artwork responses.

The fake provider must never execute real macOS playback commands.

Note that `--no-forward-url` disables Hammerspoon URL forwarding; it does not automatically prevent direct AppleScript playback commands. Provide a separate explicit mechanism for enabling real media control.

Test against real Apple Music only after deterministic fake-provider behavior is verified.

### Optional PaperView diagnostics

Expose lightweight diagnostic information without changing the source framebuffer:

- Received frames per second.
- Time since the last frame.
- Frame payload size.
- Changed pixel count.
- Dirty-region visualization.
- Simulated display refresh latency.

Diagnostic overlays must remain local to PaperView and must not appear in exported source-frame images.

Do not claim that PaperView simulates physical ghosting or waveform fidelity.

## 16. Automated tests

### Playback state and clock

- [ ] Position advances while playing.
- [ ] Position remains stable while paused.
- [ ] Position clamps to the track duration.
- [ ] External seeking resynchronizes the clock.
- [ ] Track changes reset the playback anchor.
- [ ] Unknown durations do not cause division by zero.
- [ ] Invalid numeric values are rejected.
- [ ] Provider failures stop indefinite extrapolation.

### Refresh scheduler

- [ ] No periodic frame publication while paused.
- [ ] Identical framebuffers are not republished.
- [ ] Simultaneous changes are coalesced.
- [ ] Slow acquisitions do not create overlapping requests.
- [ ] A slow client does not create an unbounded frame queue.
- [ ] Inactive application modes stop generating frames.

### Rendering

- [ ] Progress changes do not alter static artwork pixels.
- [ ] Timer changes do not move adjacent layout elements.
- [ ] Backwards seeking correctly restores pixels.
- [ ] Missing artwork renders a placeholder.
- [ ] Long metadata remains inside its layout bounds.
- [ ] Framebuffer dimensions match the negotiated viewport.

### Interaction

- [ ] One valid button release dispatches one command.
- [ ] Release outside the control cancels the action.
- [ ] Unmatched pointer releases do nothing.
- [ ] Seeking clamps to valid bounds.
- [ ] Track changes invalidate pending seek interactions.
- [ ] Reconnection cancels old pointer interactions.
- [ ] A failed command does not fabricate a successful playback state.

### Lifecycle

- [ ] Stale artwork responses are discarded.
- [ ] Late media samples cannot reactivate a stopped widget.
- [ ] Helper processes are cleaned up.
- [ ] Provider recovery restores live updates.
- [ ] Existing diagnostic frame commands remain functional.
- [ ] Existing application-grid interactions remain functional.

## 17. Performance instrumentation

Extend existing logging and latency instrumentation where appropriate.

Measure independently:

- Media acquisition duration.
- Media command execution duration.
- Command-to-observed-playback-change latency.
- Artwork extraction and decoding duration.
- Rendering duration.
- Frame publication frequency.
- Frame payload bytes.
- Changed pixel count.
- Dirty-region area.
- MXCFB submission duration.
- MXCFB completion duration.
- Dropped or coalesced frames.

Compare progress refresh intervals:

| Interval | Requested update rate |
|---|---:|
| 100 ms | 10 Hz |
| 250 ms | 4 Hz |
| 500 ms | 2 Hz |
| 1000 ms | 1 Hz |
| 2000 ms | 0.5 Hz |

Run representative tests for:

- Continuous playback.
- Paused playback.
- Rapid track switching.
- Repeated seeking.
- Network interruption.
- Slow media-provider responses.

Do not confuse host rendering throughput, network frame rate, MXCFB completion rate, and visually observed e-ink refresh rate.

Keep raw performance logs outside Git.

## 18. Physical Kindle validation

After PaperView validation, test on the Kindle Paperwhite 6.

Verify:

- [ ] Timer readability during continuous playback.
- [ ] Progress bar movement without unnecessary screen flashing.
- [ ] Stable grayscale artwork during progress updates.
- [ ] Playback button responsiveness.
- [ ] Accurate seeking.
- [ ] Ghosting after extended playback.
- [ ] Correct behavior during pause and resume.
- [ ] Recovery after network disconnection.
- [ ] Local Exit remains responsive.
- [ ] Framebuffer restoration on exit.
- [ ] No leaked helper processes or accumulating work.

Compare GC16 against any experimental partial-update policy.

Use the existing benchmark methodology and deployment instructions in `AGENTS.md`.

Do not claim physical display compatibility based solely on PaperView or successful framebuffer IOCTL calls.

## 19. Implementation milestones

### M1 — Media integration

- [ ] Audit both repositories.
- [ ] Select the initial media provider.
- [ ] Implement playback snapshots and explicit playback state.
- [ ] Implement a deterministic fake provider.
- [ ] Implement bounded acquisition and playback commands.
- [ ] Add playback clock tests.

### M2 — Live Now Playing

- [ ] Introduce persistent application mode.
- [ ] Reuse the existing Now Playing renderer.
- [ ] Implement one-second progress updates.
- [ ] Implement metadata and artwork invalidation.
- [ ] Suppress unchanged frames.
- [ ] Verify live rendering through PaperView.

### M3 — Interactive controls

- [ ] Implement playback control hit testing.
- [ ] Implement asynchronous playback commands.
- [ ] Implement tap-to-seek.
- [ ] Verify external playback synchronization.
- [ ] Test through interactive PaperView.

### M4 — E-ink optimization

- [ ] Audit dirty-region behavior.
- [ ] Measure existing display performance.
- [ ] Optimize dynamic rendering regions.
- [ ] Evaluate faster progress update intervals.
- [ ] Investigate mixed-waveform updates only if required.
- [ ] Validate on physical Kindle.

### M5 — Hardening and documentation

- [ ] Complete lifecycle and failure-path tests.
- [ ] Verify backpressure and resource cleanup.
- [ ] Document provider limitations.
- [ ] Document refresh policy and configuration.
- [ ] Update PaperView development instructions.
- [ ] Record compact physical validation evidence.

## 20. Verification commands

Use the repository's existing Rust toolchain and development environment.

Run:

```sh
cargo fmt --all -- --check
cargo check -p paperspoon -p paperview
cargo test -p paperspoon -p paperview
make check
git diff --check
```

When using Nix, execute the relevant commands inside `nix develop`.

For PaperPad-affecting changes, additionally run the existing Kindle build and verification workflow:

```sh
make build
make verify
```

Follow the deployment scope and verification rules from `AGENTS.md`.

Avoid unrelated toolchain upgrades, dependency changes, architectural refactoring, or protocol revisions.

## 21. Additional design considerations

- 💡 **Adaptive progress updates:** investigate choosing the next render deadline based on when the displayed progress actually changes by one physical pixel.
- 💡 **Time representation:** allow switching between elapsed time, remaining time, and total duration without introducing another rendering loop.
- 💡 **Shared media library:** consider extracting reusable media-provider logic from Floating Fruit into an independent crate later, once both consumers demonstrate stable requirements.
- 💡 **Power consumption:** measure sustained playback over Wi-Fi with one-second updates versus two-second updates, including host CPU usage and Kindle battery consumption.
- 💡 **Future media providers:** keep the provider boundary suitable for Spotify or system-wide Now Playing support without implementing those integrations in this milestone.
- 💡 **Transport optimization:** evaluate rectangular framebuffer updates only if measurements show that complete Gray8 frame transmission is a material bottleneck.

## 22. Definition of done

The initial release is complete when:

- PaperSpoon maintains a persistent interactive Now Playing widget.
- Playback information automatically follows the selected media source.
- Progress and playback time update once per second by default.
- Artwork and metadata update independently of playback progress.
- Playback controls and seeking work through PaperView and PaperPad.
- Paused playback does not generate unnecessary frames.
- Slow operations do not block input processing.
- Connection and provider failures recover without stale interactions.
- Existing PPFB behavior and diagnostic commands remain compatible.
- Physical e-ink behavior has been measured and documented.

Prioritize correctness, predictable lifecycle behavior, and visual stability over maximizing refresh rate.

A stable 1 Hz Now Playing widget is the baseline. Higher refresh rates are an optimization experiment, not a release requirement.

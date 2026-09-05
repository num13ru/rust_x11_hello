# Refactoring plan

Goal: a clean, maintainable Rust project with clear ownership, deterministic
tests, and preserved Kindle behavior.

## Assumptions and scope

- Keep the existing UI, semantic action IDs, wire formats, environment variable
  names, KUAL actions, binary name, and installation paths.
- Include the Kindle app and `tools/paperspoon` in the Rust cleanup.
- Preserve the existing separation between pure UI logic and X11 types.
- Use small, reviewable changes. Separate code movement from behavior fixes.
- Keep synchronous Rust and standard-library networking initially. An async
  runtime, plugin framework, or separate crate for every module is unnecessary.
- Keep device deployment and launcher safety rules intact. This plan does not
  require deploying anything during the structural refactor.

## What the current code shows

The Kindle source is approximately 2,200 lines, including tests. `main.rs` is
already small, and `ui/` already isolates geometry and contact tracking.
The biggest opportunity is clarifying responsibilities, rather than rewriting
the application.

| Area | Current issue | Intended result |
| --- | --- | --- |
| `src/x11/events.rs` | Combines event translation, app state, drawing, logging, action dispatch, and networking | A small event adapter and coordinator, with rendering and app decisions separated |
| `src/net/mod.rs` | Reads environment, resolves endpoints, owns sockets, starts readers, parses commands, and implements reconnection | Explicit configuration and connection lifecycle, with bounded operations |
| Discovery | Wire constants and parsing are repeated in the app and companion | One shared, std-only protocol crate |
| Build checks | Root `make check` does not check the companion | One workspace-wide quality gate and an explicit Kindle build target |
| Tests | Network tests mutate process-global environment; UDP client uses a fixed test port | Injected configuration, ephemeral test ports, and explicit deadlines |
| Documentation | Some comments describe absent transport or claim serial tests and bounded I/O inaccurately | Documentation matching actual behavior and verified limits |

Behavior issues found by source inspection should get focused regression tests
and separate fixes:

- A received `display` command cannot wake `wait_for_event()`, so an idle UI may
  not show it until another X11 event arrives.
- Reader disconnects and write errors do not clear the stored TCP stream;
  reconnect is conditional on that stream being absent.
- Discovery, name resolution, and writes can execute on the UI thread. TCP
  connect has a timeout, but name resolution and writes have no explicit bound.
- Incoming lines and the reader channel are unbounded. Status text goes directly
  to X11 `image_text8`, requiring an explicit length and encoding policy.
- PaperSpoon advertises TCP port 5581 even when its CLI selects another port.
- The discovery client accepts the first valid response, so its live path cannot
  detect the multiple-responder ambiguity covered by the pure selection tests.

These are inspection findings, not claims of reproduction on the Kindle.

## Target structure

Keep the Kindle package at the repository root to minimize build/deploy churn.
Use a Cargo workspace containing that package, the existing companion, and one
shared protocol crate:

```text
Cargo.toml                     # Workspace and existing Kindle package
Cargo.lock                     # Shared lockfile
src/
  main.rs                      # Entry point and top-level error reporting
  lib.rs                       # Module root; narrow public surface
  app.rs                       # State, input decisions, and orchestration
  config.rs                    # Read environment once; parse explicit values
  ui/                          # Geometry, contact state, button/action mapping
  x11/
    mod.rs
    display.rs                 # Window/GC creation and explicit cleanup
    events.rs                  # X11 event translation and scheduling
    render.rs                  # X11 drawing and status text handling
  net/
    mod.rs                     # Small transport-facing API
    connection.rs              # Socket/reader ownership and state transitions
    discover.rs                # UDP I/O and discovery deadlines
crates/paper-protocol/
  Cargo.toml
  src/lib.rs                   # TCP/UDP formats, parsing, shared ports
tools/paperspoon/
  Cargo.toml
  src/
    main.rs                    # Startup and error reporting
    config.rs                  # CLI configuration
    server.rs                  # Connections, logging, stdin forwarding
    discovery.rs               # UDP responder using the shared protocol
```

Dependency direction: UI logic has no X11 or socket dependencies; app decisions
use UI types; X11 and networking adapt external events into application inputs.
The protocol crate depends only on `std` and knows nothing about UI geometry or
X11. Prefer concrete structs and enums; add an interface only where a real
testing or ownership boundary needs it. Keep module internals private by default.

## Implementation sequence

1. **Establish deterministic checks.** Capture existing behavior in tests; replace
   environment mutation with explicit configuration inputs and allow test UDP
   sockets to bind ephemeral ports. Give socket tests deadlines and guaranteed
   worker cleanup. Add companion formatting, Clippy, and tests to `make check`.
   Exit criterion: checks cover both programs and work under normal parallel
   Rust test execution without environment or fixed-port collisions.

2. **Unify the Rust build and protocol.** Introduce the workspace and shared
   protocol crate, moving existing wire behavior without silently tightening
   parsers. Use one lockfile and explicit package selection in the ARM build
   script so the companion is not cross-built or packaged accidentally.
   Exit criterion: both programs use the same wire definitions; host checks and
   `make build && make verify` pass with unchanged artifact paths.

3. **Separate app decisions from X11.** Extract rendering, introduce a small
   application state containing geometry, contact, and status, and translate
   press/release events through one path. Make redraw, send-action, and exit
   decisions explicit and independently testable. Preserve structured diagnostic
   lines and explicit teardown error reporting. Avoid a generic event framework.
   Exit criterion: existing geometry and contact cases pass, including resize,
   unmap, auxiliary buttons, unmatched release, and local Exit.

4. **Make transport ownership explicit.** Give connection state one owner, with
   well-defined connect, disconnect, reconnect, and shutdown transitions. Ensure
   reader shutdown also closes its cloned socket and does not leak threads.
   Move blocking transport work off the X11 thread and coordinate X11/network
   readiness, or use a short bounded poll interval if that is simpler on the
   target. Bound writes, inbound frames, and queues; define queue-full behavior.
   Avoid automatic replay after an uncertain partial write, since duplicate
   actions have user-visible effects. Evaluate resolver cancellation separately:
   moving DNS to a worker alone does not give DNS a timeout.
   Exit criterion: focused tests cover disconnect/reconnect, a stalled peer,
   idle display delivery, malformed/oversized input, and prompt UI exit.

5. **Clean up the companion and settle protocol edge cases.** Separate startup,
   connection handling, and discovery; advertise the actual bound TCP port.
   Surface discovery worker failures consistently, handle read/write failures
   explicitly, and test stdin forwarding after a connection replacement. Choose
   and document discovery behavior for multiple responders; implement it in a
   dedicated change rather than preserving unreachable ambiguity handling.
   Likewise specify invalid configuration, accepted display prefixes, and status
   text length/encoding before changing their behavior.
   Exit criterion: loopback tests exercise the real client/responder pair and
   reconnect/control flow, including failure paths.

6. **Document and verify on the target.** Add concise module/ownership guidance,
   update stale comments and README claims, and document the complete check
   command. Follow the repository deployment procedure for a device evidence run.
   Exercise taps, cancellation, resize if available, Exit, idle display updates,
   companion absence, reconnection, and launcher cleanup.
   Exit criterion: host gates pass and device claims have checksum-matched logs
   and visual confirmation where rendering is involved.

## Verification baseline and operating conditions

On 2026-09-05, `make check` passed after allowing local socket access. The initial
sandbox run passed 28 tests and could not bind sockets for two network tests.
The companion's two tests and Clippy also passed separately. No source code was
changed for this plan, and no ARM build or physical-device validation was run.

Host checks require Rust, Bash, and jq; network integration tests require local
socket access. ARM build/static verification requires Docker. Device operation
requires the compatible jailbroken ARMv7 Kindle, working KUAL/X11 environment,
and the canonical extension installation. Discovery additionally requires LAN
broadcast/unicast reachability and the launcher's temporary UDP firewall rule;
the explicit companion-host override remains the recovery path.

Before any device update, run `make check && make build && make verify`, confirm
the app is stopped, and follow the backup/readback rules in `AGENTS.md`. Match
the deployed binary SHA-256 to the host artifact before trusting run evidence.
Host tests cannot establish Kindle touchscreen or e-ink rendering correctness.

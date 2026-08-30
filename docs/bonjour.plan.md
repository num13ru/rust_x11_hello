# Codex Plan: Replace Fragile UDP Discovery and Rename Companion to PaperSpoon

## Goal

Improve the current `rust_x11_hello` prototype so the Kindle can locate the macOS companion without relying on:

- a hard-coded Mac IP address;
- the currently unreliable custom UDP discovery mechanism.

At the same time, rename the vague `companion` terminology to **PaperSpoon**.

Current repository remains:

```text
num13ru/rust_x11_hello
```

Do **not** split the project into `paperpad` / `paperspoon` repositories yet.

For now:

```text
rust_x11_hello
├── Kindle prototype
└── tools/paperspoon
```

The repository split should happen only after discovery and the end-to-end control path are proven reliable.

---

# Current verified state

Treat the following as established evidence from the physical Paperwhite 6 test on 2026-08-30.

## Working

Explicit host configuration works:

```text
RUST_X11_HELLO_COMPANION=192.168.0.12
```

With an explicit host, the Kindle reliably completes:

```text
TCP connect
→ versioned handshake
→ ACK
→ semantic actions
→ display commands
```

Therefore:

**TCP transport and the application protocol are not the current problem.**

Do not redesign them as part of this task.

## Not working

The current custom UDP discovery mechanism has been tested.

PaperSpoon demonstrably sends:

- unicast discovery offers;
- limited-broadcast discovery offers.

The Kindle nevertheless logs:

```text
no valid companion discovery offer received
```

No checksum-matched valid offer has been observed in the Kindle device log.

Therefore:

**Do not assume the current UDP discovery works on this device/network.**

## Problem

The explicit host workaround:

```text
192.168.0.12
```

is operational but fragile because the Mac address may change due to DHCP, network switching, etc.

The objective is therefore:

```text
dynamic PaperSpoon discovery
        ↓
resolved host + port
        ↓
existing TCP transport
```

---

# Naming change

Rename all user-facing and internal occurrences of the vague term:

```text
companion
```

to:

```text
PaperSpoon
```

where it refers specifically to the macOS side of this project.

## Filesystem

Rename:

```text
tools/companion
```

to:

```text
tools/paperspoon
```

## Rust package / binary

Rename the crate and resulting binary from generic names such as:

```text
companion
```

to:

```text
paperspoon
```

Expected usage should become approximately:

```text
cd tools/paperspoon
cargo build --release
./target/release/paperspoon
```

## Logs

Prefer:

```text
paperspoon.log
```

instead of:

```text
companion.log
```

## Terminology

Prefer:

```text
PaperSpoon
PaperSpoon server
PaperSpoon endpoint
PaperSpoon discovery
PaperSpoon protocol peer
```

Avoid new identifiers named only:

```text
companion
server
host
peer
```

when a PaperSpoon-specific name would be clearer.

Do not mechanically rename generic networking concepts where `peer`, `server`, or `host` genuinely describes a protocol role.

---

# Preserve protocol terminology where appropriate

The existing environment variable:

```text
RUST_X11_HELLO_COMPANION
```

may remain temporarily for compatibility.

Do not rename it in the same commit as the discovery work unless doing so is low-risk and compatibility behavior is explicitly provided.

If introducing a future name, prefer:

```text
RUST_X11_HELLO_PAPERSPOON
```

with precedence:

```text
RUST_X11_HELLO_PAPERSPOON
        ↓
RUST_X11_HELLO_COMPANION
        ↓
automatic discovery
```

Log a deprecation notice when the legacy variable is used.

Do not break the verified explicit-host test path.

---

# Architectural requirement

Discovery must remain separate from transport.

Desired boundary:

```text
Discovery
   │
   │ resolves
   ▼
SocketAddr
   │
   ▼
existing TCP connection
   │
   ▼
existing handshake/protocol
```

Discovery must not:

- carry semantic actions;
- replace the TCP handshake;
- define a second protocol version;
- introduce another transport for normal commands.

Its only purpose is:

> Find the current PaperSpoon TCP endpoint.

---

# Preferred direction: Bonjour / DNS-SD

Replace the custom broadcast-discovery design with a Bonjour/DNS-SD experiment.

Advertise a TCP service:

```text
_paperspoon._tcp.local.
```

PaperSpoon publishes the service.

The Kindle browses and resolves it.

Resolution yields:

```text
address
port
```

Then the Kindle uses its already-working TCP implementation.

Conceptually:

```text
PaperSpoon
   │
   ├── TCP listener
   │
   └── advertises
       _paperspoon._tcp.local.
              │
              │ Bonjour/mDNS
              ▼
          Paperwhite
              │
              │ resolve
              ▼
       192.168.x.y:5581
              │
              ▼
       existing TCP path
```

---

# Important constraint

Bonjour also relies on UDP multicast.

Therefore:

**Do not assume Bonjour works simply because it is a standard protocol.**

The existing custom UDP failure means the multicast path must be verified on this exact Kindle/network.

The purpose of the next milestone is to determine whether Bonjour/mDNS works better than the current custom discovery implementation.

---

# Phase 1 — Rename companion to PaperSpoon

Perform the rename before modifying discovery logic.

Suggested commit:

```text
refactor: rename companion tool to paperspoon
```

Rename:

```text
tools/companion
→
tools/paperspoon
```

Update:

- `Cargo.toml`;
- package name;
- binary name;
- source comments;
- CLI usage;
- default log filename;
- Makefile targets;
- scripts;
- README;
- docs;
- AGENTS.md;
- tests;
- diagnostic messages.

The result must build and behave identically to the current companion.

Run the existing explicit-host device path after the rename.

---

# MANUAL CHECKPOINT 1 — Verify rename caused no regression

Have the user run PaperSpoon:

```text
cd tools/paperspoon
cargo build --release
./target/release/paperspoon ...
```

Launch the Kindle with the known working explicit host:

```text
RUST_X11_HELLO_COMPANION=192.168.0.12
```

Verify:

```text
TCP connection: yes/no
handshake: yes/no
ACK: yes/no
Kindle → PaperSpoon action: yes/no
PaperSpoon → Kindle display: yes/no
```

Do not proceed under the assumption the rename is harmless without verifying the existing path.

---

# Phase 2 — Isolate Bonjour from PaperSpoon implementation

Do not immediately add a Bonjour library to PaperSpoon.

First test the network path using macOS's native `dns-sd` utility.

Run the existing PaperSpoon TCP listener normally.

Separately advertise its port using macOS:

```text
dns-sd -R PaperSpoon _paperspoon._tcp local 5581
```

The purpose is to let macOS's own Bonjour stack produce the advertisements.

This isolates two questions:

```text
Can macOS advertise Bonjour correctly?
```

from:

```text
Can the Paperwhite receive and resolve Bonjour?
```

The first should be handled by macOS itself during this test.

---

# Phase 3 — Add Kindle-side Bonjour diagnostic browsing

Add the smallest possible Bonjour/DNS-SD browser to the Kindle application.

Do not immediately make it the normal connection path.

Introduce a diagnostic mode or command that:

```text
browse _paperspoon._tcp.local.
```

and logs every meaningful state transition.

Required diagnostic stages:

```text
discovery bonjour browse-start
discovery bonjour service-found ...
discovery bonjour service-resolved ...
discovery bonjour address=... port=...
discovery bonjour resolve-failed ...
discovery bonjour timeout
```

Do not collapse all failures into:

```text
discovery failed
```

The logs must distinguish:

```text
nothing received
service observed but unresolved
resolved without usable IPv4 address
TCP connection failure
handshake failure
```

---

# Dependency selection

Evaluate Rust Bonjour/mDNS crates before choosing one.

Requirements:

- Rust implementation suitable for the Kindle binary;
- ARMv7 musl cross-compilation must succeed;
- no requirement for Tokio unless absolutely necessary;
- small dependency footprint;
- support DNS-SD browsing and resolution;
- no dependency on macOS-only APIs on the Kindle side.

A candidate such as `mdns-sd` may be evaluated, but do not select it blindly.

Before integrating any crate:

1. inspect its current dependencies;
2. confirm license suitability;
3. confirm ARMv7 musl compilation;
4. check whether it requires multicast socket features available on the Kindle kernel/userspace.

Document the decision.

---

# Phase 4 — Cross-compile before device testing

After adding the smallest Bonjour browser:

Run:

```text
cargo fmt --check
cargo check
cargo clippy
cargo test
```

Then run the existing ARMv7 musl build/verification flow.

Do not proceed to architecture changes if the dependency cannot produce the existing static Kindle artifact.

---

# MANUAL CHECKPOINT 2 — Test Bonjour reception on Paperwhite 6

Mac:

1. start PaperSpoon TCP listener;
2. run:

```text
dns-sd -R PaperSpoon _paperspoon._tcp local 5581
```

Kindle:

1. deploy the new diagnostic build;
2. launch the Bonjour discovery diagnostic;
3. let it run for a bounded interval;
4. retrieve the device log.

Require the user to report one of these outcomes:

```text
A. _paperspoon._tcp service found and resolved to correct Mac IP/port

B. service packets/events observed but service cannot be resolved

C. no _paperspoon._tcp service is seen at all

D. Bonjour browser fails to initialize or bind

E. service resolves but subsequent TCP connection fails
```

Do not call Bonjour functional unless the **Kindle device log** contains a successfully resolved service.

---

# Success criterion

Bonjour discovery is proven only when the Kindle device log shows something equivalent to:

```text
service-found PaperSpoon
service-resolved
address=192.168.0.12
port=5581
```

and that endpoint subsequently completes the existing versioned TCP handshake.

Merely seeing:

```text
dns-sd advertising
```

on macOS is insufficient.

---

# Phase 5A — If Bonjour works

Integrate Bonjour as the normal automatic discovery mechanism.

Connection precedence:

```text
explicit PaperSpoon address configured?
        │
        ├── yes
        │    ↓
        │ direct TCP
        │
        └── no
             ↓
       Bonjour browse
             ↓
       service resolve
             ↓
       existing TCP connect
```

Do not try Bonjour first when an explicit host override exists.

Explicit configuration must remain deterministic.

---

# Discovery timeout

Automatic discovery must be bounded.

Do not block the X11/event loop indefinitely waiting for Bonjour.

Use a clear timeout and transition to a disconnected state.

The UI must remain usable locally even if PaperSpoon cannot be found.

---

# Multiple PaperSpoon services

Do not assume there will always be exactly one service.

Handle at least:

```text
0 services
1 service
>1 services
```

For MVP:

- zero → disconnected;
- one → connect;
- multiple → deterministic selection plus clear diagnostic log.

Do not randomly choose different instances between runs.

A future pairing/device identity mechanism may solve this properly.

---

# PaperSpoon service metadata

Optionally advertise small DNS-SD TXT metadata such as:

```text
protocol=1
role=paperspoon
```

However:

**TXT metadata is advisory only.**

The existing TCP handshake remains authoritative.

Do not skip version negotiation because Bonjour advertised a protocol version.

---

# Phase 6 — Integrate Bonjour advertisement into PaperSpoon

Only after the macOS `dns-sd` test is proven on-device should PaperSpoon itself publish the service.

Evaluate the smallest suitable implementation for the Rust PaperSpoon CLI.

Requirements:

- no Swift helper process merely for advertisement;
- no Network.framework FFI unless clearly justified;
- minimal dependencies;
- clean shutdown should withdraw the service;
- advertise the actual configured listener port.

PaperSpoon startup becomes:

```text
bind TCP listener
        ↓
advertise _paperspoon._tcp.local.
        ↓
accept PaperPad/Kindle connection
```

Do not advertise before the TCP listener has successfully bound.

---

# MANUAL CHECKPOINT 3 — Native PaperSpoon advertisement

Stop using the separate macOS:

```text
dns-sd -R ...
```

command.

Start only:

```text
tools/paperspoon/target/release/paperspoon
```

Verify from the Kindle log:

```text
service discovered: yes/no
correct IP: yes/no
correct port: yes/no
TCP handshake: yes/no
semantic action: yes/no
display command: yes/no
```

Only then consider PaperSpoon's integrated Bonjour advertisement complete.

---

# Phase 5B — If Bonjour does not work

Do not immediately rewrite the Mac tool using Network.framework.

If macOS's own `dns-sd` advertisement cannot be received/resolved by the Kindle, replacing it with another macOS API is unlikely to solve the underlying network path.

Investigate in this order:

1. whether the Kindle can receive multicast UDP on `224.0.0.251:5353`;
2. socket binding/interface behavior;
3. firewall/filtering on Kindle;
4. access-point multicast/client-isolation settings;
5. multicast-to-unicast behavior of the WLAN;
6. `.local` hostname resolution capabilities;
7. router-provided local DNS.

Document evidence.

Do not reintroduce fixed IP as the intended architecture.

---

# Fallback discovery approaches

If Bonjour is genuinely impossible on this environment, evaluate:

## Option A — Router/local DNS hostname

Example conceptual target:

```text
macbook.local-network-name:5581
```

Prefer a stable hostname resolved by DHCP/DNS.

## Option B — `.local` hostname

If Kindle resolver support exists:

```text
hostname.local:5581
```

This may still depend on mDNS and therefore must be verified independently.

## Option C — DHCP reservation

Acceptable operational fallback, but not preferred application architecture.

## Option D — explicit address

Retain only as:

- debugging path;
- recovery path;
- unusual-network override.

Do not make it the normal UX.

---

# Current custom UDP discovery

Do not delete it immediately.

Keep it available while Bonjour is being evaluated so behavior can be compared.

Once Bonjour is physically verified and integrated:

1. mark custom UDP discovery deprecated;
2. remove its default use;
3. remove dead protocol messages/tests/docs;
4. eventually delete it entirely.

Do not maintain two competing automatic discovery protocols without a clear need.

---

# Discovery abstraction

Refactor only enough to separate mechanisms.

Conceptual interface:

```text
Discovery
    ↓
ResolvedEndpoint
    - address
    - port
    - source
```

Possible sources:

```text
Explicit
Bonjour
LegacyUdp
```

Transport receives only:

```text
SocketAddr
```

The TCP layer should not know how the endpoint was discovered.

---

# Logging requirements

Make network troubleshooting evidence-based.

PaperPad/Kindle should log events similar to:

```text
discovery source=explicit host=...
discovery source=bonjour browse-start
discovery source=bonjour service-found name=...
discovery source=bonjour resolved address=... port=...
transport connect address=...
protocol handshake version=...
protocol ack version=...
```

PaperSpoon should log:

```text
paperspoon tcp-listen address=...
paperspoon bonjour publish service=... port=...
paperspoon connection peer=...
paperspoon handshake ...
```

Avoid vague logs like:

```text
failed
discovery failed
connection problem
```

without context.

---

# Network robustness tests

After Bonjour works, manually test the cases that fixed-IP configuration fails at.

## DHCP address change

Change/reconnect the Mac network so its IP changes.

Expected:

```text
PaperPad resolves new address
→ reconnects
→ handshake succeeds
```

No configuration edit should be required.

## PaperSpoon restart

Stop and restart PaperSpoon.

Expected:

```text
service disappears
service reappears
PaperPad reconnects
```

## Mac sleep/wake

Verify service rediscovery and reconnect.

## Kindle Wi-Fi reconnect

Toggle/reconnect Kindle Wi-Fi.

Verify discovery resumes.

## Router/AP restart

If practical, verify that recovery does not depend on previously cached IP addresses.

---

# MANUAL CHECKPOINT 4 — Dynamic-address proof

The dynamic-discovery milestone is not complete until at least one Mac IP change has been tested.

Record:

```text
old Mac IP:
new Mac IP:

PaperPad rediscovered without config change: yes/no
TCP reconnected: yes/no
handshake completed: yes/no
actions work: yes/no
```

This is the actual proof that the hard-coded-address problem has been solved.

---

# Network.framework

Do not introduce Apple's Network.framework into the current prototype by default.

Current PaperSpoon is a Rust CLI.

Using Network.framework now would require:

- Swift/Objective-C helper code;
- Rust FFI;
- or changing the PaperSpoon implementation language.

That is unnecessary unless the pure Rust Bonjour approach proves inadequate for a specific reason.

Network.framework becomes relevant later if PaperSpoon evolves into a native macOS application.

Future architecture could then use:

```text
NWListener
├── TCP listener
└── Bonjour service advertisement
```

That future decision must not block the current prototype.

---

# Repository split

Do not split repositories during this milestone.

Keep:

```text
rust_x11_hello/
├── Kindle side
└── tools/paperspoon/
```

Splitting into:

```text
paperpad
paperspoon
```

should happen after:

1. Kindle touch input is stable;
2. semantic TCP communication is stable;
3. PaperSpoon discovery is stable;
4. changing Mac IP no longer requires configuration;
5. recovery after disconnect has been tested.

At that point the boundary is proven rather than speculative.

---

# Suggested commit sequence

Prefer:

```text
refactor: rename companion tool to paperspoon

refactor: separate endpoint discovery from tcp transport

feat: add bonjour discovery diagnostics

docs: add bonjour device verification procedure
```

After manual Bonjour verification:

```text
feat: use bonjour for automatic paperspoon discovery

feat: advertise paperspoon via bonjour

docs: document automatic paperspoon discovery
```

After successful migration:

```text
refactor: remove legacy udp discovery
```

Do not combine all stages into one commit.

---

# Verification

Run existing project checks after each significant phase:

```text
make check
make build
make verify
```

Run PaperSpoon-specific tests/builds as appropriate.

Always verify the generated Kindle binary remains compatible with the existing ARMv7 musl/static requirements.

Physical Kindle logs remain authoritative for discovery behavior.

---

# Acceptance criteria

The milestone is complete when:

1. `tools/companion` has been renamed to `tools/paperspoon`.
2. The executable is named `paperspoon`.
3. Existing explicit-address TCP behavior still works after the rename.
4. Automatic discovery no longer depends on the current custom UDP broadcast mechanism.
5. PaperSpoon advertises `_paperspoon._tcp.local.` or another justified stable DNS-SD service.
6. The Kindle resolves PaperSpoon to its current address and port.
7. Resolved endpoints feed the unchanged existing TCP transport.
8. Existing versioned handshake and ACK still work.
9. Semantic actions still travel Kindle → PaperSpoon.
10. `display` still works PaperSpoon → Kindle.
11. Changing the Mac IP does not require editing KUAL configuration.
12. Mac restart/sleep or PaperSpoon restart does not permanently break discovery.
13. Explicit host configuration remains available as a diagnostic override.
14. Failure modes have specific device-side logs.
15. Real Paperwhite 6 device evidence confirms the discovery path.

---

# Non-goals

Do not implement during this task:

- repository split into `paperpad` and `paperspoon`;
- native macOS application;
- Hammerspoon integration;
- Network.framework wrapper;
- Swift UI;
- arbitrary remote command execution;
- cloud discovery;
- Internet traversal;
- UPnP;
- NAT traversal;
- TLS redesign;
- protocol redesign;
- USBNetwork work.

The current goal is narrowly:

```text
find PaperSpoon reliably on LAN
        ↓
use existing TCP protocol
```

without relying on a fixed IP address.

---

# Agent interaction requirements

When a physical/network test is required, Codex must stop making assumptions and provide the user with a concrete test.

Each manual checkpoint must include:

1. exact Mac commands;
2. exact Kindle/KUAL action;
3. expected log patterns;
4. location of relevant log files;
5. a small set of possible outcomes;
6. what implementation branch follows from each outcome.

Do not ask:

```text
Did it work?
```

Ask for evidence such as:

```text
Paste the Kindle lines containing:

discovery bonjour
transport connect
protocol handshake
protocol ack
```

or report:

```text
A. service resolved and TCP handshake succeeded
B. service found but resolution failed
C. no service observed
D. Bonjour initialization failed
E. service resolved but TCP failed
```

Do not claim discovery works based solely on PaperSpoon/macOS output.

The Kindle device log is required evidence.
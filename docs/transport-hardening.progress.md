# Transport hardening implementation progress

Date: 2026-08-30 (Europe/Moscow)

This document records intermediate implementation decisions, checks, and
remaining device gates for `docs/transport-hardening.plan.md`. Host checks are
not evidence of Kindle behavior; physical-device claims require checksum-matched
MTP logs as described in `AGENTS.md`.

## Assumptions and scope

- The Kindle and normally one Mac companion share a trusted IPv4 Wi-Fi LAN.
- TCP remains the ordered bidirectional data transport; UDP is discovery only.
- An ACK initially means that a valid event was appended to and flushed from
  the companion log. It does not mean an external side effect completed.
- Explicit host/port configuration remains supported and takes precedence over
  discovery.
- Discovery failure, malformed traffic, reconnects, and ACK timeouts must not
  terminate the X11 UI or create an unbounded wait or buffer.
- No deployment will occur until host checks pass and the operator can assert
  that the installed process is stopped.

## Stage 0 - Baseline audit

Starting commit: `db6320a` (`docs: archive succeed plans & add
transport-hardening plan`). The worktree was clean and `master` matched
`origin/master` before this progress file was created.

Observed baseline:

- Root Kindle package and `tools/companion` are separate Cargo packages; the
  companion has no tests.
- Both `src/net/mod.rs` and `kindle-extension/.../run.sh` fall back to the
  hardcoded USBNetwork address `192.168.15.201`, although `AGENTS.md` records
  USBNetwork as unavailable on this Paperwhite 6.
- The wire format is duplicated conceptually between the Kindle protocol module
  and the companion rather than shared.
- Both sides use `BufRead::lines()`, which does not enforce the plan's maximum
  frame size.
- The companion logs every nonempty inbound line without strict parsing.
- Companion stdin forwarding owns a cloned TCP writer. ACK support must not add
  an independent unsynchronized writer, because message framing would then rely
  on concurrent socket-write behavior.
- The Kindle reader currently recognizes `display` and disconnection only; a
  versioned handshake and ACK messages require an expanded inbound message
  path.

Pre-change checks:

- `cargo test --all-targets`: PASS (22 tests in the current baseline).
- `cargo test --manifest-path tools/companion/Cargo.toml --all-targets`: PASS,
  but there are zero companion tests.
- `sh scripts/test-deploy-kindle-mtp.sh`: PASS.
- `make check`: first run FAILED in
  `net::tests::reconnect_restores_reader_thread` with a transient local socket
  permission error while tests that mutate process-global companion environment
  variables ran in parallel. The same test and the full root suite PASS with
  `--test-threads=1`. This is not treated as a clean baseline gate; transport
  tests must stop relying on concurrently mutated global environment.

## Stage 1 - Shared bounded protocol

Status: complete for the pure protocol layer; socket wiring remains in later
stages.

Planned checkpoint:

- Introduce one shared protocol crate consumed by both binaries.
- Preserve legacy event parsing in the companion only.
- Add strict, bounded parsing/formatting for discovery, handshake, versioned
  events, ACKs, and existing display controls.
- Prove malformed, unknown-version, duplicate-field, overflow, embedded-control,
  and size-bound behavior with unit tests before wiring sockets.

Implemented:

- Added zero-dependency path crate `crates/transport-protocol`, consumed by both
  Cargo packages.
- Added typed discovery (`discover`, `offer`) and stream (`hello`, `welcome`,
  versioned `event`, `ack`, `display`, legacy `event`) messages.
- Fixed bounds: 512-byte discovery datagrams, 1,024-byte stream frames,
  256-byte display text, and 32-hex-character nonce/session/companion IDs.
- Added a bounded `BufRead` line reader that drains an oversized frame through
  its newline, allowing the following valid frame to be read without retaining
  an unbounded allocation.
- Required extension fields to use an `x-` prefix. Unknown ordinary fields and
  duplicate fields are rejected, while `x-` fields are ignored for controlled
  forward compatibility.
- Kept malformed versioned events distinct from legacy events; a bad versioned
  message cannot silently enter the legacy path.

Checkpoint checks:

- Shared protocol formatting: PASS after applying `cargo fmt`.
- Shared protocol unit tests: PASS (7 tests).
- Shared protocol clippy with `-D warnings`: PASS.
- Root and companion `cargo check` with the new path dependency: PASS.
- `git diff --check`: PASS.

## Stage 2 - Companion handshake, logging ACK, identity, and deduplication

Status: complete locally.

Implementation constraints carried forward:

- ACK status `logged` is emitted only after successful append and flush.
- Legacy events remain accepted and logged without ACK.
- Versioned messages receive strict parsing; invalid traffic is diagnosed and
  never logged as an activation.
- Companion-to-Kindle `welcome`, `ack`, and operator `display` frames use one
  serialized writer.
- Deduplication is bounded and in-memory; it prevents live-process duplicate
  logging but does not claim exactly-once behavior across companion restart.

Implemented:

- Split the companion into a small binary entry point and testable library.
- Added backward-compatible positional CLI
  `companion [tcp-port] [log-file] [identity-file] [discovery-port]` with strict
  rejection of invalid ports and excess arguments.
- Added a stable 128-bit companion ID loaded from an identity file. First use
  writes and syncs a temporary file and atomically links it into place without
  overwriting an existing identity.
- Added strict bounded stream parsing. Oversized, invalid UTF-8, unsupported,
  malformed, wrong-session, and wrong-direction frames are diagnosed and not
  logged as activations.
- Added hello/welcome negotiation, session matching, `logged` ACKs after
  append/flush/`sync_data`, and a 256-entry in-memory deduplication window.
- Preserved strict legacy-event parsing and logging without ACK.
- Serialized ACK and operator-display writes through one active-writer mutex;
  an additional connection is rejected while one Kindle is active.
- Added a UDP discovery responder on port 5580 by default. It echoes only valid
  discovery nonces and advertises the configured TCP port and persistent
  companion identity.

Checkpoint checks:

- Companion formatting and clippy with `-D warnings`: PASS.
- Companion unit/integration tests: PASS with localhost permission (initially 5
  tests; expanded failure-path/serialized-write suite is included in the final
  gate below).
- The managed sandbox denies TCP/UDP binds without elevated localhost test
  permission. This is an execution-environment restriction, not a skipped test;
  the permitted run passed.

## Stage 3 - Kindle discovery, ACK tracking, and bounded retry

Status: complete locally; ARM and device gates remain.

Implemented:

- Replaced synchronous connect/write calls on the X11 thread with a dedicated
  named network worker and bounded channels. `send_action` validates and uses
  `try_send`, so it never waits for DNS, UDP, TCP, handshake, or ACK timers.
- Removed the runtime hardcoded companion IP. Explicit host/port configuration
  takes precedence; otherwise the worker sends three bounded UDP broadcasts.
- Discovery validates size, UTF-8/control characters, protocol version, nonce,
  companion ID selection, and response source address. The offer payload has no
  host field; TCP uses the UDP source IP plus advertised port.
- One valid offer is selected automatically. Multiple unpaired IDs are an
  explicit ambiguity error; `RUST_X11_HELLO_COMPANION_ID` selects a paired ID.
- TCP revalidates the companion ID from `welcome`, including equality with the
  discovery offer.
- Added a 32-event pending queue, per-run random session, monotonic `u64`
  sequence, one-second ACK timeout, same-ID retransmission, three-send bound,
  three-connect bound, and 500 ms reconnect backoff.
- Connection loss re-arms only events with remaining attempts. Exhausted events
  are removed and diagnosed rather than becoming permanently pending.
- Display handoff is bounded. Invalid/mismatched/late ACKs and wrong-direction
  frames are ignored with diagnostics.
- Old-companion behavior is an explicit opt-in only:
  `RUST_X11_HELLO_LEGACY=1` plus an explicit host. There is no silent downgrade
  after failed version negotiation.

Checkpoint checks:

- Root formatting and clippy with `-D warnings`: PASS.
- Root test suite: PASS (27 tests) with permitted ephemeral localhost sockets.
- Verified locally: explicit-host bypass, stale-nonce rejection, advertised TCP
  port, discovery handshake, event ACK, same-ID timeout resend, legacy framing,
  no-peer non-blocking enqueue, pairing/ambiguity selection, mismatched ACK,
  exhausted retry cleanup, bounded action validation, and display routing.
- Shared protocol suite: PASS (8 tests).

## Stage 4 - Launcher, repository gates, and documentation

Status: complete locally.

- KUAL no longer injects `192.168.15.201` or the previous Mac LAN address.
  Empty/unset explicit host now starts discovery; an explicit value is preserved
  and logged.
- The long Wi-Fi KUAL action retains its 300-second watchdog but uses discovery.
- `make check` now includes formatting, check, clippy, and tests for the shared
  protocol crate and companion in addition to the root binary and shell/deploy
  checks.
- README transport documentation now covers protocol, ACK meaning,
  retry bounds, discovery conditions, explicit fallback, and trusted-LAN limits.

Completed documentation additionally records:

- UDP port 5580 discovery versus TCP port 5581 data transport.
- Companion CLI and persistent identity file behavior.
- Explicit legacy mode with no automatic downgrade and no ACK claim.
- IPv4 broadcast-domain, firewall, VLAN, AP client-isolation, and trusted-LAN
  conditions.
- Stable identity is selection, not authentication; discovery is not suitable
  for a hostile LAN without a future authenticated pairing layer.

## Stage 5 - Full host and ARM gates

Status: complete.

Full `make check` result after all implementation and documentation changes:

- Shared protocol: formatting/check/clippy PASS; 8 tests PASS.
- Rust companion: formatting/check/clippy PASS; 6 tests PASS, including
  malformed/oversized input recovery, invalid UTF-8, wrong session, duplicate
  suppression, discovery response, and concurrent ACK/display serialization.
- Kindle/root package: formatting/check/clippy PASS; 27 tests PASS.
- KUAL shell syntax, deploy-script syntax, `menu.json`, and deployment
  mock/rollback checks: PASS.
- `git diff --check`: PASS.

The first Docker ARM build failed before code generation because Docker pins
Rust 1.86 while the local host uses Rust 1.97. Six conditionals used newer
edition-2024 `let`-chain syntax that 1.86 still reports as unstable. They were
rewritten as stable combinator/match forms, then the complete host gate was
rerun and passed before retrying ARM.

Final ARM/static results:

- `make build`: PASS with Docker Rust 1.86.
- `make verify`: PASS.
- Artifact: `kindle-extension/rust_x11_hello/bin/rust_x11_hello`.
- Size: 1,123,280 bytes.
- SHA-256:
  `a54f6062aafe87296907f9fa6ae4652af876e1b88aafb99cf8cb22eb6f677d70`.
- `file`: ELF 32-bit LSB executable, ARM, EABI5, statically linked, not
  stripped.
- Static verifier: no dynamic interpreter and no GLIBC symbols.

These results prove host behavior and ARM/static buildability only. They do not
prove that this exact artifact discovers or exchanges ACKs on the Kindle.

## Stage 6 - Physical Kindle verification

Status: pending operator/device gate.

Required before deployment:

1. Confirm the watchdog ended the current run or use **Stop Rust X11 Hello** and
   confirm the window is gone. MTP cannot prove process state.
2. Resolve any retained
   `/extensions/rust_x11_hello/bin/rust_x11_hello.previous`; the update script
   intentionally refuses to overwrite it.
3. Start the new Rust companion on the trusted LAN and allow TCP 5581 plus UDP
   5580 through the Mac firewall.

Then follow `AGENTS.md` update/readback procedure, verify the deployed SHA-256
against the value above, and run the physical matrix from
`docs/transport-hardening.plan.md`. Until checksum-matched device logs are
retrieved, discovery, ACK, reconnect, deduplication, and `display` behavior must
remain described as locally verified only.

### Read-only device preflight

The connected device enumerated as the expected Paperwhite serial
`GN433X11518401E8`. After granting read-only USB access, recursive MTP listing
showed the canonical extension and both active and retained-backup binaries.

Retrieved status:

```text
STOPPED status=143 reason=watchdog
```

This indicates the last recorded run ended by watchdog, but it is not a current
process proof and does not replace visually confirming that the window/process
is gone before `--confirm-stopped`.

Readback hashes before deployment:

- Installed active (959,404 bytes):
  `ef957fc5f5eec96cfe98ec6d8a347f61e2778da4ead24aeb35860a9b391fab18`.
- Retained `.previous` (959,328 bytes):
  `c0d3cb69847dd074d4887705310bbe9b2184b3ed3d949dfa755c134d3411af7f`.
- New host artifact (1,123,280 bytes):
  `a54f6062aafe87296907f9fa6ae4652af876e1b88aafb99cf8cb22eb6f677d70`.

The deployment script will refuse the next update while `.previous` exists.
Removing that retained backup is destructive and has not been authorized or
performed. No package files were changed during this preflight.

### Authorized backup removal

The operator explicitly authorized removing the retained remote backup and supplied the
stopped-process assertion required by the deployment workflow. On 2026-08-30,
`mtp-rs rm /extensions/rust_x11_hello/bin/rust_x11_hello.previous --yes` succeeded and
reported remote handle `24383` deleted. A recovery copy remains available at
`/private/tmp/rust-x11-hello-prehardening.previous`, with SHA-256
`c0d3cb69847dd074d4887705310bbe9b2184b3ed3d949dfa755c134d3411af7f`.

### Deployment and independent readback

`scripts/deploy-kindle-mtp.sh update --confirm-stopped` completed successfully. The
deployment script uploaded and verified the new executable plus launcher/configuration
files, moved the former active executable into the retained `.previous` slot, and
installed `menu.json` last.

The post-deployment recursive MTP listing contains:

- `bin/rust_x11_hello`: 1,123,280 bytes.
- `bin/rust_x11_hello.previous`: 959,404 bytes (the pre-hardening active binary).
- `bin/run.sh`: 8,590 bytes.
- `bin/show.sh`: 359 bytes.
- `bin/stop.sh`: 1,756 bytes.
- `config.xml`: 316 bytes.
- `menu.json`: 402 bytes.
- The existing log and status files.

For an independent check, the active remote executable was downloaded to
`/private/tmp/transport-hardening.deployed`. Its SHA-256 is
`a54f6062aafe87296907f9fa6ae4652af876e1b88aafb99cf8cb22eb6f677d70`, exactly matching
the host artifact. `cmp` also reported byte-for-byte equality. This establishes that
the intended ARM/static artifact is deployed; it does not yet establish runtime
discovery, ACK, reconnect, deduplication, or display behavior.

### Physical matrix setup

The release companion was rebuilt successfully and started as:

```sh
./tools/companion/target/release/companion \
  5581 /private/tmp/transport-hardening-companion.log \
  tools/companion/companion.id 5580
```

It is listening on TCP `0.0.0.0:5581` and UDP `0.0.0.0:5580`. Its newly persisted
identity is `6eb0fc96e41f0e1bbd806ee7ef34efe0`. The process remains attached to terminal
session `23520`, permitting later `display` input and observation. At this checkpoint,
no Kindle discovery or TCP session has yet been observed.

This phase works only if both devices are on the same IPv4 broadcast domain and the
network/macOS firewall permits peer UDP 5580 and TCP 5581. The next action requires
physically launching **Run Rust X11 Hello (WiFi)** on the Kindle; MTP cannot invoke a
KUAL action.

An attempted read-only retrieval of the pre-run device log/status after starting the
companion returned `error: no MTP device found` for both files. The companion remained
running, but no fresh baseline copy was obtained. The last successful recursive listing
before this disconnect showed a 32,254-byte existing log and a 35-byte status file; new
runtime evidence must therefore be identified by run/session markers after MTP access is
restored rather than by a byte-for-byte before/after log comparison.

### First physical launch attempt

The operator reported launching **Run Rust X11 Hello (WiFi)**. No inbound TCP
connection, handshake, or event appeared in the companion terminal after the launch.
The companion process remained live and owned TCP `*:5581` plus UDP `*:5580`.

Host-side diagnostics established:

- The Mac's active `en0` address is `192.168.0.12/24`, with gateway `192.168.0.1`.
- The macOS application firewall is disabled (`State = 0`).
- A corrected loopback discovery datagram received a valid offer containing the exact
  nonce, companion ID `6eb0fc96e41f0e1bbd806ee7ef34efe0`, and port `5581`.
- A LAN peer at `192.168.0.13` answered two ICMP probes (TTL 64), but available evidence
  does not establish that this peer is the Kindle.
- A packet trace restricted to UDP 5580/TCP 5581 could not be started because the
  current process lacks permission to open `/dev/bpf0`.

Therefore this attempt does not yet prove whether the Kindle sent discovery datagrams.
Possible conditions include the KUAL-launched process not remaining active, Kindle
Wi-Fi/subnet state, or broadcast filtering. The host discovery responder and listening
sockets are independently confirmed operational.

The operator then activated `media.play_pause` once while the window was visible. After
waiting beyond the three 250 ms discovery windows and TCP/handshake bounds, the companion
still showed no inbound TCP connection or event. Consequently there is no `logged` ACK
evidence for this activation. The WiFi KUAL action uses a 300-second watchdog, so the
device process may still be active; device logs must be retrieved only after stopping it
or allowing that watchdog to finish.

### First-run device evidence and directional instrumentation

After the operator stopped the application and restored MTP access, the device log,
status, and executable were retrieved to `/private/tmp`. The authoritative status is:

```text
STOPPED status=0 reason=process_exit
```

The retrieved executable still has SHA-256
`a54f6062aafe87296907f9fa6ae4652af876e1b88aafb99cf8cb22eb6f677d70`, matching the
host artifact. The newest controlled run began with UDP discovery, mapped the X11
window, and kept processing touch input. For the single `media.play_pause` activation,
it logged all three discovery failures as `no valid companion discovery offer received`,
then logged `transport failed seq=1 action=media.play_pause
reason=connect_attempts_exhausted`. The exit button was handled and the process exited
with status 0. This proves the failure path remained responsive and bounded, but not why
the bidirectional UDP exchange failed.

The companion previously logged only malformed discovery datagrams, not successful
discover/offer exchanges. A host-only diagnostic line was added after each successful
UDP `send_to`, reporting the source peer, sent byte count, companion ID, and advertised
port. The wire bytes and Kindle artifact are unchanged. `cargo fmt --check` and all six
companion tests passed; the release companion rebuilt and restarted with the same ID in
terminal session `79197`. A second controlled Kindle launch can now distinguish a
request that never reaches the Mac from an offer that is sent but never accepted.

### Second launch: directional discovery result

On the second controlled launch, before any action activation, the instrumented
companion logged two successful 102-byte discovery offers sent to
`192.168.0.4:60702`. This proves that valid Kindle discovery requests reached the Mac
and identifies `192.168.0.4` as the request source. No TCP connection followed.

The client receive/validation implementation was inspected: it accepts only a strictly
parsed offer with the current nonce, takes the TCP address from the UDP source IP plus
advertised port, and has bounded 250 ms windows. No static inspection result explains
the missing TCP connection. Four ICMP probes from the Mac to `192.168.0.4` received no
reply, but that is not conclusive because the Kindle may drop ICMP. The next authoritative
device log is needed to distinguish `no valid offer` from `offer selected but TCP connect
failed`.

The retrieved second-run log resolves that distinction: it again reports
`transport connect attempt 1/3 failed: no valid companion discovery offer received` and
contains no TCP-connect error. It exited normally with `STOPPED status=0
reason=process_exit`. Thus the unicast UDP offer was not received or accepted by the
Kindle even though the companion's `send_to` succeeded locally.

To isolate UDP return-path behavior from TCP reachability, a temporary, validated KUAL
menu was uploaded with a **Diagnostic explicit 192.168.0.12** action. It sets
`RUST_X11_HELLO_COMPANION=192.168.0.12` only for that launch. This address is not added
to repository files or made a runtime default. The normal discovery action remains in
the temporary menu, and the 402-byte repository menu will be restored after diagnosis.

### Explicit-host TCP isolation test

The operator launched **Diagnostic explicit 192.168.0.12**. The companion accepted a
TCP connection from `192.168.0.4:41690`; the connection remained open beyond the 750 ms
handshake timeout. This proves Kindle-to-Mac TCP reachability and strongly indicates
hello/welcome negotiation succeeded, although the checksum-matched device log will be
needed for authoritative client-side handshake identity evidence. The failure is now
isolated to discovery offer return/acceptance rather than general TCP reachability.

The first explicit-host action reached the same TCP peer and was appended to the
companion evidence log exactly once as:

```text
1788104657 192.168.0.4:41690 event v=1 session=c3b9584dc340cc8ce3622488be007038 seq=1 action=media.play_pause;
```

This physically proves versioned handshake completion, event framing, and durable
companion logging for sequence 1. Per the verified companion implementation, its
`logged` ACK is emitted only after append, flush, and `sync_data`; ACK receipt by the
Kindle remains pending device-log retrieval.

The remaining five actions were then activated exactly once. The companion terminal
and durable log contain sequences 2 through 6, all from peer `192.168.0.4:41690`, all
with session `c3b9584dc340cc8ce3622488be007038`, in this order:

1. `media.next`
2. `media.previous`
3. `terminal.new_window`
4. `tmux.work`
5. `zoom.toggle_mute`

Together with sequence 1, this proves one persistent physical TCP connection, one
session, contiguous monotonic sequences 1-6, and exactly one durable companion log
entry per activation during the live companion process. Kindle-side receipt of all six
ACKs still requires device-log retrieval.

`display hello world` was sent over the same live connection. Visual confirmation and
the Kindle-side display log entry are pending.

The operator visually confirmed that `hello world` appeared in the Kindle status strip.
This proves the companion-to-Kindle display path on the live physical connection. The
corresponding device log entry remains to be retrieved.

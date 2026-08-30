Implement the next transport-hardening milestone for rust_x11_hello: automatic companion discovery plus application-level event acknowledgements.

Before changing code:

1. Read AGENTS.md, README.md, the KUAL scripts, src/net/mod.rs, src/proto/mod.rs, tools/companion/src/main.rs, Makefile, and the latest Phase 10 evidence.
2. State assumptions and identify any conflict between this plan and the repository’s current behavior.
3. Record the clean starting Git status.
4. Do not deploy to the Kindle until local checks pass and the operator confirms the existing app is stopped.

Architecture decisions

- Keep the persistent bidirectional TCP connection for events, ACKs, and display commands.
- Use UDP only to discover the companion on the local IPv4 subnet.
- Do not introduce Tokio or another async runtime; the traffic rate does not justify it.
- Preserve explicit host/port overrides. An explicit RUST_X11_HELLO_COMPANION always takes precedence over discovery.
- Remove 192.168.15.201 as the default Wi-Fi runtime destination. USBNetwork is unavailable on this Paperwhite 6.
- Keep the X11 event loop responsive. Discovery, ACK waiting, reconnects, and retry delays must not block it.
- Companion identity is not authentication. Document that automatic discovery is safe only on a trusted LAN.

Protocol

Create a single shared, strictly parsed protocol implementation used by both the Kindle binary and Rust companion. Prefer a small path crate if it does not disrupt the current Makefile/Docker build. Otherwise arrange shared source without duplicating parsers.

Use bounded newline-framed ASCII messages with:

- A protocol version.
- A maximum line/datagram size.
- Strict numeric parsing.
- Rejection of malformed, unknown-version, empty, and oversized input.
- Forward-compatible handling of unknown optional fields where safe.

Suggested messages:

    discover v=1 nonce=<random-hex>;
    offer v=1 nonce=<same> companion=<stable-id> port=<tcp-port>;

    hello v=1 session=<random-hex>;
    welcome v=1 companion=<stable-id>;

    event v=1 session=<session-id> seq=<u64> action=<semantic-id>;
    ack v=1 session=<session-id> seq=<u64> status=logged;

Keep the existing `display <text>` command compatible. Define and enforce its maximum length and single-line restrictions.

Do not use timestamps as event identifiers. The Kindle clock may be wrong. Use a random per-process session ID plus a monotonically increasing sequence number. Timestamps may be added to logs for diagnostics.

Discovery

1. If RUST_X11_HELLO_COMPANION is explicitly set, resolve it with ToSocketAddrs and skip discovery.
2. Otherwise send bounded UDP broadcast discovery requests on a dedicated port, separate from TCP port 5581.
3. Use std::net::UdpSocket unless testing proves that socket2 is necessary.
4. Send a small bounded number of probes with bounded timeouts; do not loop indefinitely.
5. Require an offer to echo the discovery nonce.
6. Use the UDP response’s source IP. Never trust an IP address supplied inside the payload.
7. Let the offer advertise the TCP port.
8. Collect offers for a short window:
   - If an expected companion ID is configured, select only that ID.
   - If exactly one valid offer exists, select it.
   - If several unpaired companions answer, report ambiguity and remain disconnected.
9. Verify the companion ID and protocol version again in the TCP handshake.
10. If discovery fails, keep the UI operational and retry with rate limiting on a later activation.
11. Preserve explicit configuration as the fallback for networks that block broadcast, enable client isolation, or place devices on different VLANs.

The companion needs a stable random ID persisted locally. Create it atomically when absent. Do not use the Mac’s IP address as identity. Preserve the existing positional CLI behavior or provide a backward-compatible migration.

ACK behavior

- The initial ACK status is `logged`.
- Send it only after the companion has:
  1. parsed and validated the event;
  2. appended it to the companion log;
  3. successfully flushed the log.
- Do not call this `done`; it does not prove an external media/tmux action succeeded.
- Track a small bounded pending-event queue on the Kindle.
- ACK handling must be asynchronous relative to X11 processing.
- On connection loss or ACK timeout, retry only with the same `(session, seq)`.
- Use bounded retry count and bounded backoff.
- The companion must remember recently logged `(session, seq)` values and ACK duplicates without logging or executing them again.
- Clearly document that in-memory deduplication does not provide exactly-once execution across a companion crash. Do not claim exactly-once semantics.
- Serialize all companion-to-Kindle writes. ACK and `display` messages must not write concurrently through independent cloned TcpStreams.

Compatibility

- The new Rust companion should continue accepting the existing legacy
  `event action=<semantic-id>;` format during migration.
- Legacy events can be logged without ACK.
- The Kindle should use the versioned protocol only after receiving a valid
  `welcome`.
- Decide and document the timeout/fallback behavior when connected to an old
  companion.
- Do not silently reinterpret malformed versioned messages as legacy events.

Required tests

Protocol unit tests:

- Round-trip every message type.
- Unknown version.
- Missing, duplicate, malformed, and unknown fields.
- Invalid semantic action.
- Empty and oversized lines/datagrams.
- Embedded newline/control characters.
- Sequence overflow.
- Display text boundary cases.

Discovery tests:

- Valid offer with matching nonce.
- Stale or mismatched nonce.
- Malformed and oversized offer.
- No companion.
- One companion.
- Multiple companions without pairing.
- Multiple companions with a matching configured ID.
- Advertised port override.
- Explicit hostname/IP bypasses discovery.
- Response payload cannot redirect the client to another IP.

TCP/ACK integration tests using ephemeral localhost ports:

- Hello/welcome negotiation.
- Event is ACKed only after log flush.
- Duplicate ID receives ACK but is logged once.
- Two different sequence numbers are logged in order.
- Disconnect before ACK and reconnect/resend.
- Companion unavailable at startup.
- Companion disappears during a run.
- Malformed ACK.
- Late ACK after timeout.
- Concurrent display and ACK traffic remains correctly framed.
- Pending queue and retry limits are enforced.

Failure-path requirements

- Network failures must not terminate or freeze the X11 UI.
- Never allocate an unbounded buffer while reading a line.
- A spoofed or stale discovery response must not be accepted.
- A TCP write success must not be logged as companion acknowledgement.
- Ambiguous discovery must be visible in the device log.
- Invalid configuration must produce a useful error and fall back to a disconnected state.
- Avoid busy loops and high-frequency polling that would waste Kindle battery.

Documentation

Update README.md and KUAL behavior/documentation to explain:

- TCP is the data transport; UDP is discovery-only.
- Explicit override precedence.
- Discovery ports and same-LAN requirements.
- Firewall, client-isolation, VLAN, and hostile-LAN limitations.
- ACK status meaning.
- Retry and duplicate semantics.
- No exactly-once guarantee across companion crashes.
- How to configure a specific companion ID.
- How to fall back to an explicit hostname or IP.

Verification sequence

1. Run formatting and static/local checks.
2. Run all root Rust tests.
3. Run companion tests separately because it is not currently a workspace member.
4. Run the deployment-script mock/rollback tests.
5. Run `make check`.
6. Run `make build`.
7. Run `make verify`.
8. Confirm the generated ARMv7-musl artifact and report its SHA-256.
9. Confirm Git diff contains no unrelated changes.
10. Do not claim Kindle behavior from host checks.

Physical-device verification

Only after the operator confirms the existing process/window is stopped:

1. Follow AGENTS.md update procedure and retain the verified `.previous` binary.
2. Verify MTP readback SHA-256 matches the host artifact.
3. Start the Rust companion without supplying the Mac IP.
4. Launch “Run Rust X11 Hello (WiFi).”
5. Confirm discovery offer, TCP handshake, and companion identity in logs.
6. Activate all six actions and prove:
   - one persistent TCP connection;
   - one session;
   - monotonically increasing sequence numbers;
   - one `logged` ACK per action;
   - exactly one companion log entry per event during the live companion process.
7. Send `display hello world` while the connection is live and confirm the Kindle renders and logs it.
8. Stop the companion mid-run, activate an action, restart the companion, and verify bounded rediscovery/reconnect/resend.
9. Test no-companion behavior and confirm X11 remains responsive.
10. If practical, start two companions and confirm ambiguity or configured-ID selection.
11. Retrieve Kindle log/status over MTP and verify the deployed checksum before treating results as authoritative.
12. Record conditions, failures, exact commands, hashes, and limitations in a new evidence directory.

Keep protocol, ACK, discovery, and documentation changes logically separable for review. Do not claim completion until local gates pass and the physical-device evidence confirms discovery, ACKs, reconnect behavior, and display delivery.

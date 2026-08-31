# Codex Plan: Minimal Zero-Config UDP Discovery for PaperPad / PaperSpoon

## Goal

Implement the smallest possible custom UDP discovery mechanism that allows the Kindle-side PaperPad prototype to locate PaperSpoon on the same LAN without configuring an IP address.

The outcome is binary:

```text
zero-config discovery works reliably
```

or:

```text
zero-config discovery is not viable on this Kindle/network
```

Do not turn this task into general transport hardening.

Do not add alternative discovery mechanisms after this experiment.

If the experiment fails after the packet path described below is verified, stop and document the result.

---

# Starting point

Use `master` as the implementation baseline.

Do **not** use `ack-udp-discovery` as the base branch because it mixes discovery work with unrelated transport changes such as:

- versioned TCP protocol;
- sessions;
- ACKs;
- retries;
- deduplication;
- persistent identities;
- protocol negotiation;
- discovery/TCP identity matching.

Those features are outside the scope of this task.

The existing `ack-udp-discovery` and `bonjour` branches are reference material only.

Reuse useful observations and diagnostic techniques from them, but do not carry their transport complexity forward.

---

# Established physical-device evidence

Treat these observations as already proven on the Paperwhite 6 and current LAN.

## Working

Explicit TCP communication works when PaperPad knows the Mac IP:

```text
PaperPad
    |
    | TCP
    v
192.168.0.12:5581
    |
    v
PaperSpoon
```

Therefore TCP is not the problem.

Do not redesign the working TCP data path.

## Custom UDP experiment

The previous custom discovery implementation successfully reached PaperSpoon:

```text
PaperPad
    |
    | UDP broadcast DISCOVER
    v
PaperSpoon
```

PaperSpoon then sent discovery offers back.

The Kindle application did not receive a valid offer.

## Bonjour experiment

The later diagnostic work established an important distinction:

```text
Kindle local multicast       works
Mac -> Kindle unicast UDP    works with explicit INPUT permission
Mac -> Kindle multicast UDP  does not work
```

The Kindle firewall normally has a restrictive INPUT policy.

This means the next custom-discovery experiment must explicitly account for the Kindle firewall instead of assuming conntrack will classify the discovery response as an established UDP flow.

---

# Non-goals

Do not implement:

- Bonjour;
- mDNS;
- DNS-SD;
- Avahi;
- Network.framework discovery;
- multicast discovery;
- TCP service scanning;
- ARP scanning;
- subnet scanning;
- router integration;
- DHCP inspection;
- persistent PaperSpoon identity;
- pairing;
- authentication;
- encryption;
- ACKs;
- event retransmission;
- exactly-once semantics;
- protocol negotiation;
- compatibility with multiple protocol versions.

Do not add another discovery fallback if custom UDP fails.

The purpose of this branch is to determine whether custom UDP zero-config discovery works.

---

# Naming

Use the project names:

```text
PaperPad   = Kindle side
PaperSpoon = macOS side
```

Rename:

```text
tools/companion
```

to:

```text
tools/paperspoon
```

The Rust package and executable should also be:

```text
paperspoon
```

Prefer new names such as:

```text
PaperSpoon
paperspoon
paperspoon_endpoint
paperspoon discovery
```

Avoid introducing more generic `companion` terminology.

The legacy environment variable may remain temporarily if changing it would unnecessarily expand this task.

---

# Discovery architecture

Use three fixed ports:

```text
UDP 5580  PaperSpoon discovery listener
UDP 5582  PaperPad discovery client
TCP 5581  existing PaperSpoon TCP listener
```

The important change from the previous experiment is:

```text
PaperPad MUST bind UDP port 5582
```

rather than an ephemeral UDP port.

The complete discovery exchange should be:

```text
PaperPad                         PaperSpoon
192.168.0.4                      192.168.0.12

UDP :5582
    |
    | DISCOVER
    | -> 255.255.255.255:5580
    +---------------------------->
                                  receives DISCOVER
                                  learns source:
                                  192.168.0.4:5582

    <----------------------------+
          HERE
          UDP unicast
          192.168.0.12:5580
          ->
          192.168.0.4:5582

PaperPad learns:

    PaperSpoon IP = UDP source IP
    PaperSpoon TCP port = response payload

PaperPad then:

192.168.0.4
    |
    | TCP
    v
192.168.0.12:5581
```

Do not broadcast the response.

The response must be unicast.

---

# Minimal discovery wire format

Keep the protocol deliberately small.

A request needs only:

```text
magic
nonce
```

A response needs only:

```text
magic
same nonce
TCP port
```

For example, conceptually:

```text
PAPERPAD_DISCOVER <nonce>
```

and:

```text
PAPERSPOON_HERE <nonce> 5581
```

Exact textual syntax may differ if there is a simpler implementation.

Requirements:

- packets must have an unambiguous project-specific marker;
- nonce must be unpredictable enough to distinguish the current discovery attempt from stale/unrelated packets;
- response nonce must exactly match;
- TCP port must be a valid non-zero `u16`;
- ignore malformed packets;
- ignore packets with a different nonce.

Do not add:

```text
session IDs
PaperSpoon persistent IDs
ACK numbers
sequence numbers
capability lists
protocol negotiation
```

The UDP source address is the discovered PaperSpoon address.

Do not put the PaperSpoon IP address inside the response payload.

---

# Kindle firewall

This is a required part of the experiment.

The Kindle must explicitly permit the PaperSpoon unicast response.

The temporary rule should be approximately:

```text
interface: wlan0
protocol: UDP
source port: 5580
destination port: 5582
action: ACCEPT
```

The source IP cannot be restricted because PaperSpoon's IP is exactly what discovery is trying to determine.

Keep the rule as narrow as practical:

```text
-i wlan0
-p udp
--sport 5580
--dport 5582
-j ACCEPT
```

The discovery nonce remains the application-level filter for unrelated packets that happen to match those ports.

For this prototype, it is acceptable for the rule to exist for the duration of the bounded KUAL run if that makes cleanup substantially simpler.

Do not build a complicated dynamic firewall lifecycle unless it is necessary.

The launcher must:

1. remove any stale discovery rule/chain left by a previous failed run;
2. install the discovery permission before starting PaperPad;
3. verify installation;
4. remove it during normal launcher cleanup;
5. attempt cleanup after child termination/watchdog termination as well.

Use a dedicated chain/name so this experiment never modifies unrelated Kindle firewall rules.

For example:

```text
PAPERPAD_DISCOVERY
```

or an equivalently project-specific name.

Do not flush Kindle firewall tables.

---

# PaperPad implementation

Discovery must remain separate from normal TCP transport.

Desired boundary:

```text
discover_paperspoon()
        |
        v
SocketAddr
        |
        v
existing TCP code
```

Implement a small function/module whose only responsibility is:

```text
find PaperSpoon TCP endpoint
```

It should:

1. bind:

   ```text
   0.0.0.0:5582
   ```

2. enable UDP broadcast;

3. generate a fresh nonce;

4. send the discovery request to:

   ```text
   255.255.255.255:5580
   ```

5. wait for responses for a short bounded interval;

6. validate:

   - sender;
   - packet format;
   - nonce;
   - TCP port;

7. return:

   ```text
   SocketAddr {
       ip: response source IP,
       port: advertised TCP port
   }
   ```

8. close the UDP socket;

9. continue through the already-working TCP path.

Do not leave a discovery thread running after an endpoint has been found.

---

# Retry policy

Keep retries simple.

Use approximately:

```text
3 discovery probes
250-500 ms response window each
```

The precise values may be adjusted based on physical testing.

Do not introduce exponential backoff or long-running discovery.

A complete failure should finish within a few seconds.

Example:

```text
probe 1
wait

probe 2
wait

probe 3
wait

no valid response
-> discovery failed
```

---

# Multiple PaperSpoons

Do not build pairing.

For the experiment:

- zero valid responders -> failure;
- exactly one valid responder -> use it;
- multiple different responders -> report ambiguity and fail.

Do not silently pick a random PaperSpoon.

Identity can simply be the response source IP + advertised TCP port for this experiment.

---

# PaperSpoon implementation

PaperSpoon should have:

```text
TCP listener :5581
UDP discovery listener :5580
```

The discovery responder should:

1. bind:

   ```text
   0.0.0.0:5580
   ```

2. receive a DISCOVER datagram;

3. validate its marker and nonce;

4. obtain the sender address from `recv_from`;

5. create the HERE response containing:

   ```text
   same nonce
   TCP port 5581
   ```

6. send it once by unicast to the exact source:

   ```text
   sender_ip:sender_port
   ```

There should be no response to:

```text
255.255.255.255
```

There should be no multicast.

There should be no ACK for the discovery response.

---

# Keep TCP simple

Once discovery produces:

```text
192.168.0.12:5581
```

use the existing simple TCP path from `master`.

Do not import the versioned transport from `ack-udp-discovery`.

For this task, successful discovery is proven by:

```text
UDP discovery
    ->
resolved endpoint
    ->
TCP connect
    ->
existing semantic event reaches PaperSpoon
```

That is enough.

---

# Explicit host override

Keep the existing explicit IP path while developing.

For example:

```text
RUST_X11_HELLO_COMPANION=192.168.0.12
```

should continue to bypass discovery.

This is necessary as a control case.

Behavior should be:

```text
explicit host configured
    -> skip UDP discovery
    -> connect directly

no explicit host
    -> run UDP discovery
    -> connect discovered endpoint
```

Do not automatically fall back to a hard-coded IP when discovery fails.

That would hide the result of the experiment.

---

# Diagnostics

This experiment must make every boundary observable.

Avoid enormous general system dumps unless a failure requires them.

The normal device log should contain concise records equivalent to:

```text
discovery bind address=0.0.0.0:5582
discovery probe attempt=1 destination=255.255.255.255:5580 nonce=...
discovery probe sent bytes=...
discovery response from=192.168.0.12:5580 bytes=...
discovery response valid endpoint=192.168.0.12:5581
transport connecting endpoint=192.168.0.12:5581
transport connected endpoint=192.168.0.12:5581
```

PaperSpoon should log:

```text
discovery listening address=0.0.0.0:5580
discovery request from=192.168.0.4:5582 nonce=...
discovery response sent to=192.168.0.4:5582
```

Never log only:

```text
discovery failed
```

without enough information to locate the failure.

---

# Firewall counters

For the physical-device test, obtain an iptables packet counter for the temporary discovery ACCEPT rule.

The final evidence chain should be:

```text
1. PaperPad logs DISCOVER sent
2. PaperSpoon logs DISCOVER received
3. PaperSpoon logs unicast HERE sent
4. Kindle firewall rule counter increments
5. PaperPad recv_from receives HERE
6. nonce validates
7. TCP connection succeeds
```

If a failure occurs, identify exactly which numbered boundary failed.

This is the main purpose of the diagnostic instrumentation.

---

# Physical-device test matrix

Run tests on the actual Paperwhite 6.

Host-only tests cannot establish discovery viability.

## Test A — explicit host control

Configure:

```text
RUST_X11_HELLO_COMPANION=192.168.0.12
```

Verify:

```text
PaperPad -> TCP -> PaperSpoon
```

still works.

If this fails, stop: the experiment is no longer testing discovery in isolation.

## Test B — custom discovery

Remove the explicit host configuration.

Start PaperSpoon.

Launch PaperPad through KUAL.

Expected:

```text
PaperPad broadcast DISCOVER
PaperSpoon receives it
PaperSpoon unicasts HERE to PaperPad:5582
Kindle firewall accepts it
PaperPad receives it
PaperPad connects TCP to PaperSpoon
```

## Test C — restart

Restart PaperPad without restarting PaperSpoon.

Discovery must work again.

## Test D — changed Mac DHCP address

If practical, change/reacquire the Mac LAN address.

Do not change PaperPad configuration.

Restart PaperPad.

Expected:

```text
new PaperSpoon address is discovered automatically
TCP connection succeeds
```

This is the actual zero-config requirement.

## Test E — PaperSpoon absent

Stop PaperSpoon.

Launch PaperPad.

Expected:

- bounded discovery attempts;
- no hang;
- clear failure log;
- UI/X11 loop remains operational where appropriate.

---

# Host-side tests

Add focused tests only.

Test:

- request parsing;
- response parsing;
- nonce mismatch rejection;
- malformed datagram rejection;
- invalid TCP port rejection;
- constructing endpoint from UDP source address;
- zero / one / multiple candidate selection;
- PaperSpoon responder sends response to request source port;
- explicit host bypasses discovery.

Do not create a large protocol framework.

---

# Acceptance criteria

The experiment succeeds only if the following works on the physical Kindle with no PaperSpoon IP configured:

```text
start PaperSpoon
        |
        v
launch PaperPad
        |
        v
automatic UDP discovery
        |
        v
TCP connection
        |
        v
semantic action reaches PaperSpoon
```

and the process remains successful after the Mac receives a different DHCP address.

The user must not enter or edit an IP address.

---

# Failure criterion

Stop the custom-discovery work if all of the following are observed:

```text
PaperPad DISCOVER sent
PaperSpoon DISCOVER received
PaperSpoon unicast HERE sent
Kindle temporary firewall rule matches the incoming datagram
PaperPad socket is correctly bound to :5582
PaperPad still cannot receive a valid HERE response
```

At that point document custom UDP zero-config discovery as unsupported/unreliable on this Kindle/network combination.

Do not proceed to:

- Bonjour again;
- another multicast protocol;
- broadcast responses;
- port scanning;
- ARP tricks;
- router APIs;
- more transport protocol layers.

The result should be:

```text
zero-config LAN discovery: not viable
```

and the branch can be archived as evidence.

---

# Cleanup after the experiment

If discovery succeeds:

1. remove experimental packet-dump code;
2. keep concise discovery logs;
3. keep the fixed UDP ports;
4. keep the narrow Kindle firewall rule;
5. keep explicit-host override as a diagnostic escape hatch;
6. keep discovery isolated behind a small endpoint-resolution API;
7. update README with the verified Paperwhite 6 behavior.

If discovery fails:

1. keep the physical-device evidence logs;
2. document exactly where packets stopped;
3. do not merge speculative discovery code into the main path;
4. retain explicit-host TCP as the known-working control;
5. mark the zero-config discovery experiment closed.

---

# Implementation order

1. Start from `master`.
2. Rename `tools/companion` to `tools/paperspoon`.
3. Preserve the existing simple TCP behavior.
4. Add fixed UDP ports `5580` and `5582`.
5. Add minimal request/response encoding.
6. Add PaperSpoon UDP responder.
7. Add PaperPad UDP discovery client.
8. Add narrow Kindle firewall permission.
9. Add concise logs and firewall counters.
10. Add focused host tests.
11. Verify explicit-host control path.
12. Perform physical discovery test.
13. If successful, test after Mac DHCP address change.
14. Either integrate the proven discovery path or document failure and stop.

---

# Guiding constraint for Codex

Whenever an implementation choice appears to require additional machinery, ask:

> Is this required to prove that a broadcast request followed by a unicast UDP response can discover PaperSpoon on this Kindle?

If the answer is no, do not implement it.
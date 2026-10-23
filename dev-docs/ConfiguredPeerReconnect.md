# Configured peer reconnect

Date: 2026-10-23. Scope: existing peer mesh and both node composition roots.

## Behavior

An addressed configured peer MUST be maintained for the running node's lifetime.
An initial failure or later closed link MUST NOT require restarting Workbench.
The one-shot `Server::connect_peer` API remains available unchanged; startup uses
`Server::maintain_peer` to install the lifecycle behavior explicitly.

The initial attempt returns its ordinary result. Even an initial failure installs
one Sessions-owned retry task per configured endpoint key. Duplicate registration
MUST return `AlreadyExists`, without creating another dialer. Admit-only entries
MUST NOT create a dialer. An existing authenticated inbound or outbound link to
that endpoint MUST suppress unnecessary dials. The task checks once per second
while connected. After failure, delays are 1, 2, 4, 8, 16, then 30 seconds, capped
at 30 seconds; each dial/HELLO/directory convergence attempt has a 30-second bound.
This is a delay between attempts, not a total recovery-time guarantee.

Every new connection MUST use the existing carrier and authenticated HELLO path,
including its current door and grants. This change does not alter trust, enable
relays, repair Wi-Fi/ZeroTier, discover new addresses, retry application commands,
or replicate the Workbench SQLite database. Shutdown MUST cancel the retry task
before releasing the carrier. No dependencies or wire formats change.

## Regression and scope of evidence

The baseline startup's one-shot dial failed
`configured_peer_reconnects_after_link_loss`: after explicitly ending a real
loopback Iroh link, no replacement appeared within five seconds. The fixed test
uses `maintain_peer`, first confirms that an already healthy inbound link is
retained, ends that link, waits for a new link and pulls directory data over it.

Two paused-clock transport tests cover initial failure, exact capped backoff,
admit-only rejection, duplicate registration, a stalled attempt's deadline,
absence of concurrent dials and cancellation of sleeping/pending retry work by
the actual Sessions task-owner mechanism. They do not pretend the unavailable
test transport authenticated a connection.

Focused command: `cargo test --locked --offline --lib configured_peer_`.
Surrounding checks: `cargo test --locked --offline --lib mesh::`, the adopted
architecture checker, and the process-global state guard. Live deployment and
cross-machine disruption evidence are recorded separately after execution.

## Execution evidence

Magenta, Windows, Rust stable 1.98.1, existing locked/offline dependencies:

- RED: former one-shot startup behavior failed the real-Iroh reconnect test in
  5.26 seconds (configured peer stayed disconnected).
- GREEN: all three `configured_peer_` tests passed in 2.31 seconds; the real
  Iroh case includes keeping an existing inbound link, closure, replacement and
  a successful directory pull. Timing and cancellation use paused Tokio time.
- Surrounding mesh suite: 53 tests passed in 4.87 seconds.
- `cargo check --locked --offline --bins` passed, checking the binary and both
  composition paths. `cargo clippy --locked --offline --lib` passed with existing
  unrelated diagnostics (nine Clippy warnings and one dead-code warning).
- Adopted architecture checker passed. Process-global guard passed with 169
  files, three permanent allowlisted items and no new occurrences. `git diff
  --check` passed. New code was formatted using the installed Rust 1.95 rustfmt;
  unrelated formatting debt was preserved.

The fix was built and installed into the two existing stacks, with copies of
their preceding binaries retained in each ignored runtime directory. Beast's
build took 10.50 seconds; Magenta's took 18.33 seconds. Runtime supervisors,
identities, app stores and the SQLite material database were retained in place.

At 09:35 UTC, Magenta was stopped/rebuilt/restarted while Beast stayed running
under supervisor 37256, started at 09:33:43 UTC. Beast logged link closure,
then a new authenticated direct link, a four-record home round in 62 ms and
`peer-connected`, without restarting its supervisor. This is an actual
two-machine peer restart/reconnect witness; it does not simulate disconnecting
the operator's Wi-Fi adapter or prove every network-migration scenario.

Post-recovery checks returned HTTP 200 for both UIs/APIs and `supplierLive:true`.
An independent client on Beast received `garns.current`, fetched the referenced
material from its local authoring API and verified the digest and fourteen
requirements. Both database heads/digests/counts remained unchanged. This verifies
the material-delivery path, not the operator's RDP browser rendering or the
separately reported asymmetric browser selection behavior. Browser shared-desk
reconnect and write-outcome limitations remain outside this peer-loop fix.

Checkpoint scope: local implementation and execution evidence. No tag, release
or push was performed.

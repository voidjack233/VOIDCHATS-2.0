> Historical pre-consolidation verification at backend `2faffd2`. Its separate
> application-V2 release conclusions are superseded by [initial V1](protocol-v1.md).
> Current tests use unchanged `9eb1319` with minimum V1. This record is retained
> to explain the original queue-barrier and cryptographic recovery investigation.

# Receiver recovery and protocol V2 verification

Verified locally on 18 November 2026. Production negotiation remains version 1.
No production identity, database, relay, or deployment was used.

## Disposable backend

The Rust integration test ran against the actual, unchanged Go implementation at
`2faffd27f12d1d02ec28e4be298610d4fee50856`. Downloaded source blobs were checked
against their Git blob hashes before building. The fixture used a new standalone
PostgreSQL 18 cluster, unique loopback ports, the backend's real migrations
through 008, and only synthetic accounts 101 and 202. PostgreSQL's native
`uuidv7()` was used; no schema or Go behavior was stubbed. Nothing was installed
as a Windows service or changed in an existing PostgreSQL instance.

All Rust SQLCipher directories and account/Signal identities were isolated test
fixtures. The HTTPS API audience in authentication was signed transcript text;
the only relay socket was the disposable `ws://127.0.0.1` endpoint.

After verification the disposable queue contained zero events. The task's Go
and PostgreSQL processes were stopped, both loopback ports were verified closed,
and the standalone cluster and synthetic password file were removed. Portable
runtime/source caches remain only under ignored `target/receiver-v2-backend/`.

## Results

The normal suite passed **113 tests**, with **2 ignored**: the opt-in actual-Go
test and the pre-existing synthetic storage timing test. The actual-Go test was
then run explicitly and passed separately. Rust build, check, and changed-file
formatting checks also passed.

The actual-Go test verified:

- Ed25519 authentication followed by protocol negotiation.
- Legacy text and authenticated encrypted delivery receipts.
- A version-2 queue head holding later version-1 ciphertext while the recipient
  advertised version 1, then resuming on a compatible version-2 reconnect.
- Real libsignal ciphertext containing reply, reaction, and deletion events,
  processed through the normal messaging receive/SQLCipher/ACK path.
- Failed local COMMIT leaving the server queue head unacknowledged; reconnect
  redelivering identical ciphertext, followed by a successful durable commit.
- A committed event with a lost ACK being redelivered and deduplicated without
  another Signal ratchet advance or duplicate reaction state.
- Ciphertext delivered out of encryption order, monotonic reaction removals,
  sender-authorized deletion, and fresh negotiation on reconnect.
- Known deferred content recovering after restart from saved plaintext, an
  unknown event remaining deferred, unchanged ratchet bytes, idempotent repeated
  recovery, and subsequent legacy text remaining readable.

Focused local tests also verified decoder upgrades, schema 10-to-11 preservation,
original receipt deadlines, more than one bounded page of unknown events,
tombstones, reaction conflicts, malformed/unauthorized/provenance-invalid
quarantine, and failed recovery COMMIT rollback. Existing account, history,
receipt, and timeline tests remained passing.

## Repeating the opt-in integration

Provision a fresh disposable backend from the exact commit above. Apply its real
migrations only to the new cluster and seed accounts 101 and 202 with public
Ed25519 keys derived from repeated-byte test seeds 121 and 122. These are public
test fixtures, never credentials for an existing account. Keep this fixture
separate from every production or personal database. Reset the disposable
queue before each run and stop/remove the cluster when finished.

The normal Rust suite does not start Go or PostgreSQL. Explicitly run the guarded
test with a loopback relay URL and its configured authentication audience:

```powershell
$env:VOID_TEST_DISPOSABLE = '1'
$env:VOID_TEST_BACKEND_COMMIT = '2faffd27f12d1d02ec28e4be298610d4fee50856'
$env:VOID_TEST_RELAY_URL = 'ws://127.0.0.1:<test-port>/dm9pZG5vdGhpbmduZXNz/ws'
$env:VOID_TEST_RELAY_AUDIENCE = '<the disposable relay authentication audience>'
cargo test --locked --offline actual_go_v2_receiver_recovers_blocked_queue_and_commits_before_ack -- --ignored --nocapture
```

The version override is compiled only with `cfg(test)`, requires a numeric
loopback `ws` URL without credentials, and is absent from the production client.
Ordinary startup has no environment variable or UI switch to enable version 2.

## Readiness and remaining limits

The defined receiver formats and durable recovery path passed the targeted
checks. Version 2 is intentionally still not advertised by production. Before a
coordinated release, freeze the complete version-2 encrypted-event contract and
provide the presentation of persisted replies, reactions, and deleted-message
placeholders. These tests do not exercise those future Slint interactions.

Feature sending remains unimplemented. It needs separate persistent outgoing
controls, revision allocation, retry/dedup rules, and peer compatibility policy;
the relay advertises only a connection's receive capability, not an offline
peer's feature support. The future emoji picker must enforce its choice policy
and visible reaction limit; receiver token storage intentionally preserves
concurrent authenticated state.

Genuinely unsupported content remains pending, and quarantined entries require
an explicit repair policy rather than automatic reinterpretation. A conflicting
reaction may require another recovery pass after a later revision is applied.
Recovery currently runs on the existing storage worker at open, without an
additional background service.

This test does not certify production deployment, restricted-role database
permissions, multiple devices, long-duration network behavior, or delivery beyond
the relay's 30-day retention. Deletion tombstones protect events actually received
and committed; a deletion expiring on the server cannot be recovered locally.

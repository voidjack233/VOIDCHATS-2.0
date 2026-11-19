# Initial VOID Native messaging protocol V1

VOID Native is unreleased. Application protocol `"1"` is the initial complete
contract: ordinary text, encrypted delivery receipts, replies, reaction state,
Delete for Me, Delete for Everyone, offline delivery and durable recovery.
Normal builds include all operations without a Cargo feature, runtime override,
peer handshake or separate coordinated V2 activation.

## Independent version namespaces

| Namespace | Initial baseline | Purpose |
| --- | --- | --- |
| Relay application negotiation | `protocol` / `protocol_ok`, `version: "1"` | Receiver contract and future mandatory updates |
| Signal envelope and text/control codec | JSON `v: 2` | Existing strict encrypted-payload encoding; not application V2 |
| Delivery receipt codec | JSON `v: 1` | Existing verified message-receipt encoding |

Authentication transcript identifiers, account/Signal binding signatures,
libsignal ratchets, device identifiers and public prekey formats are unchanged.
There is no blind renumbering of cryptographic data. The frozen payload formats
are specified in [encrypted-event-receiver.md](encrypted-event-receiver.md).

## Transport and readiness

Every authenticated connection negotiates `"1"` before messaging becomes
available. All initial message types use `send` and `event`, with implicit relay
minimum `"1"`; Go sees only an opaque encrypted payload. They do not use
`send_versioned` or require `min_protocol_version: "2"`. Acceptance correlation,
post-commit ACKs, queue ordering and the existing 30-day retention stay unchanged.

The unchanged backend at `9eb1319` supports this contract with minimum `"1"`.
Generic future-version transport support is retained. Incoming versions above
the advertised/receiver ceiling are rejected before Signal state changes. A
genuine future `update_required` minimum blocks messaging and stops reconnection
without changing accounts, sessions or history.

Reply/reaction/remote-delete controls require messaging readiness, not a special
release profile. Delete for Me and reply cancellation work offline. Restored
reply drafts can be queued offline like text and explicitly resent later.
Ordinary messages/replies are not automatically retransmitted at startup.
Pending controls resume through the existing bounded retry dispatcher, retaining
their UUIDs, revisions and committed ciphertext. Relay acceptance of a control
does not establish recipient application. Text/replies retain verified receipts.

## Storage and experimental data

No schema migration, account reset, SQLCipher wipe, key regeneration or message
rewrite is needed. Supported content previously deferred under minimum `"1"`
recovers locally without decrypting again. Genuinely unsupported or invalid
records retain the deferral/quarantine rules. Experimental deferred records
tagged above `"1"` remain preserved and deferred, without silent retagging.

Old experimental server queue heads with minimum `"2"` are not converted by the
server. They can block V1 delivery until expiry or processing by a separately
approved compatible build. This task does not purge or bypass them. Previously
stored message references, reactions and tombstones remain readable.

## Final clean-install preparation (separate authorization)

1. Keep both test clients on this same baseline; do not distribute older
   experimental V1 builds lacking the complete content contract.
2. Review pending experimental relay/control/deferred work before choosing to
   retain it, drain it or retire the two test accounts. Relay acceptance alone
   does not establish recipient application.
3. If a reset is approved later, treat authentication identity, server account
   and bound Signal store as one lifecycle. Never reuse an enrolled VOID-ID with
   a newly generated Signal store. Retire server accounts only through the
   authenticated deletion protocol; preserve identity-conflict protection.
4. Use fresh isolated profiles/accounts for clean-install registration,
   enrollment, restart and two-party checks. Clean up only designated fixtures.
   No reset is required merely because the Signal codec retains `v: 2`.

No production deployment, backend configuration change or personal-account
modification is included. Future incompatible releases retain mandatory-update
handling and require a separate contract/release review.

## Verification (19 November 2026)

- Normal `cargo check --locked --offline`, `cargo build --locked --offline` and
  `cargo test --locked --offline` passed without feature flags: 135 passed,
  0 failed, 8 opt-in fixtures excluded from the routine suite.
- Changed Rust files passed `rustfmt --check`; `git diff --check` passed.
- The separately provisioned fixtures used unchanged Go `9eb1319`, fresh
  disposable PostgreSQL clusters on loopback, and synthetic accounts 101/202.
  No production endpoints or saved personal accounts were used.
- Minimum-V1 checks passed for both Slint frontends, ordinary messages/replies,
  verified receipts, reaction add/remove, local/remote deletion, recipient offline
  delivery, reconnect, Signal restart persistence, saved reply drafts, and pending
  control recovery. Separate sender/receiver scenarios verified lost acceptance,
  exact ciphertext reuse, duplicates, revision ordering, failed-COMMIT ACK timing
  and recovery of exact deferred plaintext without another Signal decryption.
- The same normal client received Update Required at disposable backend minimum
  V2, exposed no messaging session and stopped reconnecting. Mock/UI tests retain
  future-version validation and unchanged account/storage-state checks.
- Native Slint fixture rendering passed at 850×560, 1080×680, 1280×720 and
  1600×900, including action menus, reaction picker and readiness explanations.
- Every disposable relay/database fixture was stopped and its cluster/password
  files removed. Final live-account reset and clean-install registration remain
  separately authorized release-preparation work.

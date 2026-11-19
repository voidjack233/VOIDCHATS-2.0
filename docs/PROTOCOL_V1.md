# Initial messaging protocol V1

Review baseline: backend `52fb98f`, coordinated with the Rust initial-V1 contract
at `c08ed21` on 19 November 2026. VOID Native is unreleased. Text, replies,
reactions, Delete for Everyone and encrypted delivery receipts all belong to the
initial application protocol **1**. The prerelease plan to activate these features
as application V2 is retired. The existing Go implementation already supports
this baseline; no runtime, schema, migration or configuration change is needed.

## One opaque delivery path

Native clients authenticate, advertise `{"type":"protocol","version":"1"}`,
and use ordinary `send`, `accepted`, `event`, `ack` and `ack_ok` frames. All these
frames retain the exact string-field contract in [MESSAGING_API.md](MESSAGING_API.md).
Every ordinary send is persisted with minimum recipient protocol version 1.

| Operation | Go behavior | Native responsibility |
| --- | --- | --- |
| Text and replies | Queue/forward opaque bytes | Encrypt/decrypt text and validate/persist optional author/event references |
| Reaction add/remove | Same send/event path | Validate target, emoji, reactor, explicit state and monotonic revision |
| Delete for Everyone | Same send/event path | Authorize original sender and persist/apply a tombstone |
| Delivery receipts | New opaque event in the reverse direction | Authenticate receipt contents and update local delivery state |
| Delete for Me | No relay event | Hide locally while retaining replay/deduplication evidence |

Go never identifies these feature types, inspects Signal plaintext, parses emoji
or reply targets, or authorizes message deletion. A queue ACK removes a recipient's
transport record; it is separate from Delete for Everyone and from an encrypted
delivery receipt. Relay acceptance confirms committed queue storage, not peer
receipt or application of a reaction/deletion.

Queued ciphertext survives disconnects and is offered again on reconnect. The
unchanged order is `(created_at,sender_void_id,event_id)`, with one in-flight event
per recipient until ACK or expiry. This is not a cryptographic causal sequence or
global commit order. Queued duplicate sender/event IDs return `duplicate_event`
without overwriting bytes or extending the 30-day deadline. IDs are not retained
after ACK/expiry: retry acceptance remains uncertain and exactly-once delivery is
not promised. Native retains committed ciphertext/UUIDs for retries and durable
local deduplication. The backend adds no automatic retransmission policy.

## Version mechanisms retained for future releases

`VOID_MIN_PROTOCOL_VERSION` continues to default to 1. For a genuine future
incompatible application release, a separately authorized coordinated rollout can
raise that minimum. All old backend processes/connections must be replaced; each
new connection must negotiate sufficient support before messaging is enabled.
Outdated or non-negotiating command attempts receive `update_required` when
writable and disconnect. An idle unconfirmed connection cannot receive messages.
There is no peer-capability handshake, version table or account-persisted version.
The server trusts client advertisements; this is not binary attestation.

`send_versioned`, `event_versioned`, `messages.min_protocol_version` and the
head-of-queue compatibility guard are unnecessary for dispatching the initial
feature set, but remain generic future-version support. Removing them now would
break existing transport/storage contracts and discard protection for already
queued experimental ciphertext. Incompatible heads must remain queued and block
later events until compatible processing or the original expiry; never retag,
purge or skip them as part of this consolidation. A current V1 client must not
claim V2 support merely to drain such a head.

Mandatory updates and per-event metadata serve different purposes: the minimum
gates access to the service; event metadata records the eligibility of each
queued ciphertext. Removing the latter in a future redesign would require the
global minimum to protect every remaining queued event, including during rollback
and mixed-build operation. Retaining both avoids changing that recovery contract.
The preserved version-2 examples describe a future transport envelope, not a
release requirement for replies, reactions or deletion.

## Rust and Go contract differences

- Rust advertises exactly V1 on every connection and waits for `protocol_ok`
  before exposing messaging. Go also accepts legacy clients without negotiation
  when its minimum is 1. At that minimum, Go can offer a V1 queue head immediately
  after `auth_ok`; Rust explicitly buffers that single delivery until confirmation.
- Go accepts canonical positive versions through `2147483647`. The current Rust
  receiver ceiling is application V1; its normal sender does not emit future
  versioned feature events and rejects sends above its advertised ceiling.
- Rust's Signal envelope/text/control JSON codec retains `v: 2`, while encrypted
  delivery receipts retain `v: 1`. These are payload codec revisions, separate
  from application negotiation V1 and the `authenticate:v2` proof transcript.
- Rust implements encrypted receipts, persistent outboxes, reaction revisions,
  deduplication, tombstones and application retry policy. Go provides opaque
  committed acceptance, forwarding, recipient ACK deletion and expiry. Go does
  not synthesize sender-facing delivered notifications or control receipts.
- Go retains the optional ephemeral `relay_online` transport. It is not used for
  ordinary native Signal messages or feature controls; Rust ignores those setup
  frames outside its queued Signal path.

These differences are intentional and compatible with the initial V1 baseline.
No Rust code is changed by this backend review.

## Future reset and schema implications

No reset is authorized here. The review does not inspect live account counts,
modify data, delete accounts/messages, replay migrations, restart services, or
change the minimum. Historical deployment counts remain historical records.

A separately authorized clean development reset could remove experimental queued
version-2 heads as part of its explicitly approved data/lifecycle plan. It would
not by itself make a column or table obsolete. The current PostgreSQL adapter and
strict validator require the minimum-version column, default, CHECK and existing
queue indexes. Dropping it requires coordinated adapter/interface/validator/test
changes and a separately reviewed schema plan. A reset only removes the need to
convert old rows; it does not remove those code dependencies or the future-upgrade
tradeoff above. Account identities, deletion reservations and bound native Signal
stores also require a coherent reset policy; do not reuse an enrolled identity
with regenerated keys by accident.

There are no reply/reaction/deletion feature tables or device-version tables to
remove. Public prekey inventory/claims remain part of Signal setup, and ordinary
queue routing, deduplication keys, timestamps and expiry indexes remain necessary.
No schema removal is proposed in this consolidation.

## Isolated verification

The new loopback WebSocket fixture uses two synthetic accounts and opaque binary
fixtures for text, reply, reaction add/remove, deletion and reverse receipts. It
checks offline queuing, implicit version 1 metadata, exact legacy frame shapes,
ordered replay after a lost ACK, immutable duplicate retries, recipient-only ACKs
and queue removal. Payload labels exist only in the test, never in relay fields.
These are transport fixtures, not libsignal interoperability vectors.

The existing isolated tests also cover authentication failure, impersonation,
unsupported queue heads, minimum-V2 enforcement, fresh reconnect negotiation,
ACK idempotence and exact 30-day expiration boundaries. PostgreSQL tests remain
opt-in; this review does not run migrations or connect to any database. Native
cryptography and feature semantics are covered on the Rust side of the contract.

Review verification passed `go test ./...`, `go test -race ./...` and
`go vet ./...`. The guarded PostgreSQL integration tests were skipped. Only
synthetic stores and loopback fixture servers were used; production was untouched.

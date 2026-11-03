# Temporary encrypted event relay API

Status: activated on 2026-11-02 after the owner explicitly authorized the reviewed migration. The database now has accounts/messages and the two approved Signal prekey tables; existing accounts and keys were preserved. See [deployment evidence](MESSAGING_DEPLOYMENT.md). Registration remains HTTP; ordinary messaging uses authenticated WebSockets at `/dm9pZG5vdGhpbmduZXNz/ws`. No conversation, request, package-publication/claim, HTTP messaging session or HTTP sync/ACK API is mounted.

VOID-IDs are canonical decimal positive int64 strings. They are public addresses, never authentication credentials. Event IDs are lowercase nonzero UUID strings, scoped by authenticated sender. Opaque bytes use canonical padded standard base64. Every frame is one JSON object with exactly its documented string fields; duplicate, unknown, null, numeric and case-variant fields are rejected. Timestamps in this document are illustrative server values, not client inputs.

## Authenticate and locate

Use the Ed25519 `auth_begin` → `auth_challenge` → `auth_proof` → `auth_ok` flow documented in [WEBSOCKET.md](../WEBSOCKET.md). Production proofs bind the registered public key, VOID-ID, canonical audience, challenge, expiry and nonce. One active connection is maintained per account. Authentication must complete before commands or pending delivery.

```json
{"type":"lookup","void_id":"202"}
```

```json
{"type":"person","void_id":"202","auth_public_key":"<padded-base64 registered public key>"}
```

Registration accepts no username, and usernames are not used for routing or messaging. There is no username search. Devices validate/pin the discovered peer key according to their own cryptographic identity verification policy.

## Store and send

The sender chooses an event UUID. It supplies no sender field, timestamp, expiration, conversation, MLS state or message kind:

```json
{"type":"send","event_id":"00000000-0000-4000-8000-000000000001","to_void_id":"202","payload_base64":"AAECAw=="}
```

Go reads its clock once, sets `created_at` in UTC and `expires_at = created_at + 30 * 24 hours`, then persists the row. Only after PostgreSQL commits does it reply:

```json
{"type":"accepted","event_id":"00000000-0000-4000-8000-000000000001","created_at":"2026-11-02T06:00:00Z","expires_at":"2026-12-02T06:00:00Z"}
```

Acceptance means temporary server persistence, not delivery, decryption or application processing. The authenticated connection supplies the sender. A registered recipient may be offline; KeyPackage availability is never consulted. A queued duplicate `(sender_void_id,event_id)` returns `duplicate_event` without overwriting payload or extending retention. The server does not implement a replay-safe retry protocol after deletion.

The current recipient connection receives:

```json
{"type":"event","event_id":"00000000-0000-4000-8000-000000000001","from_void_id":"101","payload_base64":"AAECAw==","created_at":"2026-11-02T06:00:00Z","expires_at":"2026-12-02T06:00:00Z"}
```

The payload is forwarded unchanged. The synthetic bytes above are not a valid MLS object or an encryption format. Encrypted edits, deletions, reactions and other device instructions use this identical opaque mechanism; the backend does not parse their meaning.

## Recipient acknowledgment and deletion

After safely processing an event, its authenticated recipient sends:

```json
{"type":"ack","sender_void_id":"101","event_id":"00000000-0000-4000-8000-000000000001"}
```

The server deletes only the matching sender/event addressed to the authenticated recipient. After deletion commits:

```json
{"type":"ack_ok","sender_void_id":"101","event_id":"00000000-0000-4000-8000-000000000001"}
```

The row and ciphertext are removed from the live queue. No delivered column, receipt, history or tombstone remains. An ACK from a different account cannot delete the intended recipient's event. A missing/previously deleted event also returns `ack_ok`; that response asserts no historical delivery outcome and does not reveal another account's queue.

There is no sender-facing delivered notification or reverse encrypted receipt lifecycle in this implementation.

## Online opaque protocol relay

Both accounts first authenticate to the relay. Devices can exchange opaque protocol objects locally, including dormant/future OpenMLS group setup:

```json
{"type":"relay_online","exchange_id":"setup-1","to_void_id":"202","payload_base64":"AAECAw=="}
```

The online peer receives:

```json
{"type":"online_event","exchange_id":"setup-1","from_void_id":"101","payload_base64":"AAECAw=="}
```

The sending connection receives a transport result:

```json
{"type":"online_result","exchange_id":"setup-1","status":"forwarded"}
```

Other statuses are `unavailable`, `overloaded` or `failed`. The printable ASCII exchange ID is 1–64 bytes from letters, digits, `.`, `_`, `:`, `-`. It is a correlation field, not a retained idempotency key.

These objects are never inserted into the queue, inventoried, claimed or interpreted as completed MLS initialization. Go never handles MLS private keys. A recipient disconnect or failed write requires the clients to decide how to recover; no automatic retransmission is implemented. Devices determine cryptographic readiness. Once established, ordinary encrypted events can use offline delivery without a new package.

## Reconnect, retention and known limitations

After successful authentication, the server offers the first remaining unexpired event, ordered by `(created_at,sender_void_id,event_id)`. One event is in flight per recipient connection. Its ACK or expiry opens the delivery window. The server wakes this pump after committed sends and periodically checks for new work; it never advances a permanent sync cursor.

Unacknowledged events remain after a transport write. Reconnection offers them again. There is no resend/backoff timer, persistent deduplication or exactly-once guarantee. A lost recipient ACK can hold the window until reconnect or expiry. A lost sender acceptance response is ambiguous: the event may already be queued, processed or deleted. After deletion, the same ID can be accepted again with new server timestamps. Devices must handle duplicate processing safely; advanced retry/dedup engineering remains future work.

The ordering above is a queue scan order, not a global commit order or MLS causal sequence. Devices own encryption/application ordering and their local history. Removing server conversations means the backend cannot prove an encrypted payload belongs to an established MLS group; that is a client responsibility.

Expired records are never dispatched. An in-process task deletes expired rows on startup and each minute in bounded batches. No external worker/service is introduced. Outages or a cleanup backlog can delay physical deletion, and database MVCC/WAL/backups have separate retention. Thirty days is the maximum delivery eligibility; no promise of exact wall-clock disk erasure is made.

## Limits and errors

- Decoded payload: at most 1 MiB for queued and online frames; configurable lower.
- Pre-authentication frames: at most 8 KiB; authenticated frames allow the configured base64 payload plus bounded envelope overhead.
- Default admitted connections: 256, including unauthenticated sockets; one active socket per account.
- Socket writer queue: bounded count and 2 MiB encoded-byte budget including the active write.
- Queued ciphertext: at most 1,000 pending events and 16 MiB each per sending and receiving account, enforced transactionally. ACK/expiry frees capacity.
- Account command rate: 120/minute with burst 40, tracked in a bounded 4,096-account map across reconnects. Rate state is process-local.
- Concurrent queue/command work: 64 slots, with database/network deadlines.

Errors use `{"type":"error","code":"..."}`. Codes include `invalid_frame`, `unauthorized`, `authentication_failed`, `not_found`, `duplicate_event`, `queue_full`, `rate_limited`, `overloaded` and `unavailable`. Protocol/authentication errors may close the socket. Database and private connection diagnostics are not exposed.

These bounds do not prevent fresh-key bot-account farming or provide persisted per-peer blocking. The target is one logical connected device per account; multi-device delivery state is not implemented.


## Signal public prekeys

Authenticated `prekeys_publish` and `prekeys_claim`, correlated responses/errors,
public payloads, fixed 24-hour claim expiry and proposed schema are documented in
[SIGNAL_PREKEYS.md](SIGNAL_PREKEYS.md). These additions are active after approved migration/deployment on
2026-11-03 11:36 (Asia/Manila). Established Signal ciphertext
uses the same `send`/`event`/`ack` contract above. The recipient may be offline
when its previously published public bundle is claimed. No existing frame or
account-authentication transcript changes.

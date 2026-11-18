# Encrypted event receiver

This defines the prepared Rust application-version-2 receiver. The authenticated
relay still negotiates version **1**; feature sending and Slint UI are unchanged.
Tests pass versioned frames directly into the same Signal/storage receive path.
The relay will not offer version-2 queue heads to this release.

## Wire contract

The outer signed-account-binding/Signal ciphertext envelope is unchanged. All
plaintexts correlate `client_message_id`, `from_void_id`, and `to_void_id` with
that envelope and the relay sender/event. Application version is distinct from
the plaintext's `v` and the authentication transcript version. Relay minimum
version metadata is not an authorization or a substitute for Signal verification.

Ordinary messages preserve their existing `v: 2`, `body`, `created_at`, and
optional `request_delivery_receipt: 1`. A reply adds only:

```json
"reply_to": {"author_void_id": "101", "event_id": "00000000-0000-4000-8000-000000000001"}
```

The reference uses the original author's VOID-ID and original relay event UUID,
not the local message row ID or the reply's author. Missing originals are allowed.
References must address one of the conversation's two participants, must not
address the event itself, and cannot resolve to another peer's conversation.
Existing originals resolve through inbox/outbox mappings, including legacy IDs.

Reaction plaintext uses the common routing fields above, `v: 2`, and:

```json
{
  "type": "reaction",
  "target": {"author_void_id": "101", "event_id": "00000000-0000-4000-8000-000000000001"},
  "emoji": "👍",
  "active": true,
  "revision": 1
}
```

`active` is an explicit Boolean. Revisions are positive signed-64-bit integers,
monotonic per target, authenticated reactor, and exact emoji token. Higher
revisions replace lower ones; stale revisions do nothing. Equal revision/state
is idempotent; equal revision with contradictory state is durably deferred.
Inactive removal rows retain their revision high-water marks across restarts.
Emoji tokens are bounded Unicode strings; full emoji choice validation and the
10-distinct-emoji interaction limit belong to future sender/picker work. Receiver
state is retained rather than dropped when concurrent offline choices exceed a
future UI limit.

Delete-for-Everyone plaintext uses the common routing fields, `v: 2`, and:

```json
{
  "type": "delete_for_everyone",
  "target": {"author_void_id": "101", "event_id": "00000000-0000-4000-8000-000000000001"}
}
```

Only the authenticated original author may delete that message. The recipient
cannot use this event to delete the local user's outgoing message. No Delete for
Me implementation is added in this receiver phase.

Replies and controls require application minimum version 2. A valid extension
received in a legacy frame is durably deferred as `protocol_version_required`.
Existing ordinary text and version-1 delivery-receipt plaintexts remain supported
with their existing encodings and meanings.

## Atomic persistence and ACKs

Migration 10 is additive. It introduces reply references, reaction high-water
state, permanent deletion tombstones, a control-event inbox, and deferred
plaintext records. It does not rewrite old messages, account credentials, Signal
keys, drafts, or timeline cursors during migration.

The message, receipt, and control inboxes share a sender/event duplicate check.
Matching duplicates return successfully without decrypting twice. Changed
ciphertext under an existing event ID fails. All new effects and deduplication
records commit in the existing SQLCipher transaction with Signal ratchet writes,
prekey consumption, and identity pinning. Failed writes or COMMIT roll everything
back; only a successful result authorizes the worker's existing relay ACK.

Controls do not create message bubbles, empty Requests, unread markers, or
delivery receipts. Reply messages follow normal message/receipt behavior.

Tombstones are peer/author/event scoped and have no target foreign key, so a
delete arriving before its message persists safely. Existing target bodies are
cleared, unread markers removed, and reactions deactivated. Message rows,
timestamps, inbox digests, reply references, and receipt records remain intact.
An original arriving later is inserted with a blank body and without a new unread
marker. Redelivery and later reactions cannot resurrect deleted content. Future
UI work must render the tombstone as a deleted-message placeholder and resolve
quoted originals as deleted or unavailable, without storing copied body previews.

## Authenticated unsupported content

Unknown event types, future positive plaintext versions, and otherwise-valid
known events with unsupported fields are deferred only after successful Signal
decryption, account/identity validation, and matching plaintext routing. Exact
bounded plaintext bytes, sender/event, ciphertext digest, relay minimum version,
reason, and local receive time are durably retained in SQLCipher. These records
can be acknowledged without stranding later Signal messages. They are not
rendered as ordinary messages and do not assert delivery of an unknown message.

Malformed known events, invalid routing/references, unauthorized deletion,
duplicate JSON keys, invalid ciphertext, and versions above the prepared receiver
ceiling fail closed without ACK. The receiver never interprets a failed
decryption as an unsupported event. No private key or plaintext is logged.

There is no deferred-event replay worker yet. A future upgrader must process
saved plaintext without decrypting or advancing Signal again, in a transaction
that preserves the original dedup ledger and applies tombstones and peer scope.
Reclassified mis-versioned content and revision conflicts likewise remain saved
until an explicit compatible recovery policy exists.

## Before advertising version 2

Keep transport negotiation at version 1 for this receiver-only release. A later
release must review/freeze the complete version-2 event contract, exercise its
negotiated relay handoff end-to-end, and integrate the feature presentation and
deferred-event recovery policy. Feature sends need their own persistent outgoing
control queue, monotonic revision allocation, retry semantics, and peer
compatibility rules; local receiver support does not prove peer support.

No retention extension or Go/database deployment is introduced. A delete that
expires from the relay before recipient processing is still not guaranteed to
reach that recipient. Tombstones protect only deletions actually received and
committed locally. Production identities and databases are not used in tests.

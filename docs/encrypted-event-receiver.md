# Encrypted event receiver

This is the complete receiver contract for initial application **V1**, including
text, receipts, replies, reactions and deletion. Normal builds negotiate V1;
all these content types use the ordinary opaque relay transport. See the
[baseline and experimental-data notes](protocol-v1.md).

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
10-distinct-emoji interaction limit are enforced by the sender/picker. Receiver
state is retained rather than dropped when concurrent offline choices exceed
the UI limit.

Delete-for-Everyone plaintext uses the common routing fields, `v: 2`, and:

```json
{
  "type": "delete_for_everyone",
  "target": {"author_void_id": "101", "event_id": "00000000-0000-4000-8000-000000000001"}
}
```

Only the authenticated original author may delete that message. The recipient
cannot use this event to delete the local user's outgoing message. Delete for Me is a separate
local-only hide ledger and has no encrypted control event.

Replies and controls are initial application V1 capabilities, processed directly
from ordinary `event` frames. Existing `v: 2` text/control encodings and `v: 1`
delivery-receipt encodings remain unchanged. No minimum version 2 is required.

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
marker. Redelivery and later reactions cannot resurrect deleted content. The
UI excludes tombstoned messages before pagination and preview selection. Deleted
messages leave no bubble, date row, or deletion placeholder. Quotes of deleted
originals are hidden; genuinely missing originals retain the unavailable fallback.
Tombstones and replay evidence remain stored, without copied body previews.

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

## Deferred recovery

The existing storage worker runs local recovery before loading conversation
lists. It scans pending records in bounded keyset pages; an unsupported record
does not stop supported records later in the scan. No service or background
thread is added. Migration 11 adds pending/quarantined recovery state and its
scan index without rewriting saved plaintext or account/Signal material.

Recovery verifies the original sender/event, current pinned account/Signal
binding, local recipient, saved digest ledger, and supported decoder. It uses
the exact saved plaintext, never the original ciphertext or a second Signal
decryption. The local decoder can process valid content previously retained
by the prerelease decoder under minimum V1; that metadata is not authorization.
Saved minimums above the current V1 ceiling remain deferred without retagging.

Each recovered event replaces its deferred ledger in the same SQLCipher
transaction as its message/control effects. Failed writes or COMMIT restore the
saved record and every affected row. Restart retries cannot create another
bubble, advance the ratchet, undo a tombstone, overwrite a newer reaction
revision, or extend the original delivery-receipt deadline.

Still-unknown types, fields, and plaintext versions remain deferred. Equal
reaction revisions with contradictory state remain deferred until they become
stale under a newer revision; recovery never chooses an arbitrary winner.
Malformed, unauthorized, conflicting, or provenance-invalid entries are retained
as quarantined, with a non-sensitive reason, and are not automatically retried.
Storage failures propagate and leave the candidate pending. A crash between a
failed application and quarantine simply causes revalidation next time.

## Sending and release

The [durable outgoing operations](outgoing-encrypted-events.md) and
[Slint actions](message-actions-ui.md) use this contract in normal builds.
There is no separate V2 activation. Future incompatible contracts retain the
mandatory-update mechanism; no peer handshake is introduced.

No retention extension or Go/database deployment is introduced. A delete that
expires from the relay before recipient processing is not guaranteed to reach
that recipient. Tombstones protect deletions actually received and committed.
Production identities and databases are not used in tests.

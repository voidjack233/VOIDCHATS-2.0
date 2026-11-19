# Outgoing encrypted operations

This foundation uses the frozen [receiver contract](encrypted-event-receiver.md).
The [message actions UI](message-actions-ui.md) now connects these operations.
No peer handshake, relay fields, or new Signal implementation is added.
The message author and original event UUID identify a target independently of
pagination, local row IDs, or display names.

## Initial application V1

Normal builds negotiate V1 and enable all completed operations after messaging
initialization. Text, replies, receipts and controls all use the existing opaque
`send`/`event` transport with implicit minimum V1. Signal payload revisions and
strict validation remain unchanged. There is no Cargo feature or peer handshake.
See [protocol-v1.md](protocol-v1.md) for future updates and experimental data.

## SQLCipher migration 12

- `draft_reply_refs` associates a draft with a stable reference. Saving/cancelling
  a reply uses `save_reply_draft`; existing body edits retain the reference.
- `outgoing_reply_refs` captures the reference when the normal outgoing operation
  is queued. Queueing the reply and clearing the submitted draft are atomic.
  Encryption commits `message_reply_refs` with the message, ciphertext, and ratchet.
- `outgoing_controls` persists reaction/deletion intent, event UUID, prekey-claim
  UUID, target, explicit reaction state/revision, committed ciphertext and status.
  It never joins the normal chat-bubble outbox.
- `local_message_hides` is a durable, conversation-scoped Delete for Me ledger.
  Timeline pagination, list previews and quote resolution exclude hidden messages
  before limiting rows. Inbox/outbox evidence remains for deduplication. This is
  logical local removal, not forensic erasure of encrypted database pages.

No original message rows, identities, sessions, receipts or history are rewritten
by migration. Feature operations only change records explicitly targeted by callers.

## Queueing and retries

Reply queueing validates that the original belongs to the conversation. It
preserves normal delivery receipts, retry UUIDs, draft handling and promotion of
Requests. Unsent reply previews resolve from the queued metadata too.

Reaction callers retain a submission UUID across retries. Reusing it for a
different intent fails. Repeating the already-intended state returns its existing
operation; a state change allocates the next revision per target/reactor/emoji in
the same transaction as local reaction state. Overflow fails closed. At most ten
distinct active emoji tokens may be chosen across the conversation participants;
removal and joining an already-present emoji remain allowed at the limit.

Delete for Everyone requires local authorship, queues an encrypted control and
applies the existing tombstone atomically. Target bodies are cleared and reactions
deactivated. Delete for Me hides the item without telling the peer. Both operations
block resending a targeted outgoing message even when its old ciphertext exists.
Neither operation removes replay ledgers. Existing replies resolve deleted or
unavailable targets without retaining copied body previews.

An explicit worker call prepares a missing Signal session using the existing
authenticated lookup/claim path. Claim UUIDs survive restart; expired claims may
be renewed without changing identity. Control ciphertext and Signal ratchet writes
commit together. A failed write leaves the durable intent available for retry and
rolls back the ratchet. Committed ciphertext is reused exactly; a missing ratchet
under committed ciphertext fails closed rather than creating a replacement session.

Before socket submission, control state becomes `unconfirmed` durably. Matching
relay acceptance changes it to `accepted`. Disconnection, lost acceptance or
`duplicate_event` preserves uncertainty. Acceptance never means the recipient has
applied the control. The current protocol defines delivery receipts for messages,
not reaction/deletion controls; no recipient-application acknowledgment is invented.

`pending_controls` recovers unfinished UUIDs after restart. The explicit dispatcher
can resume them with the saved payload. The message-action worker now resumes controls on ready V1 connections
with bounded retries and a manual retry action. Ordinary messages are not
automatically retransmitted. Accepted operations are idempotent no-ops on repeated dispatch.

## Verification and remaining integration

Automated fixtures use two isolated SQLCipher stores and official libsignal
ciphertext. They cover draft/reference persistence, receipts, reaction ordering,
repeated submissions, the emoji limit, unauthorized/cross-conversation references,
write rollback, tombstone-before-original delivery, local hiding and replay safety.
A loopback mock drops a connection after receiving a control but before acceptance;
restart and explicit retry preserve its exact UUID/ciphertext, with no automatic
send after reconnect.

The opt-in `actual_go_outgoing_features_resume_offline_intents_and_preserve_legacy_receipts`
test uses unchanged Go `9eb1319`, minimum V1 and fresh disposable PostgreSQL,
with synthetic accounts 101/202. It checks V1 replies, receipts, offline controls,
ambiguous/duplicate acceptance, restart recovery, reconnect and both deletions.
The actual receiver fixture verifies failed-COMMIT ACK timing, duplicate delivery,
out-of-order reactions and saved-plaintext recovery against the same contract.
Tests require explicitly provisioned loopback fixtures and never use personal data.

The existing 30-day retention remains: expired controls cannot be claimed as
applied. No retention extension, backend change or production activation is needed.

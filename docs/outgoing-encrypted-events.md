# Outgoing encrypted operations

This foundation uses the frozen [receiver contract](encrypted-event-receiver.md).
The [message actions UI](message-actions-ui.md) now connects these operations.
No peer handshake, relay fields, or new Signal implementation is added.
The message author and original event UUID identify a target independently of
pagination, local row IDs, or display names.

## Release gate

The default build still advertises application protocol **1**. `messaging::send` rejects
replies and `messaging::send_control` rejects all feature controls before lookup,
prekey claims, encryption, or transport. There is no environment-variable override.
The explicit `coordinated-v2-release` build and numeric-loopback V2 fixtures
can dispatch them. The feature is an operator coordination choice, not proof
of the deployment minimum; follow the [release runbook](protocol-v2-release.md).
Ordinary messages and delivery receipts continue using the legacy transport.

The chosen compatibility policy is a coordinated mandatory application update.
Before enabling these operations in a release, deploy/enforce backend minimum V2
and release a client that negotiates V2 with this receiver and sender. Negotiating
the sender's own version is not peer-capability discovery. Older recipients must
update before draining versioned queue heads. No peer handshake is implemented.

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
can resume them with the saved payload. The message-action worker now resumes controls on compatible V2 connections
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
test uses the unmodified version-aware Go relay at
`2faffd27f12d1d02ec28e4be298610d4fee50856` and a fresh disposable PostgreSQL fixture,
with synthetic accounts 101/202 only. It checks V1 feature blocking, V2 replies,
encrypted message receipts, offline controls, ambiguous/duplicate acceptance,
restart recovery, reconnection, deletion and continued legacy messaging. It is
ignored in routine runs because it requires this explicitly provisioned environment.
The newer mandatory-minimum response is covered by the existing mock tests, not
by changing this pinned Go fixture or connecting to production.

The Slint hover actions, reply banner, reaction picker and delete choices are
connected to the worker APIs, preserving timeline anchors. Next: coordinate the
mandatory V2 release. Production and
existing account stores were not used for testing. The relay's existing 30-day
retention remains: deletion/reaction events that expire before recipient processing
cannot be claimed as applied. No longer retention, backend change or synchronization
workaround is introduced here.

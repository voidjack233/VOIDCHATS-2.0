# Versioned relay transport

Contract reviewed against backend commit `2faffd27f12d1d02ec28e4be298610d4fee50856`:

- [MESSAGING_API.md](https://github.com/voidjack233/VOIDCHATS-2.0/blob/2faffd27f12d1d02ec28e4be298610d4fee50856/docs/MESSAGING_API.md)
- [VERSIONED_DELIVERY.md](https://github.com/voidjack233/VOIDCHATS-2.0/blob/2faffd27f12d1d02ec28e4be298610d4fee50856/docs/VERSIONED_DELIVERY.md)

Every authenticated messaging socket sends `protocol` with version `"1"` and
waits for a matching `protocol_ok` before becoming available to the messaging
worker. Reconnects repeat this exchange. A legacy event can arrive before the
agreement; one bounded in-flight delivery is buffered and released after the
worker receives its new session. Negotiation has a timeout and fails closed on
malformed, mismatched, or unsolicited replies.

Application versions use canonical decimal JSON strings in `1..=2147483647`.
This version is independent of the authentication transcript and the Signal
payload's existing wire version. Ordinary messages and delivery receipts still
use `send`, legacy `event`, and the existing correlated acceptance and ACKs.
ACKs remain contingent on successful local processing and durable storage.

`event_versioned` is decoded with an explicit minimum version greater than 1.
Its original envelope and version metadata remain available to Signal
processing; the metadata is a relay routing hint, not an authenticated Signal
plaintext field. Transport rejects versions above the advertised version before
worker delivery. The prepared Signal receiver has a separate version-2 ceiling
and rejects higher versions before duplicate acknowledgment, identity pinning,
or ratchet changes. Version 2 remains unadvertised; see
[encrypted receiver contract](encrypted-event-receiver.md). Rejection sends no ACK, leaving recovery to the relay's
retained queue. It does not preserve events beyond the backend's expiry policy.

The typed `SendVersioned` request prepares serialization and acceptance
correlation only. No application feature sends it yet. Sender receive-version
negotiation is not evidence of peer capability and is not a backend prerequisite
for sending. Reply, Reaction, and Delete payloads are not enabled by this change.

Do not raise `SUPPORTED_APPLICATION_PROTOCOL` until the Rust receiver can safely
process or durably defer every encrypted event allowed at that version, including
duplicate and out-of-order events, with correct post-commit ACKs. Feature senders
also need a reliable peer compatibility policy; this relay negotiation does not
provide peer capability discovery. Unsupported queue heads block later events,
including legacy messages, until acknowledgment or expiry.

Deployment requires the matching version-aware backend, migration 008, and the
coordinated relay replacement described in the backend documentation. This
integration does not verify production deployment and does not fall back to an
older backend that rejects `protocol`.

# VOID 2.0 direct messaging

Status: Android source integration, not built or exercised on devices. No tests,
builds, APK generation, migrations or live exchanges have been executed.

## Dependencies and boundaries

Android uses official `org.signal:libsignal-client:0.106.1` and
`org.signal:libsignal-android:0.106.1` from Signal's Maven repository. API usage
follows the [pinned upstream source](https://github.com/signalapp/libsignal/tree/v0.106.1/java).
Both artifacts are required for Android; desktop native resources are excluded
as described in the [official integration instructions](https://github.com/signalapp/libsignal/blob/v0.106.1/README.md).
Upstream declares AGPLv3 licensing.

React Native/TypeScript still owns screens, navigation, authenticated WebSocket
commands and messaging coordination. `MessagingDriver` is platform independent;
the native Signal/SQLCipher implementation exists on Android only. An unavailable
iOS/native module fails explicitly, with no JavaScript or plaintext crypto fallback.

`native/openmls/` has not been edited, moved or deleted. Android no longer builds
its Rust library, includes its generated sources, registers its React module or
depends on JNA. The old Android MLS adapters remain in source but are excluded
from the application source set. Groups have no active implementation.

## Existing account preservation

The registered account's Ed25519 key is separate from the new Signal identity.
`AccountKeyStore` imports the existing Ed25519 seed from the primary client's
versioned encrypted legacy checkpoint, verifies that its derived public key is
the existing account key, and stores it in a native-only SQLCipher table.
It never loads OpenMLS, rewrites its checkpoint, replaces the VOID-ID, or
regenerates an existing account key. Missing or incompatible material fails closed.

New accounts generate an Ed25519 account key natively. Username-free registration
still signs the deployed v1 message with its empty reserved field; WebSocket
authentication still validates the audience-bound v2 challenge. These methods
expose signatures only.

Local migrations 006 and 007 are additive source assets. Existing identity rows,
registration metadata, MLS groups/checkpoints, history and old outbox entries
are retained. Legacy MLS ciphertext is not retransmitted or converted. Existing
saved MLS drafts/history remain local; a new explicit send adopts only the
thread's public peer metadata and enters the Signal draft queue.

## Signal state and identity checks

`SignalStore` persists official serialized identity/session/prekey records as
SQLCipher BLOBs. SQLCipher's random database key remains wrapped by Android
Keystore. No private record enters JavaScript or the relay.

The initial publication has a signed EC prekey, 20 EC one-time prekeys, 20
signed Kyber1024 one-time prekeys and a signed last-resort Kyber1024 prekey.
The initial batch is generated once. Published private prekeys are retained
until libsignal consumes them; last-resort replay tuples are recorded.
Automatic replenishment, rotation and pruning are not implemented.

The account Ed25519 key signs an audience/VOID-ID/device/registration-bound
Signal identity certificate described in the backend proposal. Both prekey
bundles and incoming messages carry that public certificate. Native code checks
it against the registered public key from authenticated VOID-ID lookup before
pinning the Signal identity. Libsignal verifies its own prekeys and messages.
Changed account or Signal keys are rejected; protocol pinning never marks the
contact manually verified.

## Message transactions

Opening a thread does not send an invitation. New incoming plaintext appears in
Requests, with no accept/decline handshake; a reply moves it to Messages.

Before a session exists, outgoing text stays in SQLCipher as a draft. A verified
public prekey bundle creates the official libsignal session. Encryption commits
the updated ratchet and the exact ciphertext/outbox event together. Retries use
that same ciphertext and UUID. Established sessions need no prekey-directory
access or simultaneous recipient connection.

Incoming processing commits session/prekey changes, the crypto journal, visible
message and inbox deduplication row in one transaction. Each callback loads fresh
state from SQLCipher; a rollback does not leave a cached ratchet ahead of storage.
Only successful commit returns the ACK token. Same-ID/same-payload redelivery
returns its committed ACK; conflicting payloads and all failed/ambiguous events
return no ACK and do not overwrite history.

Only backend `accepted` changes outgoing UI state to **Sent to server**.
Recipient delivery receipts remain deferred.

## Backend approval dependency

The existing relay has no public-prekey inventory. A `PreKeyDirectory` interface
and WebSocket adapter are drafted in source, but the adapter is not connected to
the running provider. First-contact drafts remain saved with an explicit pending
message until the [minimal backend extension](signal-prekey-backend-proposal.md)
is approved and implemented. No Go source, server tables or deployment settings
have changed.

Old queued MLS events are unsupported and are not automatically ACKed. Such an
event can continue occupying the relay's one in-flight slot; any cleanup policy
requires a separate decision. Existing 30-day setup-expiration recovery remains
incomplete and is not replaced by a speculative recovery system.

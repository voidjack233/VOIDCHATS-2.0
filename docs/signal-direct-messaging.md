# VOID 2.0 direct messaging

Status: the owner-authorized ARM64 release APK 2.0.0 (code 5) built successfully
on 2026-11-04, overwriting the same version with manual-Resend changes.
Package inspection confirmed Signal JNI is present while dormant
OpenMLS/JNA and the unused Signal testing library are excluded. The owner reported
a successful two-device exchange using the earlier 2026-11-03 APK. The new sender
behavior has not been tested on devices. No database
migrations or deployments were executed by this frontend work.
Subsequent native verification diagnostic changes are source-only and have not
been built or tested.

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

Native verification diagnostics distinguish identity binding, prekey-bundle
processing, encryption, prekey-message parsing/decryption and established-message
parsing/decryption. Invalid message, key, version and legacy-format exceptions
have separate sanitized codes. Persisted identity/session/prekey decode failures
are identified when the exception retains the known store/constructor frames;
otherwise the stage/class code remains the fallback. A decrypt failure alone
does not prove permanent ciphertext invalidity. No raw exception messages,
record contents or plaintext enter these diagnostics. ACK and transaction rules
remain unchanged.

Established-message decryption additionally uses libsignal's public logger API
to capture fixed candidate-failure reason codes during that one decrypt call.
Only allowlisted codes enter the `VoidMessaging` Android log after a failed
decrypt. Native log text, public-key/counter dumps and message contents are
discarded. These are observed rejection reasons, not proof of malicious or
permanently invalid ciphertext; unknown or missing native detail stays unknown.

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

An explicit Send saves locally and starts the upload attempt immediately.
Existing ciphertext is sent before unrelated public-key publication or session
setup. A connection failure, timeout or app interruption changes pending sends
to **Failed to send**. Reconnection leaves those messages alone; **Resend**
re-enables only the selected saved message. An already encrypted message uses
the same ciphertext and event ID. Drafts not yet encrypted retain their local
text and use the normal verified session setup on Resend. A send whose server
acceptance cannot be established is never presented as delivered or confirmed.
The recipient may be offline throughout upload: the backend retains accepted
ciphertext until the existing ACK or expiry.

## Active backend connection

Backend commit `071aaf3` records the matching public-prekey extension as active.
The controller creates its default `PreKeyDirectory` from its existing authenticated
WebSocket client. A registered device publishes its durable public-key payload
even with an empty outbox. First-contact drafts trigger authenticated lookup,
bundle claim, native account/signature verification and Signal encryption.
The encrypted outbox then uses the existing `send`/`accepted` contract.

Unavailable recipient public keys and temporary capacity responses fail the
current send attempt; they do not schedule an automatic resend. New explicit
sends during an active pass request another pass so they cannot miss public-key
preparation. The service still reconnects for incoming delivery and ACKs, without
re-enabling failed outgoing messages. Native commit and ACK conditions are
unchanged. No Go source, server tables or deployment settings were modified by
this frontend activation.

Old queued MLS events are unsupported and are not automatically ACKed. Such an
event can continue occupying the relay's one in-flight slot; any cleanup policy
requires a separate decision. Existing 30-day setup-expiration recovery remains
incomplete and is not replaced by a speculative recovery system.

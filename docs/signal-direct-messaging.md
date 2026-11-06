# VOID 2.0 direct messaging

Status: the owner-authorized ARM64 release APK 2.0.0 (code 5) built successfully
on 2026-11-04, overwriting the same version with manual-Resend changes.
Package inspection confirmed Signal JNI is present while dormant
OpenMLS/JNA and the unused Signal testing library are excluded. The owner reported
a successful two-device exchange using the earlier 2026-11-03 APK. Subsequent
package inspection confirmed User A's installation matches the current workspace
APK, including event-correlation diagnostics; User B has different application
code but the same native libsignal binary. Recipient logs identify an established
message failing with `MAC_MISMATCH`. The historical event's cause remains unproven.
Owner-authorized isolated tests exercised the unchanged Kotlin coordinator and
SQLCipher on a disposable emulator. They did not use the production relay or real
account state. No backend database changes or deployments were performed.

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
Failed established-message decrypt logs also include validated event/routing IDs
and SHA-256 fingerprints of the decoded opaque payload and Signal ciphertext.
The chat menu's explicit **Log send diagnostics** action reads up to 128 saved
outgoing records, including accepted records, to log the corresponding metadata.
It performs no network or cryptographic processing. See the
[two-account correlation handoff](signal-message-correlation.md) for the matching
read-only backend query and collection scope.

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

The clock marks a locally pending send. The outlined check marks **Sent** after
the relay's matching `accepted` response is committed. A failed send shows a
danger-colored exclamation, with a separate warning for uncertain acceptance.

Sender-visible delivery and read receipts remain deferred. The recipient's
queue ACK clears the relay's queued event after durable processing. Its
`ack_ok` confirms queue handling to the recipient. The sender's **Sent**
indicator records relay acceptance.

A future delivery receipt can use the existing opaque `send` relay. The
recipient would durably enqueue a separate Signal-encrypted control event
referencing the original event ID after committing the incoming message. The
sender would verify the receipt's authenticated peer against the original
outgoing message, then record delivery idempotently before ACKing the receipt's
own queue event. The receipt would be stored as delivery metadata, and only
user messages would generate delivery receipts. Clients would need to establish
receipt support before emitting this payload, since existing clients expect a
chat message body.

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

The explicit **Reconnect** and **Retry connection** buttons restart the socket
lifecycle and authenticate a new connection. This invalidates stale controller
operations and lets the relay offer unacknowledged events again. Ordinary
refresh and automatic connection scheduling keep their existing behavior.

Known limitation: a failed encrypted send can be retried after newer exchanges
have advanced beyond libsignal's retained receiving chains. Isolated native
x86_64 tests reproduced this through the unchanged Kotlin coordinator and
SQLCipher: terminate after ciphertext commit, restart, leave that event
unsubmitted, complete five later DH ratchet advances, then Resend its exact bytes.
The old event fails with `MAC_MISMATCH` while fresh traffic still authenticates.
Sixteen newer messages in the same DH chain remain compatible with the held
message. Immediate crash/restart/Resend at the tested transaction boundaries
authenticated successfully; sender state and ciphertext stayed consistent.

This reproduction requires newer exchanges before the held event is submitted;
an already-accepted event occupying the relay's unacknowledged slot normally
blocks those later inbound events. It does not establish the historical event's
cause. Sender fingerprints and event-bound encryption/retry/DH chronology remain
missing; draft creation time is not encryption time. No ordering-policy correction
has been implemented, and failed-event ACK eligibility and Resend bytes are unchanged.

The disposable emulator's ARM64-translated fixture repeatedly failed fresh
prekey decryption, while the native x86_64 fixture succeeded. Its underlying reason
was unknown and no fixture SIGSEGV was observed. Without an isolated native ARM64
control or the original translated stack mapping, neither the historical MAC
failure nor the original crash can be attributed to the translator. The fixture
omits React Native/Hermes and used Kotlin 2.2.10 with byte-identical storage source
snapshots, libsignal 0.106.1 and SQLCipher 4.19.1. Local evidence is retained under
`build/diagnostics/`; it is not included in source control.

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

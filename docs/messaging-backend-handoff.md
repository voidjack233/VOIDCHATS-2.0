# Current backend contract for the Android client

Checked read-only on 2026-11-03 against backend commit
[`219847aa1bc94e0bcbbd1f8f0d27c21943691d7c`](https://github.com/voidjack233/VOIDCHATS-2.0/tree/219847aa1bc94e0bcbbd1f8f0d27c21943691d7c).
This replaces the earlier HTTP messaging proposal. The backend's
[activation record](https://github.com/voidjack233/VOIDCHATS-2.0/blob/219847aa1bc94e0bcbbd1f8f0d27c21943691d7c/docs/RELAY_VALIDATION.md)
records the relay deployment and subsequent username-free registration activation.
This client work does not change backend records, schema or deployment settings.

The canonical API audience is
`https://api.voidchats.xyz/dm9pZG5vdGhpbmduZXNz`.
Registration uses HTTPS; lookup and messaging use
`wss://api.voidchats.xyz/dm9pZG5vdGhpbmduZXNz/ws`.

## Registration

`POST /challenges` takes exactly `{"auth_public_key":"<32-byte padded base64>"}`.
It returns `challenge_id`, `message` and `expires_at`.
The exact decoded UTF-8 registration proof is:

```text
VOID-NOTHINGNESS:register:v1\n
<challenge_id>\n
<expires_at as returned>\n
<registered public key in padded standard base64>\n
\n
<32-byte nonce in unpadded base64url>\n
```

Each displayed `\n` denotes one newline byte. The empty reserved line remains part
of v1 signing bytes. `POST /accounts` takes exactly `challenge_id` and `signature`
and returns `void_id` and `created_at`. Supplying `username`, including an empty
string or null, is rejected. Existing account names and key bindings remain stored
on the backend; they are not returned by these APIs.

## Connection authentication

The socket handshake is:

```json
{"type":"auth_begin","void_id":"101"}
{"type":"auth_challenge","challenge_id":"<token>","message":"<base64>","expires_at":"<UTC RFC3339Nano>"}
{"type":"auth_proof","challenge_id":"<token>","signature":"<64-byte padded base64>"}
{"type":"auth_ok","void_id":"101"}
```

The production challenge's exact decoded bytes are:

```text
VOID-NOTHINGNESS:authenticate:v2\n
<challenge_id>\n
<void_id>\n
<registered public key in padded standard base64>\n
<expires_at as returned>\n
<canonical HTTPS API audience in padded standard base64>\n
<32-byte nonce in unpadded base64url>\n
```

Before signing, validate domain/version, field count, final newline, canonical
challenge ID and nonce, expected VOID-ID, existing protected public key, exact
expiry and expected API audience. The production challenge lifetime and socket
authentication deadline share the configured timeout, defaulting to 10 seconds
and bounded to 1–10 seconds. The general authentication library caps challenges
at 30 seconds. Preserve original expiry text when checking signed bytes because
RFC3339Nano can contain nanoseconds. Only the protected native account key signs.
Do not accept the legacy v1 socket challenge or former `auth:v1` HTTP proposal.
`auth_ok` proves connection ownership; it does not establish an MLS conversation
or peer trust.

Every reconnect repeats authentication. The backend supports one active connection
per account; successful replacement closes the old connection.

## Authenticated frames

Every frame is one JSON object with exactly the documented string fields. VOID-IDs
are canonical positive decimal int64 strings. Keep them as strings throughout.
Payloads use canonical padded standard base64 and decode to at most 1 MiB.

| Client command | Server response / peer event |
| --- | --- |
| `{"type":"lookup","void_id":"202"}` | `{"type":"person","void_id":"202","auth_public_key":"<32-byte base64>"}` |
| `{"type":"relay_online","exchange_id":"setup-1","to_void_id":"202","payload_base64":"<opaque base64>"}` | Sender: `{"type":"online_result","exchange_id":"setup-1","status":"forwarded"}`. Recipient: `{"type":"online_event","exchange_id":"setup-1","from_void_id":"101","payload_base64":"<same base64>"}`. |
| `{"type":"send","event_id":"<UUID>","to_void_id":"202","payload_base64":"<opaque base64>"}` | Sender: `{"type":"accepted","event_id":"<UUID>","created_at":"<UTC timestamp>","expires_at":"<UTC timestamp>"}`. Recipient: `{"type":"event","event_id":"<UUID>","from_void_id":"101","payload_base64":"<same base64>","created_at":"<UTC timestamp>","expires_at":"<UTC timestamp>"}`. |
| `{"type":"ack","sender_void_id":"101","event_id":"<UUID>"}` | `{"type":"ack_ok","sender_void_id":"101","event_id":"<UUID>"}` |

`exchange_id` is 1–64 ASCII characters from `[A-Za-z0-9._:-]`. Other
`online_result` statuses are `unavailable`, `overloaded` and `failed`.
`forwarded` means a socket write completed, with no promise that the peer
processed or persisted the object. The direct-messaging client uses durable
`send` events for automatic setup instead of depending on this ephemeral route.
The composer is available immediately; native code queues text locally while
the necessary MLS state is being established.

Event IDs are lowercase nonzero UUID strings scoped by sender. Go supplies the
sender from authentication, stamps creation time once, and sets expiry to 30 days
later. Clients supply no sender, timestamps, kind, conversation ID or MLS state
as routing fields. The encrypted payload may contain the application envelope.

Lookup returns a public routing/key association. It must not mark a contact
trusted. Both KeyPackage and Welcome validation must establish the peer's MLS
identity/signing-key binding against that registered key locally.

## Persistence and acknowledgment

`accepted` is emitted only after queue persistence commits. Show **Sent to server**;
there is no delivered or read receipt. Recipient `ack` physically deletes the
queued ciphertext. Send it only after native processing and the visible local
record have safely committed. For a setup event, its session transition, native
checkpoint and any required reply must be committed before ACK. No visible
invitation or approval is required. Native checkpoint/journal recovery must finish an
interrupted local write before acknowledging a replayed event.

The server retains unacknowledged, unexpired events and offers them again after
reconnect. One event is in flight per recipient connection until ACK or expiry.
A lost ACK can stall that window until reconnect. ACK of an absent event returns
`ack_ok`, which provides no historical delivery assertion.

A duplicate `(sender,event_id)` that is still queued returns `duplicate_event`
without comparing or overwriting its payload. After deletion the same ID can be
accepted again. A lost `accepted` response is ambiguous. Do not silently convert
`duplicate_event` into successful acceptance or re-encrypt a saved outgoing
message. Any explicit resend must use the exact persisted ciphertext and rely
on local duplicate handling; the backend does not promise exactly-once behavior.

Errors have exactly `{"type":"error","code":"..."}` and no correlation ID.
Serialize client commands that await responses, while processing unsolicited
`event` and `online_event` independently. Codes include `invalid_frame`,
`unauthorized`, `authentication_failed`, `not_found`, `duplicate_event`,
`queue_full`, `rate_limited`, `overloaded` and `unavailable`. Invalid protocol or
authentication can close the connection.

There are no server conversation records, request records, KeyPackage inventories,
claims, bearer sessions, HTTP messaging/sync/ACK routes or delivery receipts.
The updated client carries initial public MLS objects and confirmations through
the existing durable opaque event queue. It automatically generates and validates
KeyPackages, processes Welcome objects and releases locally pending messages
after the native group is usable. No extra backend route or deployment change is
part of this update. First actual messages from an unfamiliar peer are classified
as Requests locally; replying promotes the thread without an acceptance action.

This removes the simultaneous-online requirement, but setup still takes several
queued exchanges. If Alice sends the first message while Bob is offline, Bob can
later provide a KeyPackage. Alice must reconnect after that response before its
pending text can be encrypted and sent. With no previously available Bob
KeyPackage, the server cannot already hold that first application ciphertext.
One-step offline first delivery needs a public KeyPackage publication/claim
extension; see the [separate proposal](offline-first-bootstrap.md). An established
conversation continues receiving queued ciphertext after reconnect without a
fresh KeyPackage.

Primary references:
[frame contract](https://github.com/voidjack233/VOIDCHATS-2.0/blob/219847aa1bc94e0bcbbd1f8f0d27c21943691d7c/docs/MESSAGING_API.md),
[authentication](https://github.com/voidjack233/VOIDCHATS-2.0/blob/219847aa1bc94e0bcbbd1f8f0d27c21943691d7c/internal/auth/service.go),
[command handler](https://github.com/voidjack233/VOIDCHATS-2.0/blob/219847aa1bc94e0bcbbd1f8f0d27c21943691d7c/internal/relay/queue.go),
[registration](https://github.com/voidjack233/VOIDCHATS-2.0/blob/219847aa1bc94e0bcbbd1f8f0d27c21943691d7c/internal/account/service.go).

# Durable one-to-one messaging API

This describes the durable API deployed from source commit `bad8a72`. See
[the verification record](MESSAGING_VALIDATION.md) for the exact live checks and
[the deployment instructions](MESSAGING_DEPLOYMENT.md) for operation and restarts.

The canonical production API audience is
`https://api.voidchats.xyz/dm9pZG5vdGhpbmduZXNz`. Paths below are relative to that
base. Registration remains `POST /challenges` and `POST /accounts`, with its
existing payloads and account keys unchanged.

## Transport and encoding

Use HTTPS and `Authorization: Bearer <access_token>` for every messaging route
except `POST /auth/challenges` and `POST /auth/sessions`. Put the token only in
that header; a VOID-ID is not a credential. Bearer possession grants access, so
clients must protect the token and validate server TLS certificates. This uses
the header transport described by [RFC 6750, sections 2.1 and 5](https://www.rfc-editor.org/rfc/rfc6750.html#section-2.1).

POST and PUT bodies use `Content-Type: application/json`. Unknown or repeated
fields, null values, trailing JSON, noncanonical encodings and oversized input
are rejected. VOID-IDs, conversation sequences and sync cursors are decimal JSON
strings: never convert them through a JavaScript `Number`. VOID-IDs are positive
signed 64-bit integers with no leading zero. Other resource and stable client
IDs are UUID strings. Opaque bytes use padded standard base64, not base64url.
Times are RFC3339 UTC strings, possibly with fractional seconds.

The acting account always comes from the verified session. Senders cannot supply
a different sender, key binding or account identity in a payload. Usernames are
display names and are not unique. Account key rotation, account recovery and
multiple devices sharing one identity are outside this API.

## Client encryption responsibilities

The backend stores and routes opaque bytes. It does not encrypt text, possess
private MLS keys, validate KeyPackage signatures, parse an MLS epoch, or prove
that uploaded bytes are encrypted. Backend tests using synthetic opaque bytes
test transport and persistence, not an MLS encryption implementation.

Before creating a group, the native client must validate the actual KeyPackage
and its signing key against the public account key. KeyPackages are intended for
one use; the client must perform the MLS validation described in
[RFC 9420, section 10.1](https://www.rfc-editor.org/rfc/rfc9420.html#section-10.1).
Before accepting a Welcome, verify the expected group, exactly two members,
the local credential/key and the expected peer key. The native MLS credential
is a device UUID, not a VOID-ID; pin the verified peer credential rather than
assuming those identifiers are interchangeable.

The native application JSON envelope has a 65,536-byte plaintext limit. Enforce
that locally before encryption; the server's 1 MiB decoded wire-object limit
cannot validate a plaintext size hidden inside ciphertext.

Persist each outbound stable client ID and its exact ciphertext together with
the native MLS checkpoint before attempting delivery. Retry those bytes after
a lost response; encrypting the text again under the same ID causes a conflict
and can corrupt client ratchet state. Process inbound events in order and commit
the native checkpoint, inbox deduplication and local history before ACK. The
server cannot make native state durable on a client's behalf.

## Authentication

`POST /auth/challenges` challenges an existing registered identity:

```json
{"void_id":"110559210000000000"}
```

`201 Created`:

```json
{"challenge_id":"<43-character unpadded base64url token>","message":"<padded standard base64 of exact bytes to sign>","expires_at":"2026-11-02T06:02:00Z"}
```

Decode `message` and sign the resulting bytes verbatim using the account's
Ed25519 private key. Before signing, verify this exact UTF-8 layout and all
bindings. Each line ends with one LF byte, including the last line:

```text
VOID-NOTHINGNESS:auth:v1
<challenge_id>
<expires_at, exactly as returned>
<void_id>
<padded standard base64 registered Ed25519 public key>
<padded standard base64 UTF-8 canonical API audience>
<independent 32-byte nonce, unpadded base64url>
```

The challenge expires after two minutes and is consumed atomically before proof
verification. A well-formed failed proof cannot be retried. Get a fresh challenge
after failure. Challenges are process-local and are lost on restart; durable
sessions survive a restart. Multiple server replicas need challenge routing to
their originating process before they can share the handshake safely.

`POST /auth/sessions`:

```json
{"challenge_id":"<challenge_id from the response>","signature":"<padded standard base64 of 64-byte Ed25519 signature>"}
```

`201 Created`:

```json
{"access_token":"<43-character unpadded base64url token>","token_type":"Bearer","expires_at":"2026-11-02T06:30:00Z"}
```

Tokens contain 32 random bytes. Only their SHA-256 digests are persisted. They
expire after 30 minutes; there is no refresh token. Authenticate again using the
same registered identity when necessary, without registering another account.
There are at most eight active sessions per account. A lost successful response
can leave an unused session until it expires. Do not log tokens or signatures.

`DELETE /auth/session` has no body and revokes the caller's current token. Other
sessions remain valid. A subsequent protected operation using a revoked token
returns `401`.


## Reproducible authentication vector

The complete machine-readable vector is [auth-vector.json](../internal/session/testdata/auth-vector.json). `TestPublishedAuthenticationVector` reproduces it against the service. This public test identity uses seed `01` repeated 32 times; never register it as a real account. Its fixed expired timestamp is for interoperability tests, not a reusable live proof.

Registered public key: `iojj3XQJ8ZX9UtstPLpdcspnCb8dlBIb83SIAbQPb1w=`. The exact decoded signing message is below; there is one final LF after the nonce. The vector JSON also supplies the complete base64 form.

```text
VOID-NOTHINGNESS:auth:v1
AAECAwQFBgcICQoLDA0ODxAREhMUFRYXGBkaGxwdHh8
2026-11-02T00:02:00.123456789Z
110559210000000000
iojj3XQJ8ZX9UtstPLpdcspnCb8dlBIb83SIAbQPb1w=
aHR0cHM6Ly9hcGkudm9pZGNoYXRzLnh5ei9kbTlwWkc1dmRHaHBibWR1WlhOeg==
ICEiIyQlJicoKSorLC0uLzAxMjM0NTY3ODk6Ozw9Pj8
```

The corresponding `POST /auth/sessions` body is:
```json
{
  "challenge_id": "AAECAwQFBgcICQoLDA0ODxAREhMUFRYXGBkaGxwdHh8",
  "signature": "2yRq3ihXIiB6bd8vUYH3wYVbXLABcPVdxUE7zAHgVO8Swgfo/sVIRA5U0fxItrrPFp24/DJchFvP3PE93u65CQ=="
}
```


## Concrete resource examples

Examples below use two synthetic registered identities. Tiny opaque values such as `AAECAw==`, `BAUGBw==` and `CQgHBg==` demonstrate exact JSON shapes; they are not valid MLS objects. Real clients generate and verify MLS bytes locally. IDs and timestamps illustrate responses generated by the server; stable client UUIDs are generated and persisted by the client.

### Find a person

`GET /people/110559210000000001` returns `200 OK`:
```json
{
  "void_id": "110559210000000001",
  "username": "bob",
  "auth_public_key": "gTl3Dqh9F19Wo1Rmw0x+zMuNipG07jeiXfYPW4/Js5Q="
}
```


Looking up yourself is permitted. Unknown or blocked peers return `404 not_found`; the response does not tell the caller which condition applies. Only VOID-ID lookup is supported, not username search.

### Publish and claim public KeyPackages

Bob calls `POST /key-packages`:
```json
{
  "packages": [
    "AAECAw=="
  ]
}
```


`201 Created`:
```json
{
  "packages": [
    {
      "key_package_id": "20000000-0000-4000-8000-000000000001",
      "state": "available",
      "expires_at": "2026-11-09T06:00:00Z"
    }
  ]
}
```


Supply 1–16 nonempty packages with at most 1 MiB total decoded bytes. Exact package bytes are fingerprinted per publishing account. Repeated publication returns the same ID and its current state (`available`, `claimed` or `expired`), preserving its original expiry. It cannot replenish a claimed or expired package. The seven-day server expiry is a storage policy, not validation of the MLS object's own lifetime.

Alice calls `POST /people/110559210000000001/key-packages/claim`:
```json
{
  "client_claim_id": "10000000-0000-4000-8000-000000000001"
}
```


`200 OK`:
```json
{
  "client_claim_id": "10000000-0000-4000-8000-000000000001",
  "key_package_id": "20000000-0000-4000-8000-000000000001",
  "key_package_base64": "AAECAw==",
  "recipient": {
    "void_id": "110559210000000001",
    "username": "bob",
    "auth_public_key": "gTl3Dqh9F19Wo1Rmw0x+zMuNipG07jeiXfYPW4/Js5Q="
  }
}
```


Claims are atomic. A package goes to only one claimant/stable claim ID. Repeating that caller's same ID and target returns the same package; using that ID for another target returns `409 idempotency_conflict`. A claimed package is not recycled after disconnect, decline, block or expiry. If none is available, return `409 key_package_unavailable`: the recipient must publish a fresh package. Self-claims are invalid. A block in either direction makes the target unavailable.

### Create and list invitations

Alice calls `POST /requests`:
```json
{
  "client_request_id": "30000000-0000-4000-8000-000000000001",
  "to_void_id": "110559210000000001",
  "key_package_id": "20000000-0000-4000-8000-000000000001",
  "group_id_base64": "Vk9JRC1leGFtcGxl",
  "welcome_base64": "BAUGBw=="
}
```


`201 Created`:
```json
{
  "request_id": "40000000-0000-4000-8000-000000000001",
  "conversation_id": "50000000-0000-4000-8000-000000000001",
  "client_request_id": "30000000-0000-4000-8000-000000000001",
  "key_package_id": "20000000-0000-4000-8000-000000000001",
  "status": "pending",
  "sender": {
    "void_id": "110559210000000000",
    "username": "alice",
    "auth_public_key": "iojj3XQJ8ZX9UtstPLpdcspnCb8dlBIb83SIAbQPb1w="
  },
  "recipient": {
    "void_id": "110559210000000001",
    "username": "bob",
    "auth_public_key": "gTl3Dqh9F19Wo1Rmw0x+zMuNipG07jeiXfYPW4/Js5Q="
  },
  "group_id_base64": "Vk9JRC1leGFtcGxl",
  "welcome_base64": "BAUGBw==",
  "created_at": "2026-11-02T06:00:00Z",
  "updated_at": "2026-11-02T06:00:00Z"
}
```


The server binds the exact Welcome, group ID, claimed package, sender and recipient. The sender is the authenticated account; there is no plaintext introduction. Group IDs decode to 1–256 bytes; group plus Welcome together decode to at most 1 MiB. An identical stable request retry returns the original object, including its current status. A changed target, package, group or Welcome under that ID returns `409 idempotency_conflict`. A package already bound to another request cannot be reused.

There is at most one pending request or accepted direct conversation per unordered account pair. Simultaneous opposite-direction invitations serialize: one succeeds and the other returns `409 conversation_exists`. Preserve the local tentative MLS state until this outcome is known; use the canonical winning invitation rather than joining both groups.

`GET /requests?limit=50` returns both incoming and outgoing requests in stable request-ID order, including pending and terminal states. `200 OK`:
```json
{
  "requests": [
    {
      "request_id": "40000000-0000-4000-8000-000000000001",
      "conversation_id": "50000000-0000-4000-8000-000000000001",
      "client_request_id": "30000000-0000-4000-8000-000000000001",
      "key_package_id": "20000000-0000-4000-8000-000000000001",
      "status": "pending",
      "sender": {
        "void_id": "110559210000000000",
        "username": "alice",
        "auth_public_key": "iojj3XQJ8ZX9UtstPLpdcspnCb8dlBIb83SIAbQPb1w="
      },
      "recipient": {
        "void_id": "110559210000000001",
        "username": "bob",
        "auth_public_key": "gTl3Dqh9F19Wo1Rmw0x+zMuNipG07jeiXfYPW4/Js5Q="
      },
      "group_id_base64": "Vk9JRC1leGFtcGxl",
      "welcome_base64": "BAUGBw==",
      "created_at": "2026-11-02T06:00:00Z",
      "updated_at": "2026-11-02T06:00:00Z"
    }
  ],
  "next_cursor": "40000000-0000-4000-8000-000000000001",
  "has_more": false
}
```


For another page, call `GET /requests?after=40000000-0000-4000-8000-000000000001&limit=50`. `after` is the last request UUID, not a sync cursor. The default limit is 50, allowed range 1–100; unknown, repeated or empty query parameters are invalid. An empty initial page uses `next_cursor:""`. New concurrent inserts may sort before a page cursor; refresh lists from the beginning and use sync for new events rather than treating list pagination as a permanent change feed.

### Accept or decline an invitation

Only Bob, the recipient, calls `POST /requests/40000000-0000-4000-8000-000000000001/accept` with exactly `{}`. `200 OK`:
```json
{
  "request_id": "40000000-0000-4000-8000-000000000001",
  "conversation_id": "50000000-0000-4000-8000-000000000001",
  "client_request_id": "30000000-0000-4000-8000-000000000001",
  "key_package_id": "20000000-0000-4000-8000-000000000001",
  "status": "accepted",
  "sender": {
    "void_id": "110559210000000000",
    "username": "alice",
    "auth_public_key": "iojj3XQJ8ZX9UtstPLpdcspnCb8dlBIb83SIAbQPb1w="
  },
  "recipient": {
    "void_id": "110559210000000001",
    "username": "bob",
    "auth_public_key": "gTl3Dqh9F19Wo1Rmw0x+zMuNipG07jeiXfYPW4/Js5Q="
  },
  "group_id_base64": "Vk9JRC1leGFtcGxl",
  "welcome_base64": "BAUGBw==",
  "created_at": "2026-11-02T06:00:00Z",
  "updated_at": "2026-11-02T06:01:00Z"
}
```


The acceptance and both participants' `request.accepted` events commit together with the accepted conversation. Repeating the same action returns the current accepted request without another event. The sender receives `403 forbidden` if attempting to accept or decline their own invitation. An unrelated account receives `404 not_found` to avoid revealing another account's invitation. A request in a conflicting terminal state returns `409 request_not_pending`. Acceptance is not a delivery ACK for the Welcome: durably process its event separately.

Alternatively, while the invitation is pending, Bob calls `POST /requests/40000000-0000-4000-8000-000000000001/decline` with exactly `{}`. This is a separate example branch, not a valid action after acceptance. `200 OK`:
```json
{
  "request_id": "40000000-0000-4000-8000-000000000001",
  "conversation_id": "50000000-0000-4000-8000-000000000001",
  "client_request_id": "30000000-0000-4000-8000-000000000001",
  "key_package_id": "20000000-0000-4000-8000-000000000001",
  "status": "declined",
  "sender": {
    "void_id": "110559210000000000",
    "username": "alice",
    "auth_public_key": "iojj3XQJ8ZX9UtstPLpdcspnCb8dlBIb83SIAbQPb1w="
  },
  "recipient": {
    "void_id": "110559210000000001",
    "username": "bob",
    "auth_public_key": "gTl3Dqh9F19Wo1Rmw0x+zMuNipG07jeiXfYPW4/Js5Q="
  },
  "group_id_base64": "Vk9JRC1leGFtcGxl",
  "welcome_base64": "BAUGBw==",
  "created_at": "2026-11-02T06:00:00Z",
  "updated_at": "2026-11-02T06:01:00Z"
}
```


Decline notifies both participants, creates no accepted conversation and never makes the KeyPackage reusable. Repeating decline is idempotent. A fresh future invitation requires a fresh stable request ID and a fresh claimed package.

### Block a peer

`PUT /blocks/110559210000000000` with exactly `{}` returns `204 No Content`. The directional block is durable; a block in either direction prevents discovery, package claims, new requests and new message delivery for the pair. Pending requests become `cancelled`, and unacknowledged peer events are cancelled instead of delivered. A private `block.updated` event goes to the blocker when its event budget allows. If that notification alone would exceed capacity, the block and cancellations still commit and the new private notification is omitted; existing unrelated events remain intact. Keep the successful block action in local UI state. There is no unblock endpoint in this version.

The API does not send a block announcement to the blocked peer. Existing local copies and already-processed data cannot be retracted. Previously accepted conversations can still appear in conversation lists; their presence does not permit sending while blocked. An identical already-committed message/request retry can still return its original result without enqueueing a new delivery.

### List accepted conversations

`GET /conversations?limit=50` returns `200 OK`:
```json
{
  "conversations": [
    {
      "conversation_id": "50000000-0000-4000-8000-000000000001",
      "request_id": "40000000-0000-4000-8000-000000000001",
      "participants": [
        {
          "void_id": "110559210000000000",
          "username": "alice",
          "auth_public_key": "iojj3XQJ8ZX9UtstPLpdcspnCb8dlBIb83SIAbQPb1w="
        },
        {
          "void_id": "110559210000000001",
          "username": "bob",
          "auth_public_key": "gTl3Dqh9F19Wo1Rmw0x+zMuNipG07jeiXfYPW4/Js5Q="
        }
      ],
      "group_id_base64": "Vk9JRC1leGFtcGxl",
      "created_at": "2026-11-02T06:01:00Z"
    }
  ],
  "next_cursor": "50000000-0000-4000-8000-000000000001",
  "has_more": false
}
```


Paginate using `GET /conversations?after=50000000-0000-4000-8000-000000000001&limit=50`. The cursor is a conversation UUID and ordering is by that ID; the same limit/query rules as requests apply. This list contains accepted conversations only. `request.accepted` carries the same conversation object, so no separate `conversation.created` event exists.

### Send an encrypted wire object

Alice calls `POST /conversations/50000000-0000-4000-8000-000000000001/messages`:
```json
{
  "client_message_id": "60000000-0000-4000-8000-000000000001",
  "kind": "application",
  "ciphertext_base64": "CQgHBg=="
}
```


`201 Created`:
```json
{
  "message_id": "70000000-0000-4000-8000-000000000001",
  "client_message_id": "60000000-0000-4000-8000-000000000001",
  "conversation_id": "50000000-0000-4000-8000-000000000001",
  "sender_void_id": "110559210000000000",
  "recipient_void_id": "110559210000000001",
  "kind": "application",
  "sequence": "1",
  "ciphertext_base64": "CQgHBg==",
  "created_at": "2026-11-02T06:02:00Z"
}
```


`kind` is `application` or `commit`; both take one nonempty decoded wire object of at most 1 MiB. Only an accepted, unblocked conversation participant can send. The message and its recipient's `message.created` event commit in one transaction. `201` means durable server acceptance, not recipient delivery. An identical retry by the same sender returns the original message with the same ID, sequence and timestamp. Changing destination, kind or ciphertext under that sender/client ID returns `409 idempotency_conflict`. Stable message IDs are scoped across all conversations of that sender.

Conversation sequences are decimal strings, start at `"1"`, increase in the server's transaction order, and share one sequence for both senders and both kinds. They order accepted transport objects. They are not MLS epoch numbers or proof that concurrent commits are valid. The native client must resolve conflicting MLS commits, process the Welcome and required commits before dependent application events, and reject invalid epoch/group/member transitions. The backend cannot inspect this dependency from opaque bytes.

## Sync, acknowledgments and every event payload

Bob calls `GET /sync?after=0&limit=50`. `after` is an account-scoped decimal cursor, `"0"` before the first event. Default limit 50; allowed range 1–100. Only unacknowledged, uncancelled events for this authenticated account are returned, in ascending cursor order. Cursors can have gaps. Event UUIDs and cursors have no meaning as authorization for another account.

Example `200 OK`, after earlier invitation events have already been durably acknowledged:
```json
{
  "events": [
    {
      "event_id": "80000000-0000-4000-8000-000000000003",
      "cursor": "3",
      "type": "message.created",
      "payload": {
        "message_id": "70000000-0000-4000-8000-000000000001",
        "client_message_id": "60000000-0000-4000-8000-000000000001",
        "conversation_id": "50000000-0000-4000-8000-000000000001",
        "sender_void_id": "110559210000000000",
        "recipient_void_id": "110559210000000001",
        "kind": "application",
        "sequence": "1",
        "ciphertext_base64": "CQgHBg==",
        "created_at": "2026-11-02T06:02:00Z"
      },
      "created_at": "2026-11-02T06:02:00Z"
    }
  ],
  "next_cursor": "3",
  "has_more": false
}
```


The page is capped by both count and 4 MiB encoded JSON size. Drain `has_more:true` by fetching from `next_cursor`. Empty pages return an empty `events` array and retain the requested cursor, except that cancelled entries may advance it. Fetching does not ACK: unacknowledged events remain available when replaying from an earlier cursor or `after=0`. A client that fetches with a later cursor deliberately skips earlier entries in that response, even if they remain unacknowledged.

Keep the previous durable cursor until the entire page has been durably processed and its ACK succeeds, or keep a durable inbox that can resume ACK/replay after a crash. Never persist a cursor that skips data kept only in memory. Poll approximately every three seconds in the foreground, sync immediately on resume, and paginate promptly. Reauthenticate after session expiry. Lost network responses and reconnects require exact stable-ID retries and inbox deduplication.

`POST /sync/ack`:
```json
{
  "events": [
    {
      "event_id": "80000000-0000-4000-8000-000000000003",
      "outcome": "applied"
    }
  ]
}
```


Returns `204 No Content`. ACK 1–100 event UUIDs belonging to this account with outcome `applied` or `rejected`. The batch is atomic: an invalid or foreign event, conflicting outcome, or receipt capacity failure rolls back the batch. Repeating the same outcome succeeds without a second receipt; changing it returns `409 ack_conflict`. A cancelled event is not deliverable. A successful fetch or HTTP write is never an ACK.

ACKs record event processing only. Applying `request.created` does not accept
the invitation, and rejecting it does not decline it; use the explicit
accept/decline action for that state transition. A rejected Welcome must never
activate local MLS state. Preserve the original outcome in the durable inbox
when an already-processed event is replayed.

An applied application-message event creates `message.delivered` for its sender. Rejected application or commit events create `message.rejected`; rejection terminates redelivery and must not be displayed as delivered. Applied commit ACKs produce no delivery receipt. These outcomes report recipient-client processing, not that a human read the message. ACK receipt events too; receipt ACKs do not generate more receipts.

All events have exactly `event_id`, `cursor`, `type`, `payload`, `created_at`. Their complete payloads follow. The event examples are independent shape examples; timestamps and cursors illustrate their owner's event stream, and repeated resource IDs do not imply contradictory actions can occur on the same request.

### `request.created`

Both sender and recipient. The recipient receives the stored Welcome and public identities needed for client validation.
```json
{
  "event_id": "80000000-0000-4000-8000-000000000001",
  "cursor": "1",
  "type": "request.created",
  "payload": {
    "request_id": "40000000-0000-4000-8000-000000000001",
    "conversation_id": "50000000-0000-4000-8000-000000000001",
    "client_request_id": "30000000-0000-4000-8000-000000000001",
    "key_package_id": "20000000-0000-4000-8000-000000000001",
    "status": "pending",
    "sender": {
      "void_id": "110559210000000000",
      "username": "alice",
      "auth_public_key": "iojj3XQJ8ZX9UtstPLpdcspnCb8dlBIb83SIAbQPb1w="
    },
    "recipient": {
      "void_id": "110559210000000001",
      "username": "bob",
      "auth_public_key": "gTl3Dqh9F19Wo1Rmw0x+zMuNipG07jeiXfYPW4/Js5Q="
    },
    "group_id_base64": "Vk9JRC1leGFtcGxl",
    "welcome_base64": "BAUGBw==",
    "created_at": "2026-11-02T06:00:00Z",
    "updated_at": "2026-11-02T06:00:00Z"
  },
  "created_at": "2026-11-02T06:00:00Z"
}
```


### `request.accepted`

Both participants. This is also the conversation-activation event.
```json
{
  "event_id": "80000000-0000-4000-8000-000000000002",
  "cursor": "2",
  "type": "request.accepted",
  "payload": {
    "request": {
      "request_id": "40000000-0000-4000-8000-000000000001",
      "conversation_id": "50000000-0000-4000-8000-000000000001",
      "client_request_id": "30000000-0000-4000-8000-000000000001",
      "key_package_id": "20000000-0000-4000-8000-000000000001",
      "status": "accepted",
      "sender": {
        "void_id": "110559210000000000",
        "username": "alice",
        "auth_public_key": "iojj3XQJ8ZX9UtstPLpdcspnCb8dlBIb83SIAbQPb1w="
      },
      "recipient": {
        "void_id": "110559210000000001",
        "username": "bob",
        "auth_public_key": "gTl3Dqh9F19Wo1Rmw0x+zMuNipG07jeiXfYPW4/Js5Q="
      },
      "group_id_base64": "Vk9JRC1leGFtcGxl",
      "welcome_base64": "BAUGBw==",
      "created_at": "2026-11-02T06:00:00Z",
      "updated_at": "2026-11-02T06:01:00Z"
    },
    "conversation": {
      "conversation_id": "50000000-0000-4000-8000-000000000001",
      "request_id": "40000000-0000-4000-8000-000000000001",
      "participants": [
        {
          "void_id": "110559210000000000",
          "username": "alice",
          "auth_public_key": "iojj3XQJ8ZX9UtstPLpdcspnCb8dlBIb83SIAbQPb1w="
        },
        {
          "void_id": "110559210000000001",
          "username": "bob",
          "auth_public_key": "gTl3Dqh9F19Wo1Rmw0x+zMuNipG07jeiXfYPW4/Js5Q="
        }
      ],
      "group_id_base64": "Vk9JRC1leGFtcGxl",
      "created_at": "2026-11-02T06:01:00Z"
    }
  },
  "created_at": "2026-11-02T06:01:00Z"
}
```


### `request.declined`

Both participants. No accepted conversation is created.
```json
{
  "event_id": "80000000-0000-4000-8000-000000000003",
  "cursor": "3",
  "type": "request.declined",
  "payload": {
    "request_id": "40000000-0000-4000-8000-000000000001",
    "conversation_id": "50000000-0000-4000-8000-000000000001",
    "client_request_id": "30000000-0000-4000-8000-000000000001",
    "key_package_id": "20000000-0000-4000-8000-000000000001",
    "status": "declined",
    "sender": {
      "void_id": "110559210000000000",
      "username": "alice",
      "auth_public_key": "iojj3XQJ8ZX9UtstPLpdcspnCb8dlBIb83SIAbQPb1w="
    },
    "recipient": {
      "void_id": "110559210000000001",
      "username": "bob",
      "auth_public_key": "gTl3Dqh9F19Wo1Rmw0x+zMuNipG07jeiXfYPW4/Js5Q="
    },
    "group_id_base64": "Vk9JRC1leGFtcGxl",
    "welcome_base64": "BAUGBw==",
    "created_at": "2026-11-02T06:00:00Z",
    "updated_at": "2026-11-02T06:01:00Z"
  },
  "created_at": "2026-11-02T06:00:00Z"
}
```


### `message.created`

Recipient only. The sender uses its send response and durable outbox for its local sent state.
```json
{
  "event_id": "80000000-0000-4000-8000-000000000004",
  "cursor": "4",
  "type": "message.created",
  "payload": {
    "message_id": "70000000-0000-4000-8000-000000000001",
    "client_message_id": "60000000-0000-4000-8000-000000000001",
    "conversation_id": "50000000-0000-4000-8000-000000000001",
    "sender_void_id": "110559210000000000",
    "recipient_void_id": "110559210000000001",
    "kind": "application",
    "sequence": "1",
    "ciphertext_base64": "CQgHBg==",
    "created_at": "2026-11-02T06:02:00Z"
  },
  "created_at": "2026-11-02T06:02:00Z"
}
```


### `message.delivered`

Original sender only, after an applied application-message ACK.
```json
{
  "event_id": "80000000-0000-4000-8000-000000000005",
  "cursor": "5",
  "type": "message.delivered",
  "payload": {
    "message_id": "70000000-0000-4000-8000-000000000001",
    "conversation_id": "50000000-0000-4000-8000-000000000001",
    "sequence": "1",
    "recipient_void_id": "110559210000000001",
    "outcome": "applied",
    "created_at": "2026-11-02T06:02:03Z"
  },
  "created_at": "2026-11-02T06:02:03Z"
}
```


### `message.rejected`

Original sender only, after a rejected application or commit ACK.
```json
{
  "event_id": "80000000-0000-4000-8000-000000000006",
  "cursor": "6",
  "type": "message.rejected",
  "payload": {
    "message_id": "70000000-0000-4000-8000-000000000001",
    "conversation_id": "50000000-0000-4000-8000-000000000001",
    "sequence": "1",
    "recipient_void_id": "110559210000000001",
    "outcome": "rejected",
    "created_at": "2026-11-02T06:02:03Z"
  },
  "created_at": "2026-11-02T06:02:03Z"
}
```


### `block.updated`

Blocker only, when capacity permits the private notification. There is no peer-visible block/cancellation event.
```json
{
  "event_id": "80000000-0000-4000-8000-000000000007",
  "cursor": "7",
  "type": "block.updated",
  "payload": {
    "void_id": "110559210000000000",
    "blocked": true,
    "created_at": "2026-11-02T06:03:00Z"
  },
  "created_at": "2026-11-02T06:03:00Z"
}
```


## Errors, limits and retention

All errors use the existing JSON envelope; SQL, token and raw exception details are withheld. For example:
```json
{
  "error": {
    "code": "key_package_unavailable",
    "message": "Recipient must publish a fresh public KeyPackage."
  }
}
```


| HTTP | Code | Meaning / recovery |
| --- | --- | --- |
| 400 | `invalid_json`, `invalid_input` | Malformed/unknown/duplicate/null fields, invalid IDs, kinds, pagination or encoding. Correct the request. |
| 401 | `invalid_session` | Missing, invalid, revoked or expired bearer token; invalid/consumed authentication proof. Obtain a fresh handshake. `WWW-Authenticate: Bearer` is returned. |
| 403 | `forbidden` | An invitation sender tries a recipient-only accept/decline action. |
| 404 | `not_found` | Absent or unavailable/blocked resource, nonmember conversation or another account's invitation. |
| 405 | `method_not_allowed` | Unsupported method; see `Allow`. |
| 409 | `idempotency_conflict` | Stable client ID reused with different content/target. Recover the original durable client operation. |
| 409 | `key_package_unavailable` | Recipient must publish a fresh public package. |
| 409 | `key_package_claim_required` | Claim the package before creating this request. |
| 409 | `key_package_used` | Package is already bound to another request. Claim a fresh package. |
| 409 | `conversation_exists` | Pending request or accepted conversation already exists for this pair. Sync/list the canonical resource. |
| 409 | `request_not_pending` | Conflicting request transition. |
| 409 | `ack_conflict`, `event_cancelled` | Conflicting ACK outcome or delivery cancelled by a block. |
| 413 | `body_too_large`, `payload_too_large` | Encoded body or decoded opaque bytes exceed the limit. |
| 415 | `unsupported_media_type` | Send `application/json`. |
| 429 | `rate_limited`, `authentication_capacity` | Wait for `Retry-After`; active sessions or pending challenges can also exhaust authentication capacity. |
| 503 | `busy`, `messaging_capacity`, `messaging_unavailable`, `unavailable` | Temporary admission/storage/database failure. No success should be inferred; retry the same stable operation after `Retry-After`. Storage caps need operator action or pending ACK progress. |
| 408 / 504 | `request_canceled` / `deadline_exceeded` | Outcome may be ambiguous. Retry the exact stable operation. |

An invitation sender's forbidden recipient-only action returns `403`; access to another account's private resources generally uses `404` to avoid disclosure. Authentication challenge/session creation is not idempotent: after a lost proof response, obtain a fresh challenge rather than replaying the consumed one.

| Bound | Current default / fixed limit |
| --- | --- |
| Authentication JSON and claim/accept/decline/block bodies | 4,096 bytes |
| Package publication, invitation and message JSON body | 2 MiB |
| ACK body and batch | 32 KiB; 1–100 events |
| Decoded KeyPackage batch | 1–16 objects; 1 MiB total |
| Decoded Welcome plus group ID | 1 MiB total; group ID 1–256 bytes |
| Decoded application/commit object | 1 MiB |
| Sync/request page | Up to 100 items and 4 MiB encoded JSON |
| Authenticated account requests | 120/minute, burst 40; 4,096 tracked accounts per process |
| Unauthenticated handshake requests | 30/minute, burst 10 per `RemoteAddr` IP; 4,096 tracked IPs |
| Concurrent messaging handlers / request deadline | 64 / 10 seconds |
| Pending auth challenges / active sessions per account | 4,096 per process / 8 per account |
| Unused package server expiry | Seven days |
| Retained packages and requests | 4,096 packages and 16 MiB encoded package bytes per owner; 4,096 requests per involved account |
| Pending / retained events per account | 1,000 pending / 10,000 retained; 16 MiB pending / 64 MiB retained encoded event bytes |
| Retained messages per conversation | 10,000 messages and 128 MiB encoded ciphertext bytes |

Registration retains its separate limits and its 4 KiB body budget. Messaging rate counters and admission limits are process-local; restarting resets them and multiple replicas multiply them. Forwarding headers are ignored, so unauthenticated requests behind the current tunnel share its source-IP bucket. These controls are not a distributed bot-registration defense or an aggregate disk quota.

Each accepted outbound message also reserves one future receipt slot and 1,024
bytes in its sender's pending/retained event budget until the recipient processes
or cancels it. This prevents a full sender queue from making a recipient's ACK
depend on freeing unrelated queue space. The listed byte quotas count encoded
application data, not PostgreSQL indexes, tuple overhead, WAL, backups or total
disk consumption. Operators still need aggregate storage monitoring and an
account-admission/storage policy for bot-created accounts.

There is no automatic pruning/garbage collection in this version. Unacknowledged events survive offline time and server restart as long as the database is retained and healthy. ACKed/cancelled events, package fingerprints/claims and stable-ID records remain as tombstones so old retries cannot replay operations. Expired packages become unavailable but are retained. Retention and storage caps eventually stop new work with backpressure rather than silently dropping pending delivery; an operator must review a coordinated retention policy before raising limits or deleting records. Do not promise unlimited offline retention, permanent availability or exactly-once native processing.

## Integration status

The backend contract is implemented separately from the mobile app. The app still needs native signing, public-key and Welcome verification, durable outbox/inbox storage, foreground sync, and UI integration. Its existing 32-group and 64-lifetime-KeyPackage limits require a client replenishment/lifecycle design. Phone and LDPlayer must register separate identities for this first version.

# Phase 2: authentication and opaque relay

The existing Go process now also serves `GET /dm9pZG5vdGhpbmduZXNz/ws`
as a WebSocket upgrade. Local development uses
`ws://127.0.0.1:8080/dm9pZG5vdGhpbmduZXNz/ws`.
Registration keeps its existing paths, payloads, and responses.

The expected public URL is
`wss://api.voidchats.xyz/dm9pZG5vdGhpbmduZXNz/ws`.
**It is not exposed by the current tunnel rule**, which matches only
`accounts|challenges`. No DNS, hostname, tunnel, or networking configuration was
changed. Exposing this endpoint requires owner permission to extend the existing
path expression to `^/dm9pZG5vdGhpbmduZXNz/(accounts|challenges|ws)$`, retaining
the existing `http://127.0.0.1:8080` service. This is a proposed configuration
change, not an instruction the application executes. The currently running
Phase 1 process is not replaced automatically by building Phase 2.

## Authentication handshake

All application frames are UTF-8 WebSocket **text** messages containing one JSON
object. Every value is a string. Only the fields shown for that frame type are
accepted; duplicate fields, nulls, extra fields, trailing JSON, and binary
messages are rejected. VOID-IDs are canonical positive signed 64-bit decimal
strings (no leading zero, sign, or whitespace).

1. Connect and send the existing account's public VOID-ID:

   ```json
   {"type":"auth_begin","void_id":"110559210000000000"}
   ```

2. The server loads `auth_public_key` from the existing PostgreSQL account using
   a parameterized, read-only query. It accepts no client-supplied replacement
   public key or private key. Unknown accounts cannot authenticate. The response is:

   ```json
   {"type":"auth_challenge","challenge_id":"<opaque base64url token>","message":"<base64 bytes to sign>","expires_at":"2026-11-01T09:00:10Z"}
   ```

   `message` is padded standard base64 of these exact UTF-8 bytes, including
   the final newline:

   ```text
   VOID-NOTHINGNESS:authenticate:v1
   <challenge_id>
   <void_id>
   <expires_at formatted as RFC3339Nano UTC>
   <nonce>
   ```

   The token and nonce each contain 256 bits of independent OS randomness,
   encoded as unpadded base64url. Before signing, clients must verify the domain,
   challenge ID, intended VOID-ID, expiry, and framing, and use a trusted server
   connection. Sign the decoded `message` bytes verbatim with that account's
   Ed25519 private key, which stays on the client.

3. Send the proof on the **same socket**:

   ```json
   {"type":"auth_proof","challenge_id":"<same token>","signature":"<padded standard base64 of 64-byte signature>"}
   ```

4. Successful proof binds the connection to the stored identity:

   ```json
   {"type":"auth_ok","void_id":"110559210000000000"}
   ```

The entire handshake has a default 10-second deadline, including lookup and
proof verification; the challenge also expires after 10 seconds. Each connection
gets one authentication attempt. Proof verification consumes the challenge before
checking the proof, including wrong or malformed proofs. An unsuccessful socket
closes; reconnect and obtain a new challenge. A challenge issued to one socket
cannot authenticate a different socket. Registration and authentication use
different signing domains. Authentication issues no reusable cookie or token.

No relay operations are accepted before authentication. A known VOID-ID alone
grants no access. Errors use a fixed envelope such as
`{"type":"error","code":"authentication_failed"}` or codes `unauthorized`,
`unavailable`, and `invalid_frame`, followed by disconnection. Timeouts and broken
connections can close without an error frame. No raw database or network
diagnostics are returned.

## Relay frames

An authenticated sender transmits:

```json
{"type":"send","request_id":"client-001","recipient_id":"110559210000000001","payload":"AAECAw=="}
```

| Field | Meaning |
| --- | --- |
| `request_id` | 1–64 ASCII characters from `A-Z a-z 0-9 . _ : -`; used for correlation |
| `recipient_id` | Canonical VOID-ID of the intended online account |
| `payload` | Padded standard base64 of 1–16384 opaque bytes by default |

Base64 is only a JSON transport encoding. The Go server checks encoding and size;
it neither encrypts, decrypts, nor interprets the payload. A sender field supplied
by a client is rejected. The recipient receives server-generated metadata:

```json
{"type":"message","request_id":"client-001","sender_id":"110559210000000000","payload":"AAECAw=="}
```

The `sender_id` always comes from the authenticated connection. Request IDs and
opaque payloads are preserved. The sender receives:

```json
{"type":"result","request_id":"client-001","status":"forwarded"}
```

| Result | Meaning |
| --- | --- |
| `forwarded` | The recipient socket's WebSocket write completed successfully |
| `unavailable` | No live authenticated recipient connection was found; nothing was queued for offline delivery |
| `overloaded` | The recipient's bounded outgoing buffer was full; this request was not enqueued |
| `failed` | The transport write, session, or forwarding deadline failed; receipt is uncertain |

`forwarded` does **not** mean the recipient application read, processed, or
acknowledged the message. A successful write can still be followed by lost data
or a disconnect. A failed or missing result can also accompany an already-written
message. There is no delivery guarantee, recipient acknowledgment protocol,
deduplication, automatic retry, message history, or offline persistence.
Request IDs are correlation fields, not server-enforced idempotency keys. Clients
must account for ambiguous outcomes and duplicate application messages themselves.

## Connections and resource bounds

One authenticated socket per VOID-ID is active. The most recently successfully
authenticated socket replaces and closes its predecessor; a failed proof never
replaces a working session. Cleanup removes a mapping only if it still points to
that exact session. Replaced sockets cannot admit further sends; writes already
in flight at replacement or disconnect may complete.

The registry is protected by a mutex. Each socket has a bounded outgoing queue and
its own writer; the registry lock is never held during network I/O. A full queue
fails immediately, and all transport writes and forwarding waits have deadlines.
No goroutine is created per relayed message. Limits include unauthenticated
sockets, preventing unlimited pending authentications. Server shutdown explicitly
closes upgraded sockets, which `http.Server.Shutdown` does not close by itself.

| Setting | Default / limit |
| --- | --- |
| `VOID_WS_AUTH_TIMEOUT` | 10 seconds; configurable from 1 to 10 seconds |
| `VOID_WS_MAX_CONNECTIONS` | 256 total sockets; configurable from 1 to 4096 |
| `VOID_WS_QUEUE_SIZE` | 16 outgoing messages per socket; configurable from 1 to 256 |
| `VOID_WS_MAX_PAYLOAD_BYTES` | 16384 decoded bytes; configurable from 1 to 16384 |
| Incoming text message | Base64 payload limit plus 2048 bytes for the envelope in the executable; never over 24 KiB |
| Transport write deadline | 5 seconds |
| Forwarding deadline | 6 seconds, including queue wait |
| Heartbeat | WebSocket ping every 30 seconds; pong required within 10 seconds |

Clients must keep reading the socket so their WebSocket implementation can process
ping/pong control frames, including when the application has nothing to receive.
Unexpected disconnects, malformed data, timeouts, and unresponsive clients remove
the session. Excess connections receive HTTP 503 with `Retry-After: 1` before
upgrade. Compression is disabled. Native clients without an `Origin` header work;
the library retains its default same-origin check for browser upgrades. No CORS
or origin bypass was added.

The runtime logs connection/authentication/disconnection events and relay result
status with public routing IDs only. It never logs payloads, private keys,
challenge material, signatures, connection strings, or raw database diagnostics.

## Run the local demonstration

```bash
go run ./cmd/void-demo
```

This uses the real registration, authentication, and relay code with an in-memory
account repository and two simulated clients on an ephemeral loopback HTTP server.
It does not read `.env`, connect to PostgreSQL, contact a public host, or change
networking configuration. It registers two fresh ephemeral accounts, authenticates
both sockets, verifies unchanged simulated ciphertext in both directions, prints
the forwarded envelopes/results, disconnects the recipient, checks `unavailable`,
and checks that both connections are cleaned up. Keys and authentication proofs
are never printed or persisted.

## Database access and limitations

Returning-user authentication requires `SELECT` on `auth_public_key` in addition
to Phase 1 registration access. Startup/readiness validate that access without
changing it. The configured owner-selected `postgres` connection already has
that access. A differently provisioned role can fail readiness; granting access
requires separate owner approval. The code performs no migration, grant, password
change, database creation, or automatic repair. The old opt-in integration runner
reflects the extra read permission for a future explicitly authorized disposable
test run; it was not executed for Phase 2 verification.

Connections and online presence belong to one process and are lost on restart.
Multiple replicas, cross-instance relay, distributed flood controls, per-account
message quotas, recovery, offline storage, group messaging, and recipient
acknowledgments are outside this phase. A transport write alone cannot prove
recipient delivery. Public WebSocket routing still awaits the separately approved
tunnel rule change described above. No production deployment is performed.

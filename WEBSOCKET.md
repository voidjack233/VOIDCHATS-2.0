# Authenticated temporary WebSocket relay

The approved implementation uses the existing WebSocket endpoint for all normal messaging. See [the full frame contract](docs/MESSAGING_API.md). It has not been activated against the existing database; migration 004 requires explicit review before application.

VOID-ID is a public account address. Authentication proves possession of the private key corresponding to `accounts.auth_public_key`. Every reconnect authenticates again. There is one active connection per VOID-ID; a successfully authenticated replacement closes the previous connection. Failed authentication cannot replace it.

Start with:

```json
{"type":"auth_begin","void_id":"101"}
```

The server replies with `auth_challenge`, `challenge_id`, `message` (padded standard base64), and `expires_at`. Production signs these exact decoded UTF-8 bytes, including the final newline:

```text
VOID-NOTHINGNESS:authenticate:v2\n
<challenge_id>\n
<void_id>\n
<padded-base64 registered authentication public key>\n
<expiry in UTC RFC3339Nano>\n
<padded-base64 canonical HTTPS API audience>\n
<raw-base64url nonce>\n
```

The displayed `\n` denotes one actual newline, not a literal backslash. The audience defaults to `https://api.voidchats.xyz/dm9pZG5vdGhpbmduZXNz`. Clients must verify the version/domain, expected VOID-ID, public key, audience and expiry before signing. Send:

```json
{"type":"auth_proof","challenge_id":"<challenge_id>","signature":"<padded-base64 Ed25519 signature>"}
```

Success returns:

```json
{"type":"auth_ok","void_id":"101"}
```

Authentication is separate from MLS establishment. `auth_ok` means only that the connection proved account ownership. Each challenge belongs to one connection, expires quickly and is consumed before proof verification. The test-only/legacy constructor still supports `authenticate:v1`; the production executable selects the audience-bound v2 constructor. No server-side bearer-session table is used.

Initial OpenMLS KeyPackage/Welcome objects can travel as opaque `relay_online` frames while both participants are authenticated and online. Nothing is inventoried or marked cryptographically initialized by Go. Established MLS ciphertext uses `send`, including while the recipient is offline. No fresh KeyPackage is required for delivery.

Queued sends receive `accepted` only after storage commits. Recipient `ack` deletes the record. Ephemeral online forwarding receives `online_result`; it does not promise durable acceptance, application processing or MLS completion.

The existing transport keeps bounded admission, a single writer per socket, heartbeat, deadline handling, origin checks, connection replacement and shutdown closure. Payloads decode to at most 1 MiB; outgoing encoded frames have a 2 MiB per-connection byte budget. A disconnected sender never cancels a committed queued event.

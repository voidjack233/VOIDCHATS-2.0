# VOID-NOTHINGNESS

A Go backend that authenticates accounts and temporarily queues opaque encrypted events for delivery over WebSockets. VOID-ID is a public contact address, not an authentication credential. Devices retain their own history and OpenMLS state.

This source implements the owner-approved two-table design: `accounts` and a temporary `messages` queue. Go assigns creation time and a 30-day expiration. Server acceptance follows PostgreSQL commit; an authenticated recipient ACK deletes the complete queue record and ciphertext. Unexpired pending events are offered again after reconnect.

**Source implementation only: the existing database and installed server have not been migrated or replaced.** The exact proposed SQL is [004_temporary_relay_queue.sql](migrations/004_temporary_relay_queue.sql). Review [deployment and preservation requirements](docs/MESSAGING_DEPLOYMENT.md) before application. Startup validates the schema and never applies migrations.

The earlier request/conversation/KeyPackage/session APIs are retired in this source. No server conversation history, delivered column, KeyPackage inventory, delivery-receipt event system, persistent retry markers or automatic resend/backoff system is implemented. See the [WebSocket contract and limits](docs/MESSAGING_API.md), [approved architecture](docs/ARCHITECTURE_CORRECTION.md) and [source verification record](docs/RELAY_VALIDATION.md).

## Development

Go 1.26.5 and PostgreSQL 18 are used by this project. Build a review executable without overwriting the installed server:

```bash
go build -o /tmp/void-relay-review ./cmd/void-server
go test ./...
go test -race ./...
go vet ./...
```

Ordinary tests use synthetic accounts, mocks and loopback `httptest` servers. PostgreSQL registration tests skip unless explicitly opted in with an approved disposable database. No automated test should use real accounts or the existing database. Do not start the new executable against the old schema.

The local synthetic demo neither reads `.env` nor opens PostgreSQL or public network connections:

```bash
go run ./cmd/void-demo
```

The existing literal `.env`, configured PostgreSQL login and installed executable remain owner-managed. [`.env.example`](.env.example) documents reference settings. Environment assignments are parsed as literal text; no shell interpolation is evaluated. Process values take precedence, and `-env-file ''` disables environment-file loading. Connection diagnostics and credentials are not logged.

## Account initialization

The existing HTTP registration routes remain:

- `POST /dm9pZG5vdGhpbmduZXNz/challenges`
- `POST /dm9pZG5vdGhpbmduZXNz/accounts`

A challenge accepts an authentication public key and an optional display username:

```json
{"auth_public_key":"<padded base64 of a valid 32-byte Ed25519 public key>"}
```

When supplied, a nonempty username must retain the existing UTF-8, length and character validation. Omitted/empty usernames store SQL NULL after the reviewed migration; no substitute name is invented. Existing usernames, account UUIDs, VOID-IDs and keys are preserved.

Clients sign the exact decoded `VOID-NOTHINGNESS:register:v1` challenge message and submit `challenge_id` and padded-base64 `signature` to `/accounts`. Registration proofs remain single-use and bind the submitted public key and optional username. Private keys never leave devices. Registration creates no persistent messaging session.

All ordinary messaging uses `/dm9pZG5vdGhpbmduZXNz/ws`. Each connection proves ownership with a fresh Ed25519 challenge before lookup, send, ACK or online protocol exchange. The production proof binds the canonical API audience. See [WEBSOCKET.md](WEBSOCKET.md) for the exact proof bytes.

Local `/healthz` and `/readyz` remain available. The listener stays on loopback behind the existing HTTPS tunnel. No Cloudflared, DNS, credentials or deployment configuration changes are part of this implementation.

## Scope and reliability

The first version offers basic committed acceptance, forwarding, recipient ACK deletion and reconnect replay. A lost acceptance or recipient ACK can leave an ambiguous result or cause duplicate delivery after reconnect. Queued duplicate event IDs are rejected; after deletion the same ID can be accepted again. There are no retained tombstones or exactly-once processing guarantees. Devices remain responsible for safely handling duplicates and applying OpenMLS state.

One event is offered at a time to each recipient connection. Its ACK or expiry opens the delivery window. A lost ACK can stall that window until reconnect or expiry. There is no automatic retransmission timer and no durable sender-facing delivery status.

Expired events are excluded from dispatch. A bounded cleanup task in the same Go process deletes expired queue rows on startup and periodically; outages delay physical cleanup. PostgreSQL MVCC, WAL and backups have separate physical retention behavior.

Per-account byte/count quotas, command rate limits, connection admission and transport byte budgets bound queue abuse. They do not solve unlimited fresh-key bot-account registration. Persistent per-peer blocking, server-managed contact requests and multi-device delivery tracking are outside this minimal design.

Historical database changes and earlier deployments remain recorded in [DATABASE_CHANGES.md](DATABASE_CHANGES.md) and [the historical validation record](docs/MESSAGING_VALIDATION.md). Those records describe previous builds, not the current unactivated source.

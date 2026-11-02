# VOID-NOTHINGNESS

A Go backend that authenticates accounts and temporarily queues opaque encrypted events for delivery over WebSockets. VOID-ID is a public contact address, not an authentication credential. Devices retain their own history and OpenMLS state.

This source implements the owner-approved two-table design: `accounts` and a temporary `messages` queue. Go assigns creation time and a 30-day expiration. Server acceptance follows PostgreSQL commit; an authenticated recipient ACK deletes the complete queue record and ciphertext. Unexpired pending events are offered again after reconnect.

**Activated on 2026-11-02 at 14:56 (Asia/Manila), after the owner explicitly authorized migration.** Database `void2-0e2ee` now has only `accounts` and `messages`; both existing accounts and their keys were preserved. Backend source `5fd593f` is running with username-free registration and the username column removed. The applied SQL is [004_temporary_relay_queue.sql](migrations/004_temporary_relay_queue.sql), followed by [005_drop_account_username.sql](migrations/005_drop_account_username.sql); see [deployment and preservation evidence](docs/MESSAGING_DEPLOYMENT.md). Startup validates the schema and never applies migrations.

The earlier request/conversation/KeyPackage/session APIs are retired in this source. No server conversation history, delivered column, KeyPackage inventory, delivery-receipt event system, persistent retry markers or automatic resend/backoff system is implemented. See the [WebSocket contract and limits](docs/MESSAGING_API.md), [approved architecture](docs/ARCHITECTURE_CORRECTION.md) and [verification and activation record](docs/RELAY_VALIDATION.md).

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

A challenge accepts only the authentication public key:

```json
{"auth_public_key":"<padded base64 of a valid 32-byte Ed25519 public key>"}
```

Username creation is removed. Sending a `username` field, including an empty string or null, returns HTTP 400. Migration 005 removes the legacy username column and its CHECK constraint. Accounts contain `id`, `void_id`, `auth_public_key` and `created_at`; existing account identities and keys are preserved.

Clients sign the exact decoded `VOID-NOTHINGNESS:register:v1` challenge message and submit `challenge_id` and padded-base64 `signature` to `/accounts`. Registration proofs remain single-use and bind the submitted public key. The v1 proof retains an empty reserved line after the encoded key, so its bytes match the previous username-omitted flow. Registration responses contain only `void_id` and `created_at`. Private keys never leave devices. Registration creates no persistent messaging session.

All ordinary messaging uses `/dm9pZG5vdGhpbmduZXNz/ws`. Each connection proves ownership with a fresh Ed25519 challenge before lookup, send, ACK or online protocol exchange. The production proof binds the canonical API audience. See [WEBSOCKET.md](WEBSOCKET.md) for the exact proof bytes.

Local `/healthz` and `/readyz` remain available. The listener stays on loopback behind the existing HTTPS tunnel. No Cloudflared, DNS, credentials or deployment configuration changes are part of this implementation.

## Scope and reliability

The first version offers basic committed acceptance, forwarding, recipient ACK deletion and reconnect replay. A lost acceptance or recipient ACK can leave an ambiguous result or cause duplicate delivery after reconnect. Queued duplicate event IDs are rejected; after deletion the same ID can be accepted again. There are no retained tombstones or exactly-once processing guarantees. Devices remain responsible for safely handling duplicates and applying OpenMLS state.

One event is offered at a time to each recipient connection. Its ACK or expiry opens the delivery window. A lost ACK can stall that window until reconnect or expiry. There is no automatic retransmission timer and no durable sender-facing delivery status.

Expired events are excluded from dispatch. A bounded cleanup task in the same Go process deletes expired queue rows on startup and periodically; outages delay physical cleanup. PostgreSQL MVCC, WAL and backups have separate physical retention behavior.

Per-account byte/count quotas, command rate limits, connection admission and transport byte budgets bound queue abuse. They do not solve unlimited fresh-key bot-account registration. Persistent per-peer blocking, server-managed contact requests and multi-device delivery tracking are outside this minimal design.

Historical database changes and earlier deployments remain recorded in [DATABASE_CHANGES.md](DATABASE_CHANGES.md) and [the historical validation record](docs/MESSAGING_VALIDATION.md). Those records describe previous builds, not the current relay deployment.

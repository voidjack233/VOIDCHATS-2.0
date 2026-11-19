# VOID-NOTHINGNESS

Permanent account deletion was activated on 2026-11-16 at 16:59 (Asia/Manila),
with [migration 007](migrations/007_account_deletion.sql) and backend source
`9e6b6d5`. All 13 existing accounts remained Active and their identities, keys,
queued messages, prekey inventories, and claims were preserved. See the
[activation and recovery record](docs/ACCOUNT_DELETION_DEPLOYMENT.md) and the
[complete Rust integration contract](docs/ACCOUNT_DELETION.md). Deleted VOID-IDs
remain reserved while queued ciphertext and public prekeys are removed
transactionally. Startup never applies migrations.

The [initial messaging protocol V1](docs/PROTOCOL_V1.md) includes text, replies, reactions, Delete for Everyone and encrypted receipts through ordinary opaque `send`/`event`. No backend runtime change is required. The prerelease V2 feature activation plan is retired; generic versioned delivery and mandatory minimum enforcement are retained for future incompatible releases. This consolidation does not deploy or reset anything.

A Go backend that authenticates accounts and temporarily queues opaque encrypted events for delivery over WebSockets. VOID-ID is a public contact address, not an authentication credential. Devices retain their own history and encryption state.

Version-aware encrypted delivery was activated on 2026-11-18 at 13:04 (Asia/Manila), with source `2faffd2` and [migration 008](migrations/008_message_protocol_version.sql). All 18 accounts and 13 queued events were preserved; existing ciphertext defaults to version 1. Each connection starts at version 1 and can advertise newer support after authentication. An incompatible queue head holds delivery until a capable connection resumes or the event expires. VOID Native Reply, Reactions and Delete for Everyone share this generic encrypted path. See the [client contract](docs/MESSAGING_API.md#application-protocol-support) and [activation, recovery and ordering limits](docs/VERSIONED_DELIVERY.md).

Deployed on 2026-11-18 at 19:49 (Asia/Manila), source `9eb1319`: `VOID_MIN_PROTOCOL_VERSION` adds a server-enforced application protocol minimum, currently `1`. A higher minimum blocks messaging until each authenticated connection advertises sufficient support. See the [coordinated release instructions](docs/VERSIONED_DELIVERY.md#mandatory-minimum-protocol). Version 2 enforcement has not been activated.

Identity and temporary ciphertext delivery use `accounts` and `messages`. The approved Signal extension adds `signal_prekey_inventory` and `signal_prekey_claims` for public setup material. Go assigns creation time and a 30-day expiration. Server acceptance follows PostgreSQL commit; an authenticated recipient ACK deletes the complete queue record and ciphertext. Unexpired pending events are offered again after reconnect.

**Signal public prekeys activated on 2026-11-03 11:36 (Asia/Manila), after explicit approval.** That activation deployed source `15458d7`; the current activation is recorded above. Database `void2-0e2ee` has the four reviewed tables; all three existing accounts and authentication keys were preserved. That activation used SQL: [004](migrations/004_temporary_relay_queue.sql), [005](migrations/005_drop_account_username.sql) and [006](migrations/006_signal_public_prekeys.sql). See [deployment evidence](docs/MESSAGING_DEPLOYMENT.md). Startup validates schema and never applies migrations.

The earlier request/conversation/KeyPackage/session APIs are retired in this source. No server conversation history, delivered column, MLS KeyPackage inventory, delivery-receipt event system, permanent message retry markers or automatic resend/backoff system is implemented. See the [WebSocket contract and limits](docs/MESSAGING_API.md), [approved architecture](docs/ARCHITECTURE_CORRECTION.md) and [verification and activation record](docs/RELAY_VALIDATION.md).

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

## Signal public-prekey extension

The frontend now uses official libsignal for 1:1 messaging. The matching Go
publication/claim commands and public-only persistence adapter are active after
[approved migration 006](migrations/006_signal_public_prekeys.sql). The frontend
owner can connect its prepared prekey adapter. The binary also supports the base
two-table layout with the extension disabled. See the
[exact contract, limits and verification](docs/SIGNAL_PREKEYS.md).

## Account initialization

The existing HTTP registration routes remain:

- `POST /dm9pZG5vdGhpbmduZXNz/challenges`
- `POST /dm9pZG5vdGhpbmduZXNz/accounts`

A challenge accepts only the authentication public key:

```json
{"auth_public_key":"<padded base64 of a valid 32-byte Ed25519 public key>"}
```

Username creation is removed. Sending a `username` field, including an empty string or null, returns HTTP 400. Migration 005 removes the legacy username column and its CHECK constraint. Accounts contain `id`, `void_id`, `auth_public_key`, `created_at`, and nullable `deleted_at`; existing account identities and keys are preserved.

Clients sign the exact decoded `VOID-NOTHINGNESS:register:v1` challenge message and submit `challenge_id` and padded-base64 `signature` to `/accounts`. Registration proofs remain single-use and bind the submitted public key. The v1 proof retains an empty reserved line after the encoded key, so its bytes match the previous username-omitted flow. Registration responses contain only `void_id` and `created_at`. Private keys never leave devices. Registration creates no persistent messaging session.

All ordinary messaging uses `/dm9pZG5vdGhpbmduZXNz/ws`. Each connection proves ownership with a fresh Ed25519 challenge before lookup, send, ACK or online protocol exchange. The production proof binds the canonical API audience. See [WEBSOCKET.md](WEBSOCKET.md) for the exact proof bytes.

Local `/healthz` and `/readyz` remain available. The listener stays on loopback behind the existing HTTPS tunnel. No Cloudflared, DNS, credentials or deployment configuration changes are part of this implementation.

## Scope and reliability

The first version offers basic committed acceptance, forwarding, recipient ACK deletion and reconnect replay. A lost acceptance or recipient ACK can leave an ambiguous result or cause duplicate delivery after reconnect. Queued duplicate event IDs are rejected; after deletion the same ID can be accepted again. There are no retained tombstones or exactly-once processing guarantees. Devices remain responsible for safely handling duplicates and applying their encryption state.

One event is offered at a time to each recipient connection. Its ACK or expiry opens the delivery window. A lost ACK can stall that window until reconnect or expiry. There is no automatic retransmission timer and no durable sender-facing delivery status.

Expired events are excluded from dispatch. A bounded cleanup task in the same Go process deletes expired queue rows on startup and periodically; outages delay physical cleanup. PostgreSQL MVCC, WAL and backups have separate physical retention behavior.

Per-account byte/count quotas, command rate limits, connection admission and transport byte budgets bound queue abuse. They do not solve unlimited fresh-key bot-account registration. Persistent per-peer blocking, server-managed contact requests and multi-device delivery tracking are outside this minimal design.

Historical database changes and earlier deployments remain recorded in [DATABASE_CHANGES.md](DATABASE_CHANGES.md) and [the historical validation record](docs/MESSAGING_VALIDATION.md). Those records describe previous builds, not the current relay deployment.

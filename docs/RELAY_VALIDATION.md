# Temporary relay source verification

Verified on 2026-11-02 (Asia/Manila), on branch backend. This is source verification only: no migration, live PostgreSQL test, installed-binary replacement, service restart, credential edit, tunnel/network change or frontend work was performed. The review binary is `/tmp/void-relay-review`; it has not been started against the existing database.

| Check | Result |
| --- | --- |
| Full `go test -count=1 ./...` with PostgreSQL/live opt-ins unset | Passed; synthetic/unit/mock/loopback tests only |
| Full `go test -race -count=1 ./...` with the same opt-ins unset | Passed |
| PostgreSQL adapter race tests after UTC normalization | Passed using pgx mocks |
| `go vet ./...` | Passed |
| `go build -o /tmp/void-relay-review ./cmd/void-server` | Passed |
| Formatting and `git diff --check` | Passed |
| Documented JSON examples | All 14 parse without duplicate fields |
| Python registration helper | Parses; not executed against PostgreSQL |
| Proposed SQL/catalog behavior on PostgreSQL | Not executed; separate review/authorization required |

Core tests verify a server-owned UTC clock, exact 720-hour expiry, byte-preserving offline storage in the synthetic store, recipient-qualified deletion, physical removal from the in-memory queue, expiry boundaries, bounded cleanup, sender/recipient byte quotas, duplicate pending IDs, account-scoped IDs, cancellation and safe storage errors. Recreating a Service over the same memory store models restart access to pending records; it does not constitute real disk/process-restart verification.

Loopback WebSocket tests verify Ed25519 ownership, audience-bound authentication, persistence before acceptance, live forwarding, remaining-event replay after reconnect, a disconnected sender's pending delivery, wrong-recipient ACK safety, ACK deletion, client timestamp/sender/legacy-frame rejection, opaque online initialization forwarding without storage, payload/byte budgets, account command rates across reconnect, admission, origin handling, heartbeat, slow writers and connection replacement. Ordinary HTTP deadlines do not shorten upgraded socket lifetime. The synthetic demo registers two local identities and exercises both directions plus offline/reconnect/ACK deletion without reading `.env` or opening a database.

Registration tests cover omitted/empty usernames, retained supplied names, invalid/null/duplicate/unknown fields, key validation, signed proof binding, expiration and replay. Account repository mocks verify SQL NULL binding, existing identity/key preservation and nullable-username schema validation.

PostgreSQL queue mocks cover numeric account lock ordering, exact 64-bit ID splitting into two advisory-lock keys, no account UPDATE privilege requirement, transaction commit before acceptance, quota/duplicate rejection, ownership-bound ACK deletion, unexpired bounded scans, bounded expiry deletion, safe errors, six-column schema validation and PostgreSQL 18 NOT NULL catalog handling. These tests do not prove actual SQL execution or migration rollback; an approved disposable PostgreSQL check remains necessary before live cutover. Production connection-local IntervalStyle is pinned to postgres for deterministic constraint rendering without changing database/server settings.

The migration is deliberately drain-first: it refuses ANY legacy messaging/session/block/package data and preserves accounts. Its exact reviewed files at this verification are:

| File | SHA-256 |
| --- | --- |
| `004_temporary_relay_queue.sql` | `2c11bc55d59c274baadbfee39768a5a377c35dd641eaead119a94b28a78f7a80` |
| `004_temporary_relay_privileges.sql` | `d38cb08ad539ccddf3e4d9f20fa099e4241783c7ca13e8b794091321c3a7756c` |

There is no automatic resend/backoff, persistent deduplication, permanent delivered state or sender-facing receipt lifecycle. Tests explicitly demonstrate rejection while an ID is queued and possible reuse after deletion, without claiming exactly-once processing. One in-flight event per recipient can stall on a lost ACK until reconnect or expiry. Synthetic opaque bytes do not demonstrate native OpenMLS encryption or handset integration.

The earlier paused source draft was preserved privately before reconciliation. Historical migrations 001–003 remain unchanged. Existing database records and the old installed backend remain untouched. See [the exact migration review](MESSAGING_DEPLOYMENT.md) and [current protocol](MESSAGING_API.md).

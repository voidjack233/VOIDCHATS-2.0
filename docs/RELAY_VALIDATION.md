# Temporary relay verification and activation

Verified on 2026-11-02 (Asia/Manila), on branch backend. Deployed source: `f583d3a5d06370b4627cdcf44814e77abe72e477`. Installed binary SHA-256: `19cc6c430397eeede97633df99003cf9ddd05d146ba8d1662dcc74a4fdb49615`.

The owner explicitly authorized migration with "do the migration now". The exact 004 was rehearsed against a full isolated restore, then applied to the existing database at 14:56. Both accounts and keys remain exact; account table identity, metadata and privileges were preserved apart from optional username nullability. Exactly accounts/messages remain, and the new queue was empty after verification. The original matching backend was started after that migration.

Private backup and audit: `/home/void0000/.local/state/void-backend/deployments/20261102T144530+0800-two-table-relay`. Full backup restore, account/table hashes, guard rollback snapshots, build metadata, migration results, all-PASS synthetic real-PostgreSQL/WebSocket logs and live activation probes are retained with private permissions. Temporary private connection state and test clusters are removed after verification. Existing credentials, .env content/permissions, user-local instruction/ignore changes and Cloudflare configuration were preserved. No autostart service was installed.

Actual PostgreSQL testing found cleanup row-locking required an UPDATE privilege not granted to the restricted role. The fix removed the unnecessary SELECT lock, retaining bounded DELETE with an outer expiry recheck. No new UPDATE permission or schema change was introduced. All eight actual private database/HTTP/WebSocket assertion groups then passed. Live checks performed no account registration or message writes.

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
| Exact SQL/catalog behavior on PostgreSQL 18 | Passed on isolated restored/synthetic databases, including restricted-role validation |
| Full real-PostgreSQL/HTTP/WebSocket synthetic flow | All eight checks passed |
| Live schema/account-preservation check | Passed; two accounts and original keys preserved |
| Live HTTP/tunnel and WebSocket route checks | 17 HTTP/readiness probes and two WebSocket checks passed |

Core tests verify a server-owned UTC clock, exact 720-hour expiry, byte-preserving offline storage in the synthetic store, recipient-qualified deletion, physical removal from the in-memory queue, expiry boundaries, bounded cleanup, sender/recipient byte quotas, duplicate pending IDs, account-scoped IDs, cancellation and safe storage errors. Recreating a Service over the same memory store models restart access to pending records; it does not constitute real disk/process-restart verification.

Loopback WebSocket tests verify Ed25519 ownership, audience-bound authentication, persistence before acceptance, live forwarding, remaining-event replay after reconnect, a disconnected sender's pending delivery, wrong-recipient ACK safety, ACK deletion, client timestamp/sender/legacy-frame rejection, opaque online initialization forwarding without storage, payload/byte budgets, account command rates across reconnect, admission, origin handling, heartbeat, slow writers and connection replacement. Ordinary HTTP deadlines do not shorten upgraded socket lifetime. The synthetic demo registers two local identities and exercises both directions plus offline/reconnect/ACK deletion without reading `.env` or opening a database.

Current registration tests cover key-only registration, rejection of named/empty/null usernames, strict invalid/duplicate/unknown-field handling, key validation, signed proof binding, expiration and replay. Account repository mocks verify that INSERT/RETURNING never reference username, preserve identity/key/time values, retain nullable-username schema validation and require no username privileges.

PostgreSQL queue mocks cover numeric account lock ordering, exact 64-bit ID splitting into two advisory-lock keys, no account UPDATE privilege requirement, transaction commit before acceptance, quota/duplicate rejection, ownership-bound ACK deletion, unexpired bounded scans, bounded expiry deletion, safe errors, six-column schema validation and PostgreSQL 18 NOT NULL catalog handling. The later private PostgreSQL rehearsal and flow verification additionally exercised actual SQL, permissions and atomic guard rollback. Production connection-local IntervalStyle is pinned to postgres for deterministic constraint rendering without changing database/server settings.

The migration is deliberately drain-first: it refuses ANY legacy messaging/session/block/package data and preserves accounts. Its exact reviewed files at this verification are:

| File | SHA-256 |
| --- | --- |
| `004_temporary_relay_queue.sql` | `2c11bc55d59c274baadbfee39768a5a377c35dd641eaead119a94b28a78f7a80` |
| `004_temporary_relay_privileges.sql` | `d38cb08ad539ccddf3e4d9f20fa099e4241783c7ca13e8b794091321c3a7756c` |

There is no automatic resend/backoff, persistent deduplication, permanent delivered state or sender-facing receipt lifecycle. Tests explicitly demonstrate rejection while an ID is queued and possible reuse after deletion, without claiming exactly-once processing. One in-flight event per recipient can stall on a lost ACK until reconnect or expiry. Synthetic opaque bytes do not demonstrate native OpenMLS encryption or handset integration.

The earlier paused source draft was preserved privately before reconciliation. Historical migrations 001–003 remain unchanged. Both existing account rows were preserved and the matching new backend replaced the earlier executable after migration. All retired legacy messaging data tables were empty under the migration locks. See [the exact migration review](MESSAGING_DEPLOYMENT.md) and [current protocol](MESSAGING_API.md).


## Username creation removal: 2026-11-02, 15:28 (Asia/Manila)

The owner requested removal of username creation. Current source removes it from
HTTP fields, service/repository interfaces, account responses and the synthetic
demo. New INSERTs omit the nullable legacy column; existing names and identities
remain untouched. The v1 signed registration bytes retain their empty reserved
line, preserving the previous key-only proof format. No SQL migration, role/grant,
credential, tunnel, frontend or deployment-configuration change was made.

The complete race suite and vet passed with PostgreSQL/live test opt-ins unset.
Python integration-runner syntax passed; its disposable-schema legacy column is
retained and username privileges are no longer requested. The PostgreSQL fixture
assertions were updated but were not executed against any database for this
follow-up. The actual PostgreSQL checks above describe the initial migration.

The prior executable was backed up privately before a graceful replacement.
Thirteen local/public probes passed: health/readiness/tunnel, rejection of named,
empty and null username fields, key-only challenge creation with exact v1 binding,
and empty-registration rejection. These probes created only process-local
challenges; no account or message records were written. The backend is running.
Private executable backup, build metadata and activation results: `/home/void0000/.local/state/void-backend/deployments/20261102T152735+0800-username-free-registration`.

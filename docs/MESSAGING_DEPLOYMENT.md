# Two-table relay: migration review and deployment

Status: owner-approved design implemented in source; exact SQL is prepared for review and has NOT been applied. The existing installed server still runs the previous backend. No live messaging or PostgreSQL test, deployment, credential edit or network change is included in source verification.

## Proposed SQL and effects

[004_temporary_relay_queue.sql](../migrations/004_temporary_relay_queue.sql) is the complete exact proposed database transaction, applied after the already-existing migrations 001–003. It is not executed by startup, tests or a launcher.

The transaction sets a 5-second lock timeout and 1-minute statement timeout, checks the expected legacy schema, and locks accounts plus all nine legacy messaging tables before examining their contents. It aborts if any legacy data table contains even one row. Expired sessions, ACKed messages, blocks, claims and delivery events are all protected by this guard. Only the expected singleton internal schema marker(version 1) can be discarded. RLS must not hide rows from the guard. An unexpected schema or dependency aborts instead of using CASCADE.

After these guards, its operations are:

1. `ALTER TABLE public.accounts ALTER COLUMN username DROP NOT NULL;`
2. Drop the specific circular foreign key `key_packages_request_fk`.
3. Drop the empty old `messages`, `conversations`, `message_requests`, `key_packages`, `blocks`, `sync_events`, `sync_cursors`, `sessions` and `messaging_schema` tables, in dependency order. No CASCADE is used.
4. Create the new six-column `messages` queue with ownership foreign keys to accounts, a composite sender/event primary key, a 1 MiB payload bound and a fixed 720-hour expiry constraint. Both timestamps are supplied only by Go.
5. Create recipient-delivery and expiry indexes.

Accounts are not recreated. Their existing internal UUIDs/defaults, VOID-IDs, keys, creation timestamps and username values are preserved. No username backfill or column removal is performed. Historical migrations 001–003 remain unchanged. The superseded, unapplied optional-request-package draft is not part of this migration sequence.

The companion [004_temporary_relay_privileges.sql](../migrations/004_temporary_relay_privileges.sql) is separately reviewed and parameterized for an existing restricted runtime role. The new queue needs SELECT, INSERT and DELETE; replacing the old table does not transfer its old grants. Existing account registration/key-lookup privileges remain necessary. The owner-selected postgres login already has the needed access; do not change roles or passwords merely to apply this design.

## Legacy records and approval boundary

If any guard fails, do not delete rows to force the migration through. Record counts and review a specific preservation/drain/import decision with the owner. No data disposal is authorized by the guard failure.

The old messages table contains retained message history, while pending delivery is recorded in sync_events. Importing every message would redeliver acknowledged history. Legacy client-message IDs, server-message IDs and delivery-event IDs are different identifiers; old server-generated receipt events are not peer-encrypted payloads. Existing contact requests, packages, blocks and sessions also require explicit retirement decisions. This implementation does not invent a permanent staging table or silently transform that data.

Before any approved application, make and verify a private recovery backup, record the current executable, and snapshot account identity/key digests and schema. Confirm the exact migration and privilege changes, pending-data handling, cutover and cleanup activation. Maintain an explicit retention policy for temporary migration backups; ciphertext retention in backups is separate from queue deletion.

Only after approval should the old process be stopped, SQL applied and the reviewed new executable installed. The old binary cannot serve the new queue schema and the new binary intentionally refuses the old schema. There is no automatic fallback migration. Ordinary startup performs read-only catalog/access validation and requires exactly accounts/messages before listening or starting cleanup.

If live SQL fails, its transaction rolls back. If a post-migration application problem occurs, preserve all new queued records and coordinate rollback; do not restore a full old backup over new events or blindly recreate old tables.

## Review build and verification

Build without replacing the installed binary:

```bash
go build -o /tmp/void-relay-review ./cmd/void-server
```

Normal verification uses no PostgreSQL or real environment file:

```bash
env -u VOID_RUN_POSTGRES_INTEGRATION -u VOID_RUN_MESSAGING_INTEGRATION \
    -u VOID_RUN_MESSAGING_BLACKBOX go test -count=1 ./...
env -u VOID_RUN_POSTGRES_INTEGRATION -u VOID_RUN_MESSAGING_INTEGRATION \
    -u VOID_RUN_MESSAGING_BLACKBOX go test -race -count=1 ./...
go vet ./...
```

Loopback HTTP/WebSocket fixtures create synthetic keys and in-memory accounts only. Adapter tests mock pgx transactions; they do not establish a database connection. Default PostgreSQL registration cases skip. Historical messaging database/restart runners and their old API tests were retired rather than run against the changed architecture. This source verification does not claim that migration DDL/catalog expression handling has been exercised against PostgreSQL; that requires a separately approved disposable PostgreSQL check before live cutover.

The existing PostgreSQL administration helper remains an explicitly opted-in registration fixture; it must never target the live database. No new SQL runner applies the proposed migration automatically.

## Runtime after a reviewed cutover

The new executable always serves the temporary WebSocket relay plus existing registration/health endpoints. There is no durable-HTTP-versus-WebSocket feature-mode switch. Use the owner's existing literal environment and audience; no credential, tunnel, DNS, ingress or frontend change is needed here. The previous run-messaging-server.sh launcher remains untouched for the currently installed binary; source review does not replace or restart that process.

Go timestamps every accepted queue event and expires it after exactly 30 * 24 hours. Recipient ACK deletes the row. A cleanup task in the same process runs on startup and each minute with bounded batches/deadlines. Reads and writes exclude expired delivery regardless of cleanup lag. Cleanup cannot execute during outages; PostgreSQL MVCC/WAL/backups are not instant physical erasure.

There is no encrypted delivery-receipt lifecycle, permanent delivery status, automatic retry/backoff, persistent deduplication, contact-request system, server conversation state or KeyPackage inventory. Review the [frame and failure contract](MESSAGING_API.md) before client interoperability testing; source-only work does not inspect or modify the separate frontend.

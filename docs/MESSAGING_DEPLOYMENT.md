# Relay migration review and deployment

New source-only work: authenticated Signal public-prekey distribution is complete.
Revised migration 006 and optional restricted-role grants remain unapplied. The
new binary was verified against both base and extended schemas; it never applies
SQL. Review [the exact changes and proposed policies](SIGNAL_PREKEYS.md) before
coordinated migration/deployment. The following records describe earlier live
activations, which this implementation did not replace.

Status: migration and matching relay activated on 2026-11-02 at 14:56 (Asia/Manila), after the owner explicitly instructed "do the migration now". Current source `5fd593f3253e38d2e5ea5c7ab490581ac0135eec` is running. The separately authorized username-column removal was applied at 15:45 (Asia/Manila). Database `void2-0e2ee` contains only accounts/messages. Both existing accounts and keys were preserved; credentials and tunnel configuration are unchanged.

Private backup/audit directory: `/home/void0000/.local/state/void-backend/deployments/20261102T144530+0800-two-table-relay`. It contains the full pre-migration dump, earlier executable, account/catalog snapshots, migration hashes, private verification source/logs and activation results. The temporary connection file was removed after verification.

The exact migration was first applied to a full isolated restore of the live backup. All restored row digests matched. Applying the migration preserved the account table OID, row/key digests, ownership, ACLs, UUID/default/key/index/constraint metadata, with only username nullability relaxed as approved. A separate synthetic PostgreSQL database proved the nonempty-data guard rolls back without changing schema or rows, and validated the new schema under a restricted runtime role.

Actual HTTP/WebSocket tests against that synthetic database passed registration without username, named registration, audience-bound v2 authentication, lookup, transient online exchange, committed offline persistence, fresh application instances over the same database, reconnect replay, recipient-only ACK deletion and expiration cleanup. They found that SELECT FOR UPDATE cleanup needed unnecessary UPDATE permission; cleanup now uses a bounded plain selection and DELETE with an outer expiry recheck. The schema/grants were unchanged. Full race tests and vet then passed.

The old backend stopped before live DDL. Exactly the reviewed 004 file was applied; no privilege script was needed for the existing postgres login. The new binary started successfully. All 17 local/public HTTP/tunnel probes and two real WebSocket route/unknown-account rejection checks passed, without creating any live accounts or messages. See [full verification](RELAY_VALIDATION.md).

Registration was updated at 15:28 (Asia/Manila) to remove username creation. Challenges accept only `auth_public_key`; named, empty and null username fields are rejected. Account responses contain only `void_id` and `created_at`. The existing nullable username column and its values remain intact. No migration or privilege change was applied. Full race tests, vet and 13 local/public checks passed; challenge probes did not create accounts or messages. The previous executable and this activation record are retained privately at `/home/void0000/.local/state/void-backend/deployments/20261102T152735+0800-username-free-registration`.

## Applied username-column removal (005)

The owner explicitly authorized removing username from the table. The live
catalog showed only the column's own CHECK dependency. A new private full dump
was restored into an isolated Unix-socket-only PostgreSQL 18.6 cluster; both
tables' row hashes and account identity hash matched. Exact migration 005 passed,
preserving account table identity, remaining attributes/constraints/indexes/ACLs
and all message rows and schema. A separate synthetic dependent view correctly
caused RESTRICT to abort atomically. Both Go validators passed under restricted
runtime permissions. Full race tests and vet also passed.

The complete [005 transaction](../migrations/005_drop_account_username.sql) is:

```sql
BEGIN;
SET LOCAL lock_timeout = '5s';
SET LOCAL statement_timeout = '1min';
ALTER TABLE public.accounts DROP COLUMN username RESTRICT;
COMMIT;
```

The backend stopped gracefully before live application. Accounts now contain
exactly `id`, `void_id`, `auth_public_key` and `created_at`. Both existing identity
rows, keys, timestamps and remaining metadata match their pre-migration hashes;
messages and their foreign keys/indexes are unchanged. Only username values and
their CHECK were removed. No CASCADE, role, credential, tunnel or network change
was used. Six live HTTP/readiness probes passed without account/message writes.

Private recovery archive, earlier executable, before/after snapshots and audit:
`/home/void0000/.local/state/void-backend/deployments/20261102T154015+0800-drop-account-username`. The removed names are retained in that recovery archive. Temporary
connection state and the isolated test cluster were removed after verification.

The remaining instructions describe future reviewed applications/recovery; they are not permission to replay this migration.

## Earlier applied relay migration (004)

[004_temporary_relay_queue.sql](../migrations/004_temporary_relay_queue.sql) is the complete exact reviewed database transaction, applied after the already-existing migrations 001–003. It is not automatically executed by startup, the standard test suite or a launcher.

The transaction sets a 5-second lock timeout and 1-minute statement timeout, checks the expected legacy schema, and locks accounts plus all nine legacy messaging tables before examining their contents. It aborts if any legacy data table contains even one row. Expired sessions, ACKed messages, blocks, claims and delivery events are all protected by this guard. Only the expected singleton internal schema marker(version 1) can be discarded. RLS must not hide rows from the guard. An unexpected schema or dependency aborts instead of using CASCADE.

After these guards, its operations are:

1. `ALTER TABLE public.accounts ALTER COLUMN username DROP NOT NULL;`
2. Drop the specific circular foreign key `key_packages_request_fk`.
3. Drop the empty old `messages`, `conversations`, `message_requests`, `key_packages`, `blocks`, `sync_events`, `sync_cursors`, `sessions` and `messaging_schema` tables, in dependency order. No CASCADE is used.
4. Create the new six-column `messages` queue with ownership foreign keys to accounts, a composite sender/event primary key, a 1 MiB payload bound and a fixed 720-hour expiry constraint. Both timestamps are supplied only by Go.
5. Create recipient-delivery and expiry indexes.

Accounts are not recreated. Their existing internal UUIDs/defaults, VOID-IDs, keys, creation timestamps and username values are preserved. No username backfill or column removal is performed. Historical migrations 001–003 remain unchanged. The superseded, unapplied optional-request-package draft is not part of this migration sequence.

The companion [004_temporary_relay_privileges.sql](../migrations/004_temporary_relay_privileges.sql) is separately reviewed and parameterized for an existing restricted runtime role. The new queue needs SELECT, INSERT and DELETE; replacing the old table does not transfer its old grants. Existing account registration/key-lookup privileges remain necessary. The owner-selected postgres login already has the needed access; do not change roles or passwords merely to apply this design.

## Recovery and future migration boundaries

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

Loopback HTTP/WebSocket fixtures create synthetic keys and in-memory accounts only. Adapter tests mock pgx transactions; they do not establish a database connection. Default PostgreSQL registration cases skip. Historical messaging database/restart runners and their old API tests were retired rather than run against the changed architecture. The subsequent approved private PostgreSQL verification and live cutover are recorded above. Repeat database tests only against explicitly approved disposable resources.

The existing PostgreSQL administration helper remains an explicitly opted-in registration fixture; it must never target the live database. No new SQL runner applies the migration automatically.

## Runtime after a reviewed cutover

The deployed new executable always serves the temporary WebSocket relay plus existing registration/health endpoints. There is no durable-HTTP-versus-WebSocket feature-mode switch. Use the owner's existing literal environment and audience; no credential, tunnel, DNS, ingress or frontend change is needed here. The previous run-messaging-server.sh launcher remains untouched. The new executable was started directly as ./void-server; its mode is always the temporary relay, so no old feature switch is needed. No autostart service was installed.

Go timestamps every accepted queue event and expires it after exactly 30 * 24 hours. Recipient ACK deletes the row. A cleanup task in the same process runs on startup and each minute with bounded batches/deadlines. Reads and writes exclude expired delivery regardless of cleanup lag. Cleanup cannot execute during outages; PostgreSQL MVCC/WAL/backups are not instant physical erasure.

There is no encrypted delivery-receipt lifecycle, permanent delivery status, automatic retry/backoff, persistent deduplication, contact-request system, server conversation state or KeyPackage inventory. Review the [frame and failure contract](MESSAGING_API.md) before client interoperability testing; this backend work did not inspect or modify the separate frontend.

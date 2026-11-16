# Account deletion change report

Implemented and tested on 2026-11-16 (Asia/Manila). No production migration,
account deletion, deployment, credential/role change, or Rust edit was performed.
The configured `.env` was read literally for isolated test connections and was
not modified. Temporary `void_test_` databases were removed after testing.

## Results

- `go test ./...`: passed with database integration opt-in disabled.
- `python3 scripts/test-integration.py --allow-database-admin --existing-login --admin-env .env -- go test ./... -race -count=1`: passed with PostgreSQL 18 integration enabled, including the final source/test changes.
- `go vet ./...`: passed.
- `go build -o /tmp/void-account-deletion-review ./cmd/void-server`: passed; installed server unchanged.
- `git diff --check`: passed.
- The existing restricted-grant fixture is explicitly skipped with `--existing-login`; all other integration tests run. No roles were provisioned for this rehearsal.

Coverage includes authorized deletion, owner/key/confirmation restrictions,
transcript domain/audience/ID tampering, expiry, failed/concurrent/copied/replayed
proofs, cross-socket replay, concurrent duplicate deletions, deletion racing
sends/claims/publication, rollback on cleanup failure, lost ACK recovery, fresh
services and database connections, Deleted lookup/authentication, ciphertext and
prekey cleanup, preservation of unrelated accounts/queued data, immutable
VOID-ID/key tombstones, and suppression of stale delivery errors on peer sockets.
There is no server-side conversation history to alter; remote histories are
never addressed by any deletion command. Actual process/database crash injection
and production restarts were not performed.

## Schema and security

Review-only migration `007_account_deletion.sql` adds nullable
`accounts.deleted_at` plus immutable-identity/tombstone triggers. Its separate
privileges file supplies lifecycle/cleanup grants where the existing connection
needs them. No migration runs at application startup or readiness.

Deletion requires a fresh, socket-bound, one-time, 30-second Ed25519 proof bound
to the ID, stored authentication public key, deletion domain, `DELETE`, and
configured backend audience. Database time and permanent unique constraints
reserve each deleted identity. Account locks serialize deletion with operations;
the timestamp and ciphertext/prekey cleanup commit atomically. Fresh proofs
recover the original result after uncertain acknowledgments. Normal relay
authentication, lookup, prekey distribution, and all account operations reject
Deleted accounts. Trigger protections also reject identity/tombstone mutation,
account DELETE, and TRUNCATE.

## Integration, deployment, and limitations

The [complete Rust contract and deployment procedure](ACCOUNT_DELETION.md)
defines every new request/response/error frame, exact signed bytes, reconnect and
uncertain-ACK handling, cache reconciliation, and existing-session behavior.

Deployment still requires explicit approval: stop every old backend, apply the
reviewed migration and needed grants, start only the matching binary, verify
readiness, and coordinate the separate Rust implementation. Never restore an
older binary/backup that ignores or loses tombstones. No real-account deletion
is needed for deployment smoke testing.

Deletion does not erase device history, already released bytes, PostgreSQL WAL,
MVCC remnants, or backups. Status Active/Unknown is a snapshot. Cross-process
idle sockets may close after a polling delay, but durable operation gates apply
immediately. Bounded writes can delay a concurrent deletion until their deadline.
Administrative owners can disable triggers; backup/recovery procedures must
preserve tombstones.

## Exact files changed

- `README.md`
- `cmd/void-server/main.go`
- `docs/ACCOUNT_DELETION.md`
- `docs/ACCOUNT_DELETION_VALIDATION.md`
- `internal/account/deletion.go`
- `internal/account/deletion_test.go`
- `internal/postgres/authentication.go`
- `internal/postgres/authentication_test.go`
- `internal/postgres/deletion.go`
- `internal/postgres/deletion_test.go`
- `internal/postgres/prekeys.go`
- `internal/postgres/prekeys_test.go`
- `internal/postgres/queue.go`
- `internal/postgres/queue_test.go`
- `internal/postgres/repository.go`
- `internal/postgres/repository_test.go`
- `internal/prekeys/service.go`
- `internal/queue/service.go`
- `internal/relay/accounts.go`
- `internal/relay/accounts_test.go`
- `internal/relay/prekeys.go`
- `internal/relay/queue.go`
- `internal/relay/server.go`
- `migrations/007_account_deletion.sql`
- `migrations/007_account_deletion_privileges.sql`
- `scripts/test-integration.py`
- `tests/integration/deletion_test.go`
- `tests/integration/postgres_test.go`

# Version-aware encrypted delivery preparation

This backend change prepares the generic encrypted delivery path for VOID Native Reply, Reactions and Delete for Everyone. It does not deploy the backend or apply a production migration. The exact new frames and reconnect behavior are specified in [MESSAGING_API.md](MESSAGING_API.md#application-protocol-support).

## Deployment prerequisites

1. Review and separately authorize [migration 008](../migrations/008_message_protocol_version.sql) for the intended database. Follow the existing backup/recovery procedure before applying it.
2. Stop and drain every old backend reader and writer before permitting versioned traffic. An already-running old binary ignores the additive version column and can deliver unsupported ciphertext. Its old strict schema validator also rejects the migrated layout on restart, so mixed-version operation is unsuitable.
3. Apply migration 008 once with the schema owner's existing access while the backend is stopped. It adds `messages.min_protocol_version integer NOT NULL DEFAULT 1` and a positive-version constraint. It preserves queued IDs, routing, ciphertext and timestamps, including expired rows. It does not change accounts, authentication keys, database roles, privileges, indexes or the 30-day retention rule. Existing table-level queue grants cover the new column; restricted deployments with column-specific grants need their existing operator to review access before startup.
4. Start only the new backend build. Its read-only schema validator requires the new column, default and validated constraint as well as effective access. Startup never applies migrations.
5. Verify legacy send/receive/ACK and the native client's reconnect advertisement before enabling version 2 feature sends. Native clients must support the `event_versioned` envelope and preserve local Signal processing order.

No deployment, service restart or production database change is part of this preparation. Do not roll back to an old backend while any event requiring a newer protocol remains queued: old readers cannot enforce the compatibility gate. Do not discard or relabel queued ciphertext to make rollback appear safe. Coordinate recovery with a version-aware build and the client's encryption state.

## Ordering decision and limits

The existing pump asks storage for the first unexpired event ordered by `(created_at,sender_void_id,event_id)` and permits one in-flight event per connection. The implementation checks compatibility only after reading that head. A blocked head remains stored and prevents every subsequent delivery to that recipient, including ordinary messages and independent senders. A protocol increase or a compatible reconnect can resume the same queue; ACK and expiration retain their existing effects.

This is deliberately conservative. Skipping unsupported Signal ciphertext and delivering later ciphertext could require client cryptographic guarantees that the backend cannot establish by inspecting encrypted bytes. No alternative skipping scheme is adopted. Backend tests establish head blocking, persistence, reconnect recovery and envelope compatibility using opaque fixtures; they do not prove native libsignal decryption behavior.

The retained order is a queue scan order rather than a global commit or Signal causal order. Creation timestamps are assigned before database commit, and existing tie-breakers remain unchanged. Expiration still removes eligibility after 30 days, so it can release later ciphertext even if an earlier event was never processed. A lost ACK can still stall a delivered event until reconnect or expiry. Clients remain responsible for encryption state, durable processing before ACK, duplicate handling and feature authorization. There is no capability discovery for offline recipients, multi-device tracking, retained delivery history or server-side deletion authorization.

## Isolated database verification

The integration runner creates a new `void_test_` database, applies the actual migration, and removes the database afterward. Before migration 008 it seeds both live and expired synthetic ciphertext and compares every existing field afterward, requiring version 1 defaults. PostgreSQL integration tests exercise the migrated validator, version metadata persistence, duplicate immutability, legacy SQL defaults, constraints, ACKs, expiration and recovery through a new service/adapter.

Use a separately provisioned disposable PostgreSQL 18 instance. Do not point the runner at production or read production credentials. `--existing-login` avoids creating database roles:

```bash
python3 scripts/test-integration.py --allow-database-admin --existing-login -- go test ./...
```

The command requires explicit test database administration opt-in. Its environment must identify only the isolated test instance. Ordinary `go test ./...` skips PostgreSQL integration tests unless the guarded disposable-database configuration is supplied.

## Validation results

Backend verification on 2026-11-18 passed `go test ./...`, `go test -race ./...` and `go vet ./...`. The ordinary test runs skip guarded PostgreSQL integration tests.

A separate full `go test ./...` run passed against a newly initialized standalone PostgreSQL 18 cluster with a fresh data directory, unique loopback port and socket, and no inherited database connection settings. The runner verified the actual migration against existing live and expired synthetic ciphertext before running the suite. It used `--existing-login`, so `TestPostgresRuntimePrivileges` was skipped because the restricted-role fixture was intentionally absent; all other PostgreSQL integration tests ran. The disposable database and standalone cluster were removed afterward. No production database, account or service was accessed.

These are backend transport/storage checks using synthetic opaque payloads. They do not validate native libsignal decryption, reply/reaction/delete application semantics, or client feature authorization.

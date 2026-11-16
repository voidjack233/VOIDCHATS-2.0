# Account deletion migration and activation

Applied on 2026-11-16 at 16:59 (Asia/Manila), after the owner instructed
"start migration". Backend source `9e6b6d5b54808f2f85672a2eb5616a150dda28fb` is running on the existing
loopback listener with migration 007 applied to `void2-0e2ee` (PostgreSQL 18.6).
Local readiness and the public WebSocket route passed verification at 17:01.

## Preservation and verification

Before live application, a private full database dump was restored to a newly
created, isolated `void_test_deletion_rehearsal_` database. The exact committed
migration passed there, preserving all restored records. The matching Go account,
queue, and prekey validators passed. The isolated database was then removed.

The old backend stopped gracefully before live DDL. A second full recovery dump
and before/after record digests were taken while backend traffic was stopped.
The live migration committed successfully. All four table digests matched:

| Table | Records at migration | Preservation |
| --- | ---: | --- |
| `accounts` | 13 | UUIDs, VOID-IDs, authentication public keys, and creation times unchanged |
| `messages` | 13 | Every queued ciphertext record unchanged |
| `signal_prekey_inventory` | 11 | Every public inventory record unchanged |
| `signal_prekey_claims` | 6 | Every claim/retry record unchanged |

All 13 existing accounts remained Active (`deleted_at IS NULL`). No account was
registered, modified, or deleted as a smoke test. The optional privilege script
was unnecessary for the configured connection and was not applied. Credentials,
`.env`, roles, cluster, tunnel routes, and listener settings were unchanged.

After migration, all three Go schema/access validators passed against the live
database. The reviewed executable was atomically installed and started with the
previous process environment. Normal relay processing then resumed.

Checks passed:

- Local `/healthz` and `/readyz`: HTTP 200.
- Local WebSocket ordinary authentication for a verified nonexistent ID:
  `authentication_failed`.
- Local WebSocket deletion-only request for that nonexistent ID:
  `deletion_authorization_failed`.
- Public WebSocket equivalents at the existing canonical endpoint: both passed.
- Running executable hash matches the reviewed build from clean committed source.
- `.env` hash matches its pre-migration value.

Health/readiness are checked locally. The unchanged public tunnel routes only
`/dm9pZG5vdGhpbmduZXNz` to this backend; public root `/healthz` belongs to another
service. No tunnel configuration was changed to expose backend health routes.

## Recovery artifacts and running build

Private recovery/archive directory (owner-only permissions):

`/home/void0000/.local/state/void-backend/deployments/20261116T165601+0800-account-deletion`

It contains `before-migration.dump`, the successfully rehearsed preliminary
archive, the previous executable, the reviewed new executable, exact migration
hash, before/after schema dumps and record digests, schema-validator source and
logs, local/public probe logs, and activation metadata. Credentials are not
written into the audit metadata. Runtime output continues in
`/tmp/void-server.log`. The current process started with PID 173421.

- Source: `9e6b6d5b54808f2f85672a2eb5616a150dda28fb`
- Installed binary SHA-256: `ed54a71e95b6d37f3d9addf851454c880f1a3b18411f3de3271c82320ebcd2c0`
- Applied migration 007 SHA-256: `b83f4c364667fe013554b25b0cb8223ada1308caff4462f265dd11eae8329d4d`

The backup retains pre-migration data under the owner's existing retention policy.
Do not start a binary that ignores deletion state or restore an older backup over
live data. Future recovery must preserve/replay every committed tombstone before
admitting traffic. Physical WAL/backup retention and device history remain
outside account deletion. See the [complete protocol and lifecycle
contract](ACCOUNT_DELETION.md) and [implementation test
report](ACCOUNT_DELETION_VALIDATION.md).

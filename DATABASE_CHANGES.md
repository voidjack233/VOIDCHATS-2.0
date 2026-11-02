# Previous database and environment changes

This report records the initial changes, the user's instruction to stop database administration, and subsequent explicitly authorized corrections. The initial review used read-only queries with `default_transaction_read_only=on`.

## Persistent database changes

On existing database `void2-0e2ee`:

- Created login role `void_accounts_app` with a generated random password, `NOSUPERUSER NOCREATEDB NOCREATEROLE NOINHERIT NOREPLICATION NOBYPASSRLS`, and no role memberships.
- Granted it `CONNECT` on the database and `USAGE` on `public`.
- Granted column-level `INSERT (void_id, username, auth_public_key)` and `SELECT (void_id, username, created_at)` on existing `public.accounts`.
- Revoked `TEMPORARY` on `void2-0e2ee` from `PUBLIC`. This changes permissions for other roles relying on that implicit grant.

Read-only inspection after the stop instruction confirmed this role and these grants remain. No development account rows were inserted, deleted, or updated. The existing database, schema, table definitions, constraints, and indexes were not replaced or changed.

## Disposable integration resources

Before the stop instruction, the integration runner created these databases and corresponding temporary login roles:

| Database | Role |
| --- | --- |
| `void_test_36830f21827dd3a8` | `void_test_app_36830f21827dd3a8` |
| `void_test_928b35affc212f35` | `void_test_app_928b35affc212f35` |
| `void_test_36dd8b3a1b139994` | `void_test_app_36dd8b3a1b139994` |

Each received a new `public.accounts` fixture and restricted grants. Test registrations were inserted only there. The runner dropped all three databases and roles on completion. Read-only inspection confirmed no `void_test_*` databases or `void_test_app_*` roles remain. No runner was active when the stop instruction was handled.

## Environment-file changes

- Initially changed the supplied `.env` permissions to `0600`, renamed it to `.env.admin.local`, and wrote a new `.env` using the generated application credentials and corrected database name.
- At the subsequent read-only review, `.env` and `.env.admin.local` were byte-for-byte identical, both mode `0600`, and both contained the supplied `postgres` connection fields. Neither file has been modified, renamed, restored, or deleted in response to the stop instruction. No passwords are reproduced here.
- Added `.env.example` with placeholder credentials and ignored actual environment files in `.gitignore`.

## Configuration findings at the stop-instruction review

- The current `.env` has `PGDATABASE=void2.0e2ee`. That database does not exist; the inspected existing database is `void2-0e2ee`. Recommendation: the owner should correct the database name when ready. No correction was applied after the stop instruction.
- The current `.env` has `PGUSER=postgres`, a superuser. The Go server intentionally rejects an administrative runtime role. Recommendation: the owner should choose/configure a restricted application credential and review the role/grants listed above. No password or role changes were made after the stop instruction.
- An additional sensitive `.env.admin.local` remains. Recommendation: the owner should decide how to retain or remove it. It has been preserved untouched.

Database administration remains with the user. The integration runner now refuses to connect without an explicit administration opt-in; PostgreSQL integration tests are disabled unless separately opted in. Current verification uses unit tests and mocks only.

## Subsequent authorized environment edit

The user later explicitly authorized touching `.env` while leaving PostgreSQL unchanged. Under that permission, only the database-name assignment was changed:

```diff
-PGDATABASE=void2.0e2ee
+PGDATABASE=void2-0e2ee
```

All other file bytes, including the existing username/password, and its file permissions were preserved. `.env.admin.local` was not changed. A read-only startup check now connects successfully but rejects the `postgres` superuser as a runtime role. No database administration or application inserts were performed during this check. The owner must supply credentials for an existing suitable application role or separately authorize any PostgreSQL changes; none are assumed.

## Subsequent authorized role removal and login choice

The user explicitly instructed: "right now drop the void account app role... use postgress thats my account". Read-only dependency inspection confirmed `void_accounts_app` owned no database objects and had only the previously listed grants. The following targeted transaction removed those grants and the role:

```sql
BEGIN;
REVOKE INSERT (void_id, username, auth_public_key),
       SELECT (void_id, username, created_at)
ON TABLE public.accounts FROM void_accounts_app;
REVOKE USAGE ON SCHEMA public FROM void_accounts_app;
REVOKE CONNECT ON DATABASE "void2-0e2ee" FROM void_accounts_app;
DROP ROLE void_accounts_app;
COMMIT;
```

Verification confirmed the role no longer exists and the existing account column definitions, defaults, constraints, and row count were unchanged. The current `.env` contents and permissions were unchanged. No `DROP OWNED` or cascade deletion was used. The earlier `PUBLIC TEMPORARY` revocation was not reversed because that was outside this request.

The Go validator now accepts the configured `postgres` login. It checks required registration access and the existing schema, without rejecting additional role privileges. This supersedes the earlier startup restriction and credential-blocker findings above. Future PostgreSQL administration still requires separate explicit approval.

## Durable messaging migration preparation: 2026-11-02

The messaging handoff requested additive SQL for owner review. Prepared
`migrations/001_messaging.sql`, `migrations/002_sessions.sql`, and separate
parameterized privilege scripts. These files have **not** been applied to the
existing database. No account rows, table definitions, credentials, roles,
grants, database settings or real environment files were changed during this
implementation. Normal startup performs only read-only schema/access validation;
enabling messaging requires the owner-reviewed migrations first. Database tests
remain separately opted in against an approved disposable `void_test_` database.
See [deployment instructions](docs/MESSAGING_DEPLOYMENT.md).

## Owner-approved durable messaging application: 2026-11-02

The owner answered **"yes i approve"** to isolated PostgreSQL verification,
application of the reviewed additive SQL and activation only after tests pass.
The subsequent "continue" retained that approval. Verification ran in a private,
Unix-socket-only PostgreSQL 18 cluster, never against existing accounts.

Real PostgreSQL testing found an ambiguous unary-minus parameter in the receipt
reservation update. An explicit integer cast fixed it in source commit
`bad8a72`; all nine database integration cases and the real HTTP/backend plus
PostgreSQL restart test then passed, including race checks.

A full custom-format backup was created at
`/home/void0000/.local/state/void-backend/deployments/20261102T084024+0800/database.before.dump`
and restored into a separate database in the private test cluster. Account data
matched exactly. The restored username CHECK differs only by flattened
associative AND parentheses; the remaining account catalog lines matched.

The following approved files were applied sequentially with `psql -X
--no-password -v ON_ERROR_STOP=1 -f <file>`, using the existing configured login
without exposing connection credentials:

- `migrations/001_messaging.sql`
- `migrations/002_sessions.sql`

These added nine messaging/session tables. The existing `postgres` login
already has the required privileges, so neither privilege script was needed;
no role, password, grant or credential changes were made. The real `.env` bytes
and permissions were left intact. Account data, columns, defaults, existing
checks and unique indexes matched their exact pre-application digests; both
existing accounts and their key bindings were preserved. New foreign-key
references and their internal dependencies are expected additive objects.

The new tables' column/default/constraint/index catalog digest matches the
tested isolated migration catalog. Source commit `bad8a72` is running with
durable messaging enabled through `scripts/run-messaging-server.sh`; the
ordinary application still performs no automatic migrations. See
[the deployment evidence](docs/MESSAGING_VALIDATION.md).

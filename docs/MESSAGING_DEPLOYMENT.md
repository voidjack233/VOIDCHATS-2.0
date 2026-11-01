# Durable messaging: review and deployment

The durable APIs are implemented in this checkout and remain disabled by default.
No live migration has been applied, and the running Go backend is unchanged.
On 2026-11-02 (Asia/Manila), the authorized one-line Cloudflare prefix expansion
was backed up, validated and applied, and the tunnel was restarted. Public new
API paths still return the old backend's JSON `404`. See
[the exact verification and deployment record](MESSAGING_VALIDATION.md).

The database policy in [DATABASE_CHANGES.md](../DATABASE_CHANGES.md) and
[README.md](../README.md) keeps schema administration with the owner. Review the
exact migration and grants before a separately authorized application. Startup
and readiness perform read-only validation; the server does not run migrations,
create roles, alter account keys or repair a missing schema.

## Preserve the current installation

Keep the current PostgreSQL database, login/password, `public.accounts`, account
VOID-IDs, and public-key bindings. The new tables reference existing account
VOID-IDs; no re-registration or identity replacement is necessary. Keep real
`.env`, database URLs, tunnel credentials and private keys out of Git and logs.

Before applying reviewed changes, take and verify an owner-managed database
backup, record the current server executable/configuration, and back up the
Cloudflare configuration. SQL changes are additive. Rolling back the executable
must preserve the new tables and queued ciphertext so it can be recovered;
dropping messaging tables is destructive and is not a rollback instruction.

## Exact additive SQL to review

Review these files in order:

1. [001_messaging.sql](../migrations/001_messaging.sql) creates
   `void_messaging_schema`, `void_key_packages`, `void_requests`,
   `void_conversations`, `void_blocks`, `void_messages`, `void_event_counters`
   and `void_events` in `public`. It adds foreign keys to the existing accounts
   table without altering that table. It includes uniqueness for sender/stable
   IDs, package fingerprints/claims/use, active unordered pairs, conversation
   sequences and per-account event cursors.
2. [002_sessions.sql](../migrations/002_sessions.sql) creates `void_sessions`
   and its account/expiry index. It stores token digests, account IDs and times.
3. [001_messaging_privileges.sql](../migrations/001_messaging_privileges.sql) and
   [002_sessions_privileges.sql](../migrations/002_sessions_privileges.sql)
   supply parameterized grants for the existing runtime role. The schema
   transactions do not apply those grants automatically. An existing superuser
   already has the necessary privileges; keep the owner's selected login.

Each schema migration is an explicit transaction and deliberately fails if its
table names already exist. Do not rerun it blindly, add `IF NOT EXISTS` to hide
an incomplete migration, or grant administrative privileges to satisfy startup.
The required runtime privileges are:

```sql
-- Review-only commands: replace through psql -v runtime_role=EXISTING_ROLE.
GRANT SELECT (void_id, username, auth_public_key, created_at),
      INSERT (void_id, username, auth_public_key)
ON TABLE public.accounts TO :"runtime_role";

GRANT SELECT ON TABLE public.void_messaging_schema TO :"runtime_role";
GRANT SELECT, INSERT, UPDATE ON TABLE
    public.void_key_packages,
    public.void_requests,
    public.void_conversations,
    public.void_blocks,
    public.void_messages,
    public.void_event_counters,
    public.void_events
TO :"runtime_role";

GRANT SELECT, INSERT, DELETE ON TABLE public.void_sessions TO :"runtime_role";
```

The existing database `CONNECT` and `public` schema `USAGE` permissions remain
necessary. No new sequence privileges, account `UPDATE`/`DELETE`, role changes,
password changes, or schema `CREATE` privilege are required. A superuser already
has the necessary access; keeping the owner's selected login does not require
creating a new role or changing `.env`. Session expiry cleanup deletes only
expired sessions for the account obtaining a new session. Messaging tables have
no runtime delete path; retained data is not a backup substitute.

## Enable the reviewed build

After applying the reviewed schema/grants, set these process environment values
for the new server process, retaining the existing environment file and secrets:

```text
VOID_MESSAGING_ENABLED=true
VOID_API_AUDIENCE=https://api.voidchats.xyz/dm9pZG5vdGhpbmduZXNz
VOID_HTTP_ADDR=127.0.0.1:8080
```

The default is `VOID_MESSAGING_ENABLED=false`; merely replacing the executable
does not activate the APIs. With messaging enabled the server requires a
loopback listener, validates the new schema/access before listening, and adds
the same validation to readiness. A missing migration fails startup with a safe
error rather than partially enabling messaging. The audience is the exact HTTPS
API base URL without a trailing slash/query/fragment, included in authentication
proofs. Local test HTTP is a transport harness, not an audience to configure for
real clients.

Enabling durable messaging removes the legacy `/dm9pZG5vdGhpbmduZXNz/ws`
endpoint (`404`). That old online relay does not check accepted conversations or
durable blocks and must not coexist as a bypass. With the feature disabled the
earlier relay mode remains available; consult [WEBSOCKET.md](../WEBSOCKET.md) for
its separate, weaker delivery contract.

PostgreSQL messaging transactions take one namespaced advisory lock across
processes, including sync/list operations. This serializes message/sequence and
event allocation in commit order and favors a simple correctness model over
high throughput. Scale requires measuring lock contention and designing finer
locking without introducing late-committing earlier cursors. HTTP authentication
challenges and rate counters remain process-local; do not add replicas without
challenge routing and coordinated admission controls.

Review the [current storage limits](MESSAGING_API.md#errors-limits-and-retention)
and set up disk/WAL/backup monitoring before traffic. The byte limits cover
encoded records, not total physical disk use, and there is no aggregate bot-user
storage budget. Lifetime tombstones eventually cause backpressure even when all
events are acknowledged. Raising caps or introducing cleanup requires a reviewed
retention policy that preserves offline delivery and stable-ID semantics.

## Cloudflare route change

The historical ingress expression allowed only
`^/dm9pZG5vdGhpbmduZXNz/(accounts|challenges|ws)$`. The authorized replacement is
now installed before broader rules for this same hostname:

```yaml
- hostname: api.voidchats.xyz
  path: ^/dm9pZG5vdGhpbmduZXNz(/.*)?$
  service: http://127.0.0.1:8080
```

Preserve the hostname, backend port, all unrelated ingress entries, tunnel
identity and credentials. This prefix routes registration and the new APIs
together. It does not expose `/healthz` or `/readyz`; those remain loopback-only.
Review a diff against the backed-up file, validate the ingress configuration,
and restart the existing tunnel only after the exact edit is authorized. Do
not add a broad hostname rule that could capture the other project's routes.

The applied patch is [cloudflared-ingress.patch](../deploy/cloudflared-ingress.patch)
and a portable rule fragment is [cloudflared-ingress.yml](../deploy/cloudflared-ingress.yml).
These fragments are not complete tunnel configuration files. The installed
configuration preserved every other byte and all 31 unrelated ingress rules.

HTTPS terminates at Cloudflare. Keep the Go listener on `127.0.0.1:8080` and
the tunnel-to-listener hop local. Forwarding headers are ignored: never trust
arbitrary `X-Forwarded-For` or `CF-Connecting-IP` values as proof of client IP.
The shared proxy IP is especially relevant to registration and unauthenticated
authentication limits; authenticated messaging additionally limits each verified
account. In-process limits do not replace edge flood protection.

## Client rollout and acceptance checks

Roll out native signing and verified KeyPackage publication before expecting
the recipient to be discoverable for encrypted setup. An account can exist
without an available KeyPackage. Treat `key_package_unavailable` as an
actionable requirement to run the updated recipient app and publish fresh
packages.

Use two distinct synthetic/disposable accounts for end-to-end checks. The
backend can prove durable opaque delivery without decrypting anything, but the
phone/LDPlayer test must separately demonstrate native MLS verification,
actual encryption/decryption, outbox/checkpoint durability and local history.
Do not reuse production accounts or keys for automated tests.

After the owner deploys the reviewed build, migrations and routing:

1. Check local liveness/readiness and existing registration behavior.
2. Call public `POST /auth/challenges` using an existing test VOID-ID. A Go
   listener responding locally does not establish that Cloudflare forwards it.
3. Authenticate both synthetic identities, discover each peer, publish and
   atomically claim packages, create/accept one invitation and exchange opaque
   application events in both directions.
4. Disconnect a recipient, post a message, restart the backend, then sync and
   verify it remains available until a durable ACK. Retry a lost send response
   with identical stable ID and bytes; it must return the original message.
5. Verify rejected ACKs produce rejection rather than delivery, unauthorized
   access fails, and a block stops requests, claims and delivery both ways.
6. Record deployed commit/executable identity, applied migrations, exact public
   statuses and client test results. Do not call the mobile feature complete
   from server-side opaque-byte tests alone.

The client should sync on resume and poll in the foreground approximately every
three seconds while draining pagination promptly. WebSockets and background push
are not prerequisites for this flow. Keep a durable per-account sync checkpoint;
the cursor and ACK are separate operations, as described in
[MESSAGING_API.md](MESSAGING_API.md).

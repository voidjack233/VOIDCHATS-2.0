# VOID-NOTHINGNESS

A native Go modular monolith for account initialization and durable one-to-one opaque messaging on Ubuntu 24.04 and PostgreSQL 18. The server preserves the existing `public.accounts` table and never performs schema administration at startup. See [WEBSOCKET.md](WEBSOCKET.md) for the earlier online-only relay, which is disabled in durable messaging mode.

Durable messaging provides short-lived bearer sessions, exact VOID-ID discovery, public MLS KeyPackage publication/claims, invitations, acceptance/decline/blocks, opaque messages, and ordered offline sync with durable acknowledgments and duplicate-safe retries. See the [exact API contract and examples](docs/MESSAGING_API.md) and [migration/deployment instructions](docs/MESSAGING_DEPLOYMENT.md). On 2026-11-02 the owner approved the migrations and activation; source commit `bad8a72` was the initial durable messaging deployment. Fresh installations still default to **disabled** until their SQL is reviewed/applied and the mode is explicitly selected. The server never runs migrations itself.

The current build uses the owner-approved table names `accounts`, `sessions`, `key_packages`, `message_requests`, `conversations`, `blocks`, `messages`, `sync_cursors`, `sync_events` and `messaging_schema`. Existing durable messaging installations need migration [003_clear_table_names.sql](migrations/003_clear_table_names.sql) together with this build. It renames existing tables and their constraints/indexes, preserving data, account identities and grants; API paths and fields are unchanged. Fresh installations apply schemas 001 and 002, their historical privilege files, then 003. See the deployment instructions for the exact order and coordinated backend restart.

## Run locally

Run the commands below from the repository root on the `backend` branch of VOIDCHATS-2.0. It includes the durable messaging implementation, migrations and handoff documentation. The real `.env` is excluded from Git. For a new checkout, create your local `.env` from the settings in `.env.example` using an existing owner-approved connection; preserve an existing environment file.

Go 1.26.5 and PostgreSQL 18 are required. The owner chose the existing `postgres` login in `.env` and database `void2-0e2ee`. The backend accepts that configured login and checks the access needed for registration. The existing username and password have not been changed. See [DATABASE_CHANGES.md](DATABASE_CHANGES.md) for the record of database/environment changes, including the explicitly authorized removal of `void_accounts_app`.

```bash
go build ./cmd/void-server
./scripts/run-messaging-server.sh
curl --fail http://127.0.0.1:8080/healthz
curl --fail http://127.0.0.1:8080/readyz
```

The default listener is `127.0.0.1:8080`. `SIGINT` or `SIGTERM` stops accepting connections and allows up to 10 seconds for active requests. Startup reports safe, separate errors for a missing database, rejected credentials, schema mismatch, and missing required access. Readiness repeats these read-only checks; liveness does not depend on PostgreSQL. The application performs no database administration. Database and `.env` credential changes require the owner's explicit approval of the exact proposed commands or edits.

`.env.example` documents the available settings; configuration and credential changes belong to the owner. The application reads literal `KEY=VALUE` assignments (optional matching quotes), never shell commands or interpolation. Process environment overrides the file. `-env-file /path/to/file` selects another file; `-env-file ''` uses process environment alone. `DATABASE_URL`, if set, takes precedence over the `PG*` connection fields. Connection strings and database diagnostics are never logged.

Settings and defaults are listed in `.env.example`. Connection pooling defaults to 8 connections. Registration HTTP bodies are limited to 4096 bytes, headers to 8192 bytes, concurrent registration/readiness handlers to 64, pending challenges to 4096, and tracked rate-limit clients to 4096. Enabled messaging has separate body, admission and authenticated account rate budgets, described in its API contract. Header/body/idle deadlines and database statement/lock/connect timeouts bound stalled operations. Request contexts reach PostgreSQL and ID generation.

## Cloudflared

The existing `void0000-main` tunnel forwards the `/dm9pZG5vdGhpbmduZXNz` namespace to the Go backend. Available endpoints depend on the running build and feature mode:

- `POST https://api.voidchats.xyz/dm9pZG5vdGhpbmduZXNz/challenges`
- `POST https://api.voidchats.xyz/dm9pZG5vdGhpbmduZXNz/accounts`
- `GET wss://api.voidchats.xyz/dm9pZG5vdGhpbmduZXNz/ws` (WebSocket upgrade; requires the Phase 2 backend)
- The durable HTTP APIs in [MESSAGING_API.md](docs/MESSAGING_API.md), after reviewed migrations and deployment with `VOID_MESSAGING_ENABLED=true`; that mode removes the older `/ws` endpoint.

The owner-approved `/ws` route was added on 2026-11-02. The later durable-messaging handoff expanded it to the complete API prefix; that one-line change was backed up, validated and applied, and Cloudflared was restarted. After isolated PostgreSQL and full process-restart verification passed, the owner-approved migrations and durable backend were activated. New protected public messaging routes return JSON `401` without a bearer token; the earlier `/ws` returns `404` in this mode. See [the verification/deployment record](docs/MESSAGING_VALIDATION.md).

The following ingress rule is installed in `~/.cloudflared/config.yml`, before the existing routes for `api.voidchats.xyz`:

```yaml
- hostname: api.voidchats.xyz
  path: ^/dm9pZG5vdGhpbmduZXNz(/.*)?$
  service: http://127.0.0.1:8080
```

All 31 previous ingress rules are preserved. The original configuration is backed up at `~/.cloudflared/config.yml.before-backend`. No tunnel credentials or DNS records were changed. Health and readiness for this backend remain local at `http://127.0.0.1:8080/healthz` and `/readyz`; the public hostname's existing health routes still belong to the other project.

For the migrated durable backend, start `./scripts/run-messaging-server.sh` from the project directory, then run the tunnel in another terminal:

```bash
cloudflared tunnel --config "$HOME/.cloudflared/config.yml" --no-autoupdate --metrics 127.0.0.1:20241 run void0000-main
```

Both processes must remain running; no autostart service was installed. API clients should send a descriptive `User-Agent`, such as `VOIDCHATS/2.0`. The current Cloudflare configuration returned error 1010 for Python's default client header; requests with `VOIDCHATS/2.0` passed. See [Cloudflare's error 1010 guidance](https://developers.cloudflare.com/support/troubleshooting/http-status-codes/cloudflare-1xxx-errors/error-1010/). Through this local tunnel, all clients currently share the backend's per-IP rate limit because forwarding headers are ignored.

## Database connection

The backend uses the existing connection chosen by the owner. Startup requires `SELECT` access to `void_id`, `username`, `auth_public_key`, and `created_at`, and `INSERT` access to `void_id`, `username`, and `auth_public_key`. Phase 2 reads the existing public authentication key by VOID-ID; it does not alter the table. Additional privileges, ownership, and administrative role flags do not block startup. It never creates roles, grants permissions, or repairs the schema. The current connection uses `postgres`, as explicitly requested. Earlier administrative changes and authorized corrections are recorded in [DATABASE_CHANGES.md](DATABASE_CHANGES.md).

`id` is omitted from every insert, so PostgreSQL's `uuidv7()` default creates it. The server returns only public fields. Username uniqueness is deliberately not required by the existing schema.

## Registration API

All POST requests require `Content-Type: application/json`. Fields must be exactly those specified below. Nulls, duplicate or unknown fields, trailing JSON, noncanonical base64, and oversized bodies are rejected. Keys/signatures use padded standard base64; challenge IDs use unpadded base64url.

1. `POST /dm9pZG5vdGhpbmduZXNz/challenges`

   ```json
   {"username":"alice","auth_public_key":"<base64 of the 32-byte Ed25519 public key>"}
   ```

   Returns `201`:

   ```json
   {"challenge_id":"<opaque token>","message":"<base64 of exact bytes to sign>","expires_at":"2026-11-01T09:00:00Z"}
   ```

   Usernames must be valid UTF-8, 1–32 Unicode code points, have no surrounding whitespace, and contain no control/format characters. Keys must be canonical, nonidentity prime-order Ed25519 points. Challenges contain 256-bit random tokens plus independent 256-bit nonces and expire after two minutes by default. The domain-separated signed message binds the token, expiry, public key, username, and nonce.

2. Locally decode `message` and sign those bytes with the corresponding Ed25519 private key. For a Go client:

   ```go
   message, err := base64.StdEncoding.DecodeString(challenge.Message)
   if err != nil { /* handle error */ }
   signature := ed25519.Sign(privateKey, message)
   encodedSignature := base64.StdEncoding.EncodeToString(signature)
   ```

   Clients should also verify the message's `VOID-NOTHINGNESS:register:v1` domain and the expected username/public-key binding before signing, and use a trusted server connection. Private keys stay on the client; the API accepts no private-key field.

3. `POST /dm9pZG5vdGhpbmduZXNz/accounts`

   ```json
   {"challenge_id":"<opaque token>","signature":"<base64 of the 64-byte signature>"}
   ```

   Returns `201`:

   ```json
   {"void_id":"110559210000000000","username":"alice","created_at":"2026-11-01T08:58:12.123456Z"}
   ```

   `void_id` is a positive signed 64-bit integer represented as a decimal JSON string. The timestamp comes from PostgreSQL. Registration creates no authenticated session, cookie, or token. VOID-IDs are public identifiers and never authentication secrets.

Registration challenges are atomically consumed before signature verification and database access. Well-formed invalid signatures, expired challenges, concurrent replay, database failures, and interrupted in-flight registrations cannot reuse them. Obtain a fresh challenge after a failure. A disconnected client can miss a successful commit; retrying with a fresh proof then returns `409` if that key was registered. Registration issues no session; returning users authenticate through the [HTTP challenge/session flow](docs/MESSAGING_API.md) in durable mode, or the [WebSocket handshake](WEBSOCKET.md) in legacy relay mode. Account recovery remains outside scope.

Errors have the shape `{"error":{"code":"invalid_proof","message":"Registration proof is invalid or expired."}}`.

| Status | Meaning |
| --- | --- |
| 400 | Invalid input or malformed JSON |
| 401 | Missing, expired, consumed challenge, or invalid signature |
| 409 | Public key already registered |
| 413 / 415 | Body too large / unsupported content type |
| 429 | IP rate limit or pending-challenge capacity; see `Retry-After` |
| 503 | Database/ID generation unavailable, readiness failure, or concurrency limit |
| 408 / 504 | Request canceled / deadline exceeded |

Both POST endpoints share a per-IP token bucket: 30 requests/minute with burst 10 by default. `RemoteAddr` determines the IP; forwarding headers are ignored. `GET /healthz` returns `200 {"status":"ok"}`. `GET /readyz` returns `200 {"status":"ready"}` or a safe `503` error.

## Snowflake IDs and deployment limits

IDs use 41 timestamp milliseconds, 10 worker bits, and 12 sequence bits. `VOID_WORKER_ID` is 0–1023 and `VOID_SNOWFLAKE_EPOCH` defaults to `2026-01-01T00:00:00Z`. The generator is concurrency safe, waits with cancellation on sequence exhaustion, detects backward clocks, rejects nonpositive IDs and timestamp overflow, and leaves state intact on failure. PostgreSQL uniqueness is the final guard; the service retries at most five VOID-ID conflicts and never overwrites an account.

Before running multiple generators, assign a unique worker ID to every simultaneously active process sharing the ID namespace, keep one immutable epoch, and coordinate worker reuse after restart. A restarted generator has no durable high-water mark: do not reuse a worker while a previous process can still run or before its last emitted wall time has passed. Clock rollback or collisions can temporarily stop registration. The 41-bit timestamp range lasts approximately 69 years from the epoch.

Challenges and limits are bounded, process-local memory. Restarting invalidates outstanding challenges. Multiple replicas would need challenge routing to their originating process and coordinated abuse controls before deployment. Basic in-process limits do not handle distributed traffic floods. Behind a proxy, all callers share the proxy's IP bucket unless a separately designed trusted-proxy policy is added.

The listener is plain HTTP for local development. Configure TLS termination before exposing it beyond loopback, and use `PGSSLMODE=verify-full` with a trusted certificate for a remote database. The original administrative credential backup is sensitive; keep it private and separate from the application process. This implementation has automated security tests, but has not had an external security audit.

## Project layout and verification

```text
cmd/void-server/        executable, HTTP timeouts and graceful shutdown
internal/account/      input/key validation, challenges and registration
internal/config/       literal environment loading and validated settings
internal/httpapi/      routing, strict JSON, limits and safe responses
internal/postgres/     pooled parameterized inserts and schema/role checks
internal/snowflake/    concurrency-safe public ID generator
tests/registration/    HTTP/service/Snowflake flow with an in-memory repository
tests/integration/     explicit disposable PostgreSQL integration tests
scripts/               earlier integration runner (disabled without explicit opt-in)
```

Application dependencies are `pgx/v5` (PostgreSQL driver/pool) and `filippo.io/edwards25519` (public-key group validation). Signature verification uses Go's `crypto/ed25519`; no custom curve arithmetic or signing implementation is used. The latter library provides the point operations used to reject degenerate public keys ([API documentation](https://pkg.go.dev/filippo.io/edwards25519)); UUID generation stays in [PostgreSQL 18](https://www.postgresql.org/docs/18/functions-uuid.html).

```bash
go fmt ./...
go vet ./...
go test ./...
go test -race ./...
go build ./cmd/void-server
```

Default tests use unit tests and mocks; PostgreSQL cases skip unless explicitly opted in against a disposable `void_test_` database. After the owner's approval, `python3 scripts/test-messaging-isolated.py --keep-running` provisioned a private PostgreSQL 18 cluster with Unix-socket access only, applied the migrations and restricted runtime grants, and passed all nine PostgreSQL integration cases with and without the race detector. It never reads the real `.env` or connects to the existing database. See the [verification record](docs/MESSAGING_VALIDATION.md) for the separate real-process restart check and reproducible commands.

The earlier Python integration runner is preserved for review. It refuses even to connect unless explicitly opted into database administration. Do not invoke its administration opt-in without the owner's separate permission; database creation, roles, grants, and cleanup belong to the owner.

Tests cover proof binding, invalid/degenerate keys, expiration, replay, concurrent consumption, duplicate keys/IDs, retry limits, cancellation, strict JSON/body limits, bounded admission/rate limiting, clock rollback/overflow/sequence handling, and concurrent Snowflake uniqueness. Registration flow tests compose the real HTTP handler, account service, and Snowflake generator with an in-memory repository: they verify public response fields, concurrent duplicate proofs, failed-persistence consumption, and fresh-proof recovery without connecting to PostgreSQL. A deterministic rate-limit regression test verifies concurrent requests cannot count the same refill interval twice. Opt-in PostgreSQL tests verify UUIDv7/default timestamps, persisted HTTP registration, privilege denial, conflicts, and a blocked insert canceled before persistence.

Earlier PostgreSQL results predate the stop instruction. The current approved verification also runs messaging and registration against the isolated cluster and restarts the actual test backend and PostgreSQL processes. Live verification uses non-mutating health, routing and authorization probes; no synthetic account or message is inserted into the owner's database.


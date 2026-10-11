# VOID backend

The Go API uses standard-library HTTP and the maintained pgx PostgreSQL driver. Public accounts persist in PostgreSQL; the in-memory repository remains for tests. The React Native debug diagnostic checks health only. Account initialization and native MLS identity generation remain separate.

## Run locally

Install 64-bit [Go 1.26 or newer](https://go.dev/dl/), Node.js, and Podman. From the repository root, create the ignored local environment once, preserving any existing password, then run:

```powershell
.\scripts\dev.ps1 machine
.\scripts\dev.ps1 up
.\scripts\dev.ps1 ready
.\scripts\dev.ps1 migrate
.\scripts\dev.ps1 api
```

On a Bash host, use the corresponding `bash scripts/dev.sh` actions. The API runs in the foreground; Ctrl+C shuts it down. Follow [the development guide](../docs/DEVELOPMENT.md) for credential generation, host setup, WSL compatibility, persistence checks, physical Android LAN access, and protected tunnels. [The verification report](../docs/development-verification.md) records actual checks and limitations.

The helper environment selects PostgreSQL at `127.0.0.1:15432`, container `void-dev-database`, and persistent volume `void-dev-postgres-data`. An existing native PostgreSQL server on 5432 remains independent. Direct Go commands do not load `.env`; load the intended environment before running them.

## Configuration and lifecycle

| Setting | Behavior |
| --- | --- |
| `VOID_HTTP_ADDR` | Optional full host:port override |
| `VOID_HTTP_HOST`, `VOID_HTTP_PORT` | Default `127.0.0.1`, `8080` when no full override |
| `VOID_ENV` | `development` by default; accepts `development`, `test`, or `production` |
| `PGHOST`, `PGPORT` | Direct Go defaults `127.0.0.1`, `5432`; development helpers select `15432` |
| `PGUSER`, `PGDATABASE` | Default `void_dev` |
| `PGPASSWORD` | Required server credential; never sent to the mobile app |
| `PGSSLMODE` | `disable` only for loopback; `verify-full` required for remote hosts |
| `PGSSLROOTCERT` | Optional CA certificate path for verified TLS |
| `VOID_DIAGNOSTIC_ADDR` | Separate health-only process; default numeric loopback `127.0.0.1:8091` |

Startup opens a pool capped at four connections, pings PostgreSQL, and checks the expected schema before binding the HTTP listener. Startup/connect deadlines are five seconds; repository operations use a five-second deadline and readiness uses two seconds. Database failures do not activate memory storage. Pool connections have bounded lifetime and idle time. Driver errors are not returned to clients or logged with credentials.

HTTP has a five-second header timeout, ten-second read/write timeouts, and a sixty-second idle timeout. Shutdown stops accepting connections and allows in-flight requests up to ten seconds to finish, then closes remaining connections if necessary. The database pool closes after HTTP shutdown. Startup and shutdown failures produce a nonzero exit status.

```text
backend/
  cmd/api/                 database/schema startup and HTTP lifecycle
  cmd/migrate/             explicit migration command
  cmd/diagnostic/          independent loopback health-only origin
  internal/account/        model, service, PostgreSQL repository, test memory repository
  internal/config/         explicit environment configuration
  internal/database/       pool, embedded migrations, schema verification
  internal/transport/http/ HTTP server, JSON handlers, readiness
  internal/voidid/          submitted-ID rules and earlier standalone ID helpers
```

## Account API

| Route | Result |
| --- | --- |
| `GET /health` | HTTP 200, `{"status":"ok"}` |
| `GET /ready` | HTTP 200, `{"status":"ready"}`; database failure gives 503, `{"status":"not_ready"}` |
| `POST /v1/accounts` | Creates an account with HTTP 201; duplicate VOID-ID gives 409 |
| `GET /v1/accounts/{void_id}` | Stored account with HTTP 200, or 404 when absent |

The client generates its VOID-ID and chat identity keypair locally and chooses a display name. It submits exactly these public fields:

```http
POST /v1/accounts
Content-Type: application/json

{
  "void_id": "client-generated-id",
  "display_name": "Jack",
  "identity_public_key": "encoded-public-key"
}
```

The response preserves those strings exactly and adds `created_at`, a server-assigned UTC timestamp:

```json
{
  "void_id": "client-generated-id",
  "display_name": "Jack",
  "identity_public_key": "encoded-public-key",
  "created_at": "2026-10-12T08:00:00.123456Z"
}
```

URL-encode the identifier when building its GET path. Duplicate VOID-IDs return JSON `{"error":"void_id already exists"}` and HTTP 409. PostgreSQL's UNIQUE constraint performs the atomic conflict check, including concurrent submissions; the repository does not overwrite the existing account. Display names and public keys are not uniqueness constraints.

All three fields must be valid UTF-8, nonblank strings without NUL. The submitted VOID-ID may contain at most 512 UTF-8 bytes. Requests are limited to 16 KiB; larger bodies receive HTTP 413. Malformed JSON, invalid UTF-8, unknown fields (including private-key fields or `created_at`), and trailing JSON values receive HTTP 400 before persistence. Errors have a consistent JSON `error` field; internal database details remain hidden. Request bodies and key material are not logged.

## Identifier and timestamp compatibility

The backend does not generate an account VOID-ID. `voidid.ValidateSubmitted()` checks the storage-compatible rules above without case folding, trimming, normalization, or a fixed-format conversion. PostgreSQL uses deterministic `C` collation for exact identifier comparison.

The earlier memory milestone accepted any nonblank submitted ID. PostgreSQL persistence adds the explicit 512-byte limit, valid UTF-8, and NUL rejection for its text/index representation. These rules do not select the final client identifier format. Future format validation belongs in `internal/voidid`.

Creation timestamps use UTC and microsecond precision before insertion so PostgreSQL round trips preserve the returned account exactly. The earlier `voidid.New()`, `Parse()`, and `Validate()` helpers for provisional 160-bit uppercase Base32 IDs remain standalone and are not used by account creation or retrieval. Account logic does not depend on that format.

## Migrations and checks

The migration command applies embedded, ordered SQL migrations in a transaction protected by an advisory lock and records checksums. Changed applied migrations or incompatible schema/history cause an error. Add a new migration instead of editing an applied one. SQL checkout line endings are fixed through `.gitattributes` so checksums remain consistent across Windows and Unix. API startup verifies the schema; it does not silently run migrations.

From `backend/`:

```sh
go fmt ./...
go vet ./...
go test ./...
go test -race ./...
```

The race detector requires a supported 64-bit target and C compiler. Real PostgreSQL tests are opt-in: run `.\scripts\dev.ps1 integration` or `bash scripts/dev.sh integration` from the repository root. They set `VOID_POSTGRES_TEST=1`, create isolated temporary schemas, and clean up only their own schemas. Without that flag, ordinary tests skip database integration tests.

With the API running, `.\scripts\dev.ps1 test` or `bash scripts/dev.sh test` checks the live routes with synthetic public fixtures. Its `--record PATH` and `--verify-record PATH` options compare the same account after an API and PostgreSQL-container restart; see [the persistence procedure](../docs/DEVELOPMENT.md#verify-persistence-and-automated-checks).

## Identity and exposure boundaries

The account model and database contain only VOID-ID, display name, identity public key, and creation time. The backend never generates, requests, transmits, logs, or stores the chat identity private key. Display names are cosmetic metadata. VOID-ID is a public identifier, not a password or encryption key. Public keys remain opaque strings until a separate encoding, algorithm, ownership-proof, and cryptographic-validation contract is defined.

Quick Tunnels use only `cmd/diagnostic` at loopback port 8091. This process has no database/service dependency and returns JSON 404 for account routes. Never point an unauthenticated Quick Tunnel at the account API. A named API tunnel requires Cloudflare Access configured before DNS publication and origin JWT validation for the intended team/audience, with credentials outside Git and the mobile app.

This milestone provides persistence and development connectivity. It does not authenticate account ownership or connect registration to the frontend. Application authentication, rate limiting, public-key verification, final identifier format, contacts, messaging/WebSocket, device linking, recovery, durable client key storage, and production deployment remain future work. A development Access policy does not implement those application guarantees.

# VOIDCHATS Go backend — phase 1

A **small account-initialization API**, not the former NOTE2EE or 2.0 backend. No container orchestration, message transport, chat history, token auth, push notifications, or MLS key distribution is included.

The October 2026 frontend inspection found:
- `src/startup/InitializingScreen.tsx` is currently a placeholder, not an API caller.
- `src/storage/types.ts` describes a **local** device identity with `accountVoidId: null` until registration.
- `src/profile/ProfileScreen.tsx` has no registered VOID-ID; contacts lookup and messages are disabled.
- Native OpenMLS keys and SQLCipher exist, but the MLS signing key is **not** a generic backend-auth credential. Expo Go cannot load the native cryptography/storage modules.
- The desktop SDK 58 upgrade may not yet be committed to GitHub. This backend branch does not change the frontend's dependency versions.

## Scope and ownership

The phone should generate a CSPRNG **numeric 16–20 digit VOID-ID** (store as a string) and a distinct Ed25519 **authentication** key pair. It chooses a changeable username. The server validates ownership using a signed server challenge and atomically registers the VOID-ID. The authentication private key stays in a protected native keystore boundary, never in SQL or ordinary JS persistence. This native auth-key interface is **not implemented** in the current frontend.

The server stores only `void_id`, `username`, `auth_public_key` (32 bytes), and `created_at`. E2EE identity, MLS key packages, groups, conversation data, and message queue semantics are **not decided by this API**.

## Run locally (Go + PostgreSQL)

Requires Go 1.23+, PostgreSQL, and a database provisioned outside the application. No Docker/Podman required. From `backend/`:

```sh
# Use your own database credentials. Do not commit them.
export DATABASE_URL='postgres://void:YOUR_PASSWORD@127.0.0.1:5432/void?sslmode=disable'
psql "$DATABASE_URL" -v ON_ERROR_STOP=1 -f migrations/001_accounts.sql
go mod tidy  # generates go.sum for the pinned PostgreSQL driver
go test ./...
go run ./cmd/void-server
```

PowerShell equivalents for the environment and migration:

```powershell
$env:DATABASE_URL = 'postgres://void:YOUR_PASSWORD@127.0.0.1:5432/void?sslmode=disable'
psql $env:DATABASE_URL -v ON_ERROR_STOP=1 -f migrations/001_accounts.sql
go mod tidy
go test ./...
go run ./cmd/void-server
```

Default bind: `127.0.0.1:8080`. Set `LISTEN_ADDR=:8080` explicitly if you need LAN access. `sslmode=disable` is **only** for local PostgreSQL. Provide HTTPS at a trusted reverse proxy and a TLS-protected DB connection in production. Rate limiting is currently per direct peer IP; do not expose publicly without an abuse-controlled reverse proxy.

## HTTP contract (JSON; version 1)

`GET /healthz`: process liveness. `GET /readyz`: checks DB connectivity.

**1. Request a registration challenge**

```http
POST /v1/registration/challenges
Content-Type: application/json

{"void_id":"13134123413412345678","username":"Alice","auth_public_key_base64":"<standard base64 Ed25519 public key>"}
```

Returns HTTP 201:

```json
{"challenge_id":"<opaque id>","challenge_base64":"<standard base64 bytes to sign>","expires_at":"<UTC timestamp>"}
```

**2. Sign and finalize**

The *native-held Ed25519 authentication private key* signs the **decoded bytes** of `challenge_base64` without modification, with ordinary Ed25519 (not a hash or MLS operation). Do not sign its base64 text. Submit:

```http
POST /v1/accounts
Content-Type: application/json

{"challenge_id":"<opaque id>","signature_base64":"<standard base64 64-byte signature>"}
```

Returns HTTP 201:

```json
{"void_id":"13134123413412345678","username":"Alice","created_at":"<UTC timestamp>"}
```

The one-use challenge expires in 5 minutes. Any failed attempt consumes it; the client should request another. HTTP 409 `identity_unavailable` means the chosen ID or public key already belongs to an account: generate another **random** ID where appropriate, and never overwrite existing credentials. Inputs are length bounded and JSON fields are strict. Username is trimmed; maximum 32 Unicode code points / 128 UTF-8 bytes. The DB prevents concurrent ID collisions.

## Deliberate omissions before frontend connection

1. Add separate secure Ed25519 auth-key generation, challenge signing, and **durable account binding** to the native client. Do not reuse the MLS signature key unless its security model is expressly designed for this.
2. Handle interrupted registration, backups/recovery, and restoration without silently creating a new identity. Do not display an unconfirmed VOID-ID as registered.
3. Add a signed **authentication/login** and session/reconnect protocol before user-specific operations, WebSockets, account changes, or online presence. Registration does **not** issue a login token.
4. Decide lookup privacy, first-contact message requests, MLS public key and key-package publication, and offline encrypted message retention *before* messaging endpoints.
5. Current in-memory challenge/rate-limit state assumes **one API instance**. For multiple replicas, move challenges into shared, one-time transactional storage. Registration is intentionally open to anyone who can solve a challenge; signed keys alone do not prevent bulk fake accounts or ID squatting. Apply layered anti-abuse defenses before public deployment.

No plaintext message body, MLS secret, password, or E2EE decryption material is accepted by this API. This is a **development foundation**, not a production-ready secure messenger.

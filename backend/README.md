# VOID backend

The backend is one Go API process using only the standard library. It supports account initialization and retrieval with temporary in-memory storage. The React Native frontend is not connected yet.

## Run and test

Install [Go 1.26 or newer](https://go.dev/dl/), then run from the repository root:

```sh
cd backend
go run ./cmd/api
```

The development address defaults to `127.0.0.1:8080`. Set `VOID_HTTP_ADDR` to override it, for example in PowerShell:

```powershell
$env:VOID_HTTP_ADDR = '127.0.0.1:9000'
go run ./cmd/api
```

Check the running API with `curl http://127.0.0.1:8080/health` (use `curl.exe` in Windows PowerShell). It returns HTTP 200, `Content-Type: application/json`, and `{"status":"ok"}`. Stop the process with Ctrl+C. The process also handles SIGTERM where supported.

Run checks from `backend/`:

```sh
go fmt ./...
go vet ./...
go test ./...
```

There are no third-party dependencies, so `go.sum` is not needed.

## Structure and lifecycle

```text
backend/
  cmd/api/                process startup, signals, and graceful shutdown
  internal/account/       account model, service, repository, and memory storage
  internal/config/        HTTP address from the environment
  internal/transport/http/ standard-library server and JSON API handlers
  internal/voidid/         isolated identifier validation and earlier ID helpers
  go.mod
```

The server sets a 5-second header timeout, 10-second read and write timeouts, and a 60-second idle timeout. Startup is logged after the listener binds successfully. Shutdown stops accepting connections and allows in-flight requests up to 10 seconds to finish, then closes remaining connections if necessary. Startup and shutdown failures produce a nonzero exit status.

## Account API

The client generates its VOID-ID and chat identity keypair locally, and chooses a display name. It submits only these three fields:

```http
POST /v1/accounts
Content-Type: application/json

{
  "void_id": "client-generated-id",
  "display_name": "Jack",
  "identity_public_key": "encoded-public-key"
}
```

Successful creation returns HTTP 201 and the stored account, with a server-assigned UTC timestamp:

```json
{
  "void_id": "client-generated-id",
  "display_name": "Jack",
  "identity_public_key": "encoded-public-key",
  "created_at": "2026-10-05T08:00:00Z"
}
```

`GET /v1/accounts/{void_id}` returns that account with HTTP 200, or HTTP 404 when absent. URL-encode the identifier when building the path. This endpoint is for verifying initialization in the current milestone.

For example, with the API running, use PowerShell:

```powershell
$accountRequest = @{
    void_id = 'client-generated-id'
    display_name = 'Jack'
    identity_public_key = 'encoded-public-key'
} | ConvertTo-Json
Invoke-RestMethod -Method Post -Uri 'http://127.0.0.1:8080/v1/accounts' -ContentType 'application/json' -Body $accountRequest
Invoke-RestMethod -Uri 'http://127.0.0.1:8080/v1/accounts/client-generated-id'
```

All three fields must be nonblank strings. Their submitted values are preserved exactly. Malformed JSON, invalid UTF-8, unknown fields (including private-key fields), and extra JSON values are rejected with HTTP 400 before storage. Requests are limited to 16 KiB; larger bodies receive HTTP 413. Errors use JSON such as `{"error":"void_id already exists"}`; internal errors are not exposed to clients.

Duplicate VOID-IDs receive HTTP 409. A mutex protects the map's check-and-insert operation, so concurrent submissions cannot overwrite the original account. Display names and public keys are not uniqueness constraints. A repository interface isolates storage from the service and handlers so a future database implementation can enforce the same behavior.

Storage exists only in this API process: restarting it loses all accounts. It is not shared across multiple processes.

## VOID-ID rules

Account initialization does not generate a VOID-ID on the backend. It calls `voidid.ValidateSubmitted()`, which currently requires only a nonblank client-supplied identifier. There is no case folding, trimming, or fixed-format conversion. Account logic does not depend on the earlier Base32 format, and future identifier rules belong in `internal/voidid`.

The earlier `voidid.New()`, `Parse()`, and `Validate()` helpers remain available for the provisional 160-bit uppercase Base32 format (`VOID-XXXXXXXX-XXXXXXXX-XXXXXXXX-XXXXXXXX`). They are not used by account creation or retrieval. That helper uses `crypto/rand.Reader`, returns available read errors without an insecure fallback, and retains its opaque `ID` type and tests.

Future persistence must enforce a UNIQUE constraint on the client-supplied VOID-ID. The backend returns a conflict rather than replacing the supplied identity or generating a replacement.

## Identity boundaries and deferred work

Display names are cosmetic metadata and need not be unique. VOID-ID is the public account identifier, not an authentication secret, encryption key, or chat identity key. The chat identity keypair belongs on the client/device; the backend must never generate, request, transmit, log, or store the identity private key. The account model contains only VOID-ID, display name, identity public key, and creation time. Request bodies and key material are not logged.

Identity public keys are currently opaque nonblank strings; encoding, algorithm, ownership proof, and cryptographic validation will be defined separately. This milestone does not authenticate account ownership.

Client-side generation/integration, the final VOID-ID format, PostgreSQL and migrations, authentication, login, sessions/JWT, contacts, rate limiting, messaging/WebSocket, device linking, recovery/key backup, E2EE protocols/session establishment, and Docker remain deferred.

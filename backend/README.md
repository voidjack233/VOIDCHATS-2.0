# VOID backend

The initial backend is one Go API process using only the standard library. It provides `GET /health` and a standalone VOID-ID package. It does not create accounts or connect to the React Native frontend yet.

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
  cmd/api/                 process startup, signals, and graceful shutdown
  internal/config/         HTTP address from the environment
  internal/transport/http/ standard-library server and health handler
  internal/voidid/         opaque ID, generation, parsing, and validation
  go.mod
```

The server sets a 5-second header timeout, 10-second read and write timeouts, and a 60-second idle timeout. Startup is logged after the listener binds successfully. Shutdown stops accepting connections and allows in-flight requests up to 10 seconds to finish, then closes remaining connections if necessary. Startup and shutdown failures produce a nonzero exit status.

## VOID-ID

`voidid.New()` returns an opaque, comparable `voidid.ID` and an error. Generation reads 20 bytes from `crypto/rand.Reader`: 160 bits of entropy and a namespace of `2^160` possible IDs. There is no timestamp, counter, user metadata, or insecure randomness fallback. Reader errors are returned to the caller.

The provisional human-readable representation uses uppercase RFC 4648 Base32 without padding, in four groups of eight characters:

```text
VOID-KTQ4SZBR-Q7NA2W5Y-BYGVN3PC-XE64MDUZ
```

Base32 avoids mixed case, and grouping makes a long identifier easier to read and copy. The format is a foundation choice, not a permanent product specification. Encoding and parsing stay inside `internal/voidid`, so future account logic uses the `ID` type without knowing its representation. A later format change still needs a compatibility plan for already-issued public IDs.

`voidid.Parse()` constructs an ID from canonical text; `voidid.Validate()` checks text without exposing the random bytes. Parsing rejects incorrect lengths, separators, case, whitespace, padding, and characters. `ID.String()` formats a valid ID. The zero value is invalid, `ID.IsValid()` distinguishes it, and text serialization rejects it. There is no public raw-byte constructor or accessor.

Random collisions are extraordinarily unlikely but are not impossible. The sample uniqueness test checks for duplicates; it is not a global uniqueness guarantee. When persistence is introduced, account creation must enforce a UNIQUE constraint on VOID-ID and call `New()` again on a collision. No database lookup or collision reservation is implemented now.

## Identity boundaries and deferred work

Display names are mutable cosmetic metadata and need not be unique. VOID-ID is the stable public account identifier, not an authentication secret, encryption key, or chat identity key. The chat identity keypair belongs on the client/device; the backend must never generate, request, transmit, log, or store the identity private key. A future backend may register identity public keys.

Account and public-key models, migrations, persistence, authentication, contacts, messaging transports, and E2EE protocols remain deferred. Those packages and directories are added when they have real implementation, without placeholder files.

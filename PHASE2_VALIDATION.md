# Phase 2 implementation and verification

Verified on 2026-11-01 with Go 1.26.5 on the existing Linux backend workspace.

## Changed code

| Package / files | Change |
| --- | --- |
| `internal/auth/service.go`, `service_test.go` | Stored-key authentication, canonical VOID-IDs, connection-owned expiring challenges, atomic single-use proofs, replay and invalid-key tests |
| `internal/account/key_validation.go` | Small exported wrapper reusing the existing registration key validation; registration implementation unchanged |
| `internal/postgres/authentication.go`, `authentication_test.go` | Parameterized read-only public-key lookup, cancellation, absent-account handling, safe error mapping and mocked query tests |
| `internal/postgres/repository.go`, `repository_test.go` | Startup/readiness require the public-key SELECT access needed for authentication |
| `internal/relay/server.go`, `json.go`, `server_test.go`, `slow_test.go` | Bounded WebSockets, strict envelopes, authenticated sender metadata, online relay, connection replacement, heartbeat, shutdown and adversarial/concurrency tests |
| `cmd/void-server/main.go` | WebSocket endpoint in the existing HTTP server, with explicit upgraded-socket shutdown |
| `internal/config/config.go`, `config_test.go`, `.env.example` | Bounded socket capacity, queue, payload and authentication-timeout settings; existing `.env` unchanged |
| `cmd/void-demo/main.go`, `main_test.go` | Two-account in-memory registration/authentication/relay demonstration and verification of observed network exchanges |
| `tests/relay/http_lifetime_test.go` | Real shared-listener test ensuring WebSockets outlive ordinary HTTP deadlines and registration remains available |
| `tests/integration/postgres_test.go`, `scripts/test-integration.py` | Update future opt-in PostgreSQL tests to include authentication key lookup/read permission; runner not executed |
| `go.mod`, `go.sum` | Pin `github.com/coder/websocket v1.8.15` |

Protocol and operating limits are documented in [WEBSOCKET.md](WEBSOCKET.md).
The [captured local demonstration](docs/phase2-demo.txt) shows both forwarded
envelopes, transport results and disconnect/offline behavior. Every printed
payload is explicitly simulated ciphertext.

## Actual check results

| Check | Result |
| --- | --- |
| `gofmt -l cmd internal tests` | Passed; no unformatted Go files |
| `go test ./...` | Passed; opt-in PostgreSQL write/integration cases skipped |
| `go test -race ./...` | Passed; no race reports |
| `go vet ./...` | Passed |
| `go build -o bin/void-server ./cmd/void-server` | Passed |
| `go build -o bin/void-demo ./cmd/void-demo` | Passed |
| `go run ./cmd/void-demo` | Passed; two registrations, two authenticated sessions, both forwarding directions, offline result and zero final active sessions verified |
| `go test -race ./internal/relay -count=5` | Passed |
| `go test -race -count=5 ./tests/relay` | Passed |
| `git diff --check` | Passed |

Tests exercise valid/invalid authentication; challenge expiry, identity binding,
replay and concurrent consumption; unknown accounts and degenerate stored keys;
unauthorized sends and sender impersonation; malformed/binary/oversized input;
concurrent clients; full queues, failed/stalled writes and transport receipts;
heartbeat eviction; successful reconnection and stale cleanup; admission limits;
and shutdown, including a socket still being upgraded.

A separate temporary Phase 2 process bound to an ephemeral loopback port and read
the existing `.env` and PostgreSQL connection. It passed startup validation,
`/healthz` and `/readyz` returned HTTP 200, an invalid registration probe returned
the existing HTTP 400 JSON response, and a WebSocket request without upgrade
headers returned HTTP 426. That temporary process shut down successfully. These
checks were read-only: no real account was created or modified and no schema,
role, grant, credential, or database configuration was changed.

The PostgreSQL cases requiring a disposable writable database were explicitly
observed to skip. No database-administration opt-in or integration runner was
used, so this report does not claim a fresh PostgreSQL write-integration run.

## Remaining limits

The relay is process-local and online-only. It has no offline queue, persistence,
deduplication or recipient acknowledgment. `forwarded` confirms only a completed
WebSocket write; a failed/missing result can have an ambiguous transport outcome.
Cross-process routing and distributed/per-account message flood controls remain
outside scope.

The public WebSocket path still requires a separately approved extension of the
existing tunnel path rule. The running Phase 1 backend and Cloudflared setup were
left in place; Phase 2 was not deployed or merged. Work is committed locally on
the existing `backend` development branch.

# Durable messaging implementation and deployment record

Verified on 2026-11-02 (Asia/Manila). Source branch:
`codex/durable-messaging`, based on backend commit `4b9bed8`.

## What is implemented

The 15 HTTP endpoints in [MESSAGING_API.md](MESSAGING_API.md), including existing
Ed25519 identity authentication, hashed short-lived bearer sessions, exact
VOID-ID discovery, opaque public KeyPackages and atomic claims, message requests,
acceptance/decline/blocks, accepted conversations, opaque application/commit
messages and ordered durable sync/acknowledgments.

Registration's endpoints, payloads, account schema and identity bindings are
preserved. Production persistence uses the new PostgreSQL tables in the
review-only migrations; neither construction nor startup runs migrations.
Messaging defaults to disabled. Enabled mode removes the older `/ws` route,
which otherwise lacks conversation/block authorization.

## Actual automated checks

| Check | Result |
| --- | --- |
| `go test ./...` with both PostgreSQL integration opt-ins set to `0` | Passed |
| `go test -race ./...` with both opt-ins set to `0` | Passed |
| `go vet ./...` | Passed |
| `go build -o /tmp/void-durable-messaging-server ./cmd/void-server` | Passed; executable not deployed |
| `gofmt -l cmd internal tests` | No unformatted files |
| `git diff --check` | Passed |
| API documentation JSON examples | All 28 parse, with duplicate-field detection |
| Local documentation links | Resolve |
| Live/disposable PostgreSQL integration | Explicitly skipped, not executed |

Synthetic tests exercise the real HTTP/session/domain code with in-memory
stores: two separately generated identities authenticate and discover each
other, claim a public package, create/accept an invitation, exchange opaque
bytes in both directions, order commits and application messages, retry exact
messages, reconstruct services over retained storage, sync, ACK and revoke.
They reject identity forgery, unknown/duplicate/case-aliased fields, malformed
input, oversized bodies, foreign ACKs, conflicting retries and unauthorized
actions. Authenticated rates distinguish accounts sharing one proxy IP.

Domain tests additionally exercise concurrent claims, opposite invitations and
message ordering; package expiry/deduplication/one-use binding; declined requests;
blocks cancelling pending invitations/queued events; rejection receipts; atomic
ACK batches; byte/count quotas and rollback; reserved receipt capacity; blocking
despite event exhaustion; and the 4 MiB encoded sync-page bound. Driver unit
checks cover typed int64/UUID parameters, decrement guards and safe diagnostics.

Service reconstruction over a retained in-memory store proves the service's
persistence contract, not a PostgreSQL server restart. Four new PostgreSQL
integration tests are prepared in `tests/integration/messaging_test.go`; they
require BOTH explicit opt-ins, externally reviewed/provisioned migrations and a
dedicated `void_test_` database. They never create schemas or grant privileges.
The normal verification explicitly observed them skip before any connection.

After the owner provisions an approved disposable database and applies the
reviewed schema externally, the focused command is:

```bash
VOID_RUN_POSTGRES_INTEGRATION=1 \
VOID_RUN_MESSAGING_INTEGRATION=1 \
VOID_TEST_DATABASE_URL="$APPROVED_DISPOSABLE_DATABASE_URL" \
go test -race ./tests/integration -run '^TestPostgresMessaging' -count=1 -v
```

This command was **not** run. Keep the disposable URL separate from the existing
development/production connection.

## Actual Cloudflare change

Applied the user-requested one-line prefix expansion to the existing
`void0000-main` tunnel. Reviewed portable artifacts:
[patch](../deploy/cloudflared-ingress.patch) and
[rule fragment](../deploy/cloudflared-ingress.yml).

Installed configuration: `/home/void0000/.cloudflared/config.yml`.
Exact original backup:
`/home/void0000/.cloudflared/config.yml.before-durable-messaging-20261101T212057Z`
(the backup filename uses UTC).

The original process was gracefully stopped and the same tunnel command was
restarted. The new process registered a Cloudflare connection, and its local
metrics readiness endpoint returned HTTP 200. Configuration validation passed;
the ingress matcher selects rule 17 for `/auth/challenges` under the API prefix.
A byte comparison verified that only the reviewed path expression changed;
the other 31 ingress rules, hostname, backend address, DNS and credentials were
preserved. No autostart service was installed.

## Routes actually available

Read-only GET probes used `User-Agent: VOIDCHATS/2.0`; they created no accounts,
sessions, challenges or messages.

| Probe | Local backend | Public hostname |
| --- | --- | --- |
| `GET /healthz` | 200 | Not probed; this backend's health remains local |
| `GET /dm9pZG5vdGhpbmduZXNz/accounts` | 405 JSON `method_not_allowed` | 405 JSON `method_not_allowed` |
| `GET /dm9pZG5vdGhpbmduZXNz/auth/challenges` | 404 JSON `not_found` | 404 JSON `not_found` |
| `GET /dm9pZG5vdGhpbmduZXNz/people/1` | Not probed | 404 JSON `not_found` |

The tunnel forwards the expanded namespace, but the running Go process was left
unchanged on the earlier registration build. **The new HTTP messaging APIs are
not deployed.** No migration, database administration, account write, credential
change or real `.env` edit was performed. Database migration review and backend
activation are the next deployment steps in
[MESSAGING_DEPLOYMENT.md](MESSAGING_DEPLOYMENT.md).

## Limits of this verification

Actual PostgreSQL migration execution, transaction behavior under a real server
restart and the two-device phone/LDPlayer flow remain unverified. The server
does not implement MLS cryptography: the native client must validate account
keys/KeyPackages/Welcome membership, manage MLS commits, encrypt/decrypt and
durably persist its inbox/outbox and checkpoint before ACK. Synthetic opaque
bytes are not a claim that real MLS encryption was tested.

Storage and rate limits do not fix the separate bot-account issuance gap.
Persistent aggregate signup controls and an operational retention policy remain
necessary before unrestricted traffic. See the API's explicit backpressure and
retention limits; no unacknowledged event is silently evicted to make space.

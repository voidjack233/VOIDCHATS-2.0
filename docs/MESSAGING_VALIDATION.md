# Durable messaging verification and live deployment

Verified on 2026-11-02 (Asia/Manila). Branch: `codex/durable-messaging`.
**Deployed backend source: `bad8a7216c5276d1cfd71f22a188fdcda69cc241`.**

The owner approved isolated PostgreSQL verification, the two reviewed additive
migrations and activation only after tests passed. The backend is now running
with durable messaging enabled; the Cloudflare API prefix is active.

## Verification actually performed

| Check | Result |
| --- | --- |
| Full `go test ./...` against private PostgreSQL 18 with both integration opt-ins | Passed; all nine PostgreSQL integration tests executed, none skipped |
| `go test -race ./tests/integration -count=1` against private PostgreSQL | Passed; all nine cases executed |
| `go test -race ./...` against private PostgreSQL | Passed |
| Opt-in real HTTP/backend plus PostgreSQL process restart test with race detector | Passed |
| `go vet ./...` | Passed |
| Build deployed server | Passed; Git build metadata reports source `bad8a72`, `vcs.modified=false` |
| Formatting and `git diff --check` | Passed |
| API documentation examples | All 28 JSON examples parse, with duplicate-field detection |
| Full database backup restore into the isolated cluster | Passed; account data matches exactly and account schema is equivalent |
| Live migration/account-preservation verification | Passed |
| Local and public routing/authentication probes | All 38 expected statuses matched |

The test cluster ran PostgreSQL 18 on a Unix socket inside an owned private
`/tmp/void-messaging-pg-*` directory, with TCP disabled. It used a separately
created disposable `void_test_` database and restricted synthetic application
role. No existing database or real environment file was used for automated
account/message tests.

The first real PostgreSQL run found an ambiguous unary-minus parameter in the
receipt-reservation decrement query. PostgreSQL could not infer the type of
`-$2`. Explicit integer casts fixed it; the subsequent full and race runs
passed. The database tests cover this path through real ACK and block operations,
in addition to the mocked driver tests.

## Real HTTP and process restart coverage

`tests/blackbox/messaging_restart_test.go` launched the actual server executable
against the isolated database, with `-env-file ''`. Two fresh Ed25519 identities
registered through the existing HTTP endpoints, authenticated through the new
challenge/session flow and looked each other up with exact public-key bindings.
The test then verified:

- Public KeyPackage publication, atomic claims and identical claim retries.
- Recipient-visible invitations, decline, acceptance and the same conversation
  visible to both participants.
- Opaque application objects in both directions, shared conversation sequences
  and commit ordering.
- Deliberately losing a successful send response, then retrying the exact bytes
  and receiving the original message ID/sequence; changed bytes return 409.
- Applied/rejected ACK outcomes and corresponding delivered/rejected receipts.
- Enqueueing while the recipient performs no requests, then stopping and
  restarting both the actual backend and PostgreSQL processes.
- Existing bearer sessions, exact offline ciphertext, stable retries, ACK state
  and duplicate suppression after that full restart.
- Durable logout and blocks preventing discovery/new delivery.

The domain and database suites additionally cover concurrent claims/opposite
invitations/sends, foreign-resource access, one-use package binding, pending
request cancellation, blocking queued delivery, atomic ACK batches, receipt
reservations, byte/count backpressure and encoded sync-page size.

The bytes in these tests are explicitly synthetic opaque objects. They are not
an MLS cryptographic implementation or a claim of successful phone/LDPlayer
native encryption.

## Reproduce the isolated checks

Run the cluster provisioner only with owner authorization for disposable tests:

```bash
python3 scripts/test-messaging-isolated.py --keep-running
```

It applies the exact migrations and restricted grants, runs the full suite and
race-enabled PostgreSQL integration suite, and prints a private state path.
Using that path, run:

```bash
VOID_RUN_MESSAGING_BLACKBOX=1 \
VOID_MESSAGING_BLACKBOX_STATE=/tmp/void-messaging-pg-REPLACE/state.json \
go test -race ./tests/blackbox -run '^TestMessagingHTTPPostgresProcessRestart$' -count=1 -v
```

After verification, stop and remove only that owned cluster:

```bash
python3 scripts/test-messaging-isolated.py --stop /tmp/void-messaging-pg-REPLACE/state.json
```

The blackbox test validates the private marker, path ownership, disposable
name, actual connected data directory and private postmaster before controlling
PostgreSQL. Ordinary test runs leave this process-restart case disabled.

## Backup and exact live database changes

Private backup directory:
`/home/void0000/.local/state/void-backend/deployments/20261102T084024+0800`.
It contains the preserved pre-deployment executable, full custom-format backup,
account/catalog snapshots, migration hashes and verification results.

`database.before.dump` was restored in a separate isolated database before any
live migration. All account data matched exactly. PostgreSQL's restore parser
flattened associative AND parentheses in the existing username CHECK; that
rendering difference was inspected, and every other account catalog line
matched. The live migration itself did not rewrite that CHECK: the exact live
account schema digest stayed unchanged before and after application.

Applied only these reviewed files to `void2-0e2ee`, sequentially with
`psql -X --no-password -v ON_ERROR_STOP=1 -f <file>`:

| File | SHA-256 |
| --- | --- |
| `migrations/001_messaging.sql` | `efba72e59e4c70e8af9b451ea2c4c12dac01db3ca319be7be590aefde96c644f` |
| `migrations/002_sessions.sql` | `ef7099f18d0f431ec60d466be76e802b65a08c3482f88eabadd17e274ef4cb10` |

Nine messaging/session tables were added. Their column/default/constraint/index
catalog digest matches the isolated tested catalog. The existing `postgres`
login already has the required access, so no privilege script, role, password,
grant or credential change was needed.

Both existing accounts and their original keys were preserved. Before and after
application, the row count was 2 and these digests matched exactly:

- Account data: `c9efbddd8902439fe34031a68ae7b01642797531668e41bcef1ce2b5dc98225b`.
- Account columns/defaults/checks/indexes: `245f4bd2d861415649264a7abe347202b4df05b734a043eaa9f9b6d0551d0e29`.

The real `.env` file's content digest and permissions were unchanged. Additive
foreign-key references introduce expected internal dependencies on accounts;
they do not replace the existing columns or key bindings. Automated verification
created no account, session, package, request or message in the live database.

## Routes actually deployed

The `void0000-main` tunnel forwards:

```text
https://api.voidchats.xyz/dm9pZG5vdGhpbmduZXNz
```

Its reviewed one-line path expansion is recorded in
[cloudflared-ingress.patch](../deploy/cloudflared-ingress.patch). All 31 unrelated
ingress rules and the hostname, backend address, DNS and tunnel credentials were
preserved. The original tunnel configuration backup remains at
`/home/void0000/.cloudflared/config.yml.before-durable-messaging-20261101T212057Z`.

Live source `bad8a72` was installed as `./void-server` and started using
`scripts/run-messaging-server.sh`. Startup reported `durable_messaging=true`.
The installed executable SHA-256 is
`eb1f066d061df264ec288112b34c4be2f6f9b7b0036cc985d6e06cd1910a07a5`.

Local health/readiness returned 200. All 15 messaging endpoints were probed
locally and publicly. Protected routes without a bearer returned JSON 401,
while auth handshake probes used an unknown account/unknown challenge and
returned JSON 401 without creating state. Existing registration rejected an
empty body with 400, and a wrong GET method with 405. Legacy `/ws` returned 404,
preventing the earlier relay from bypassing acceptance or blocks. All 38 checks
matched; no live account or messaging data was inserted for verification.

Use the launcher for routine restarts. The real `.env` was not edited; durable
mode is selected through process variables. No autostart service was installed.
Rollback uses the preserved earlier executable while retaining the new tables
and queued data. Do not drop tables or restore the full backup over new messages
as a routine application rollback.

## Remaining client work

The native phone/LDPlayer integration and real MLS encryption/decryption remain
to be demonstrated by the app. Clients must validate discovered identity keys,
KeyPackages, Welcome membership and pinned peer credentials, resolve MLS commit
conflicts, and persist native checkpoints/inbox/outbox state before ACK. The
backend verifies durable opaque transport and authorization, not plaintext or
MLS epochs. See the [API contract](MESSAGING_API.md) for exact examples.

The separate bot-account issuance/storage gap is unchanged by this deployment;
the API documents its bounded queues, backpressure and retention limits.

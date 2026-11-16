# Permanent account deletion: Rust integration and deployment

This is the contract accompanying the Rust deletion UI in `a91049f`. No Rust
source changed. This backend is prepared for review, not deployed. Migration 007
has not been applied to production.

## Transport and state

Use the existing TLS WebSocket endpoint:
`wss://api.voidchats.xyz/dm9pZG5vdGhpbmduZXNz/ws`.
The canonical signed audience is
`https://api.voidchats.xyz/dm9pZG5vdGhpbmduZXNz` (without `/ws` or a trailing slash),
or the exact configured audience for another installation.

Frames are strict JSON objects with string values. Unknown/duplicate fields,
nulls, non-string values, malformed UTF-8, and binary frames are rejected.
VOID-IDs are canonical positive decimal int64 strings, never JSON numbers.
Messages/signatures use canonical standard padded base64. Timestamps use UTC
RFC3339Nano. JSON field order does not matter; signed transcript bytes do.

Persisted lifecycle states are `active` and `deleted`. An absent account reports
`unknown`. Deleted is permanent. Neither its VOID-ID nor its authentication key
can be registered again. There is no reactivation operation.

## Explicit permanent deletion

Require the exact `DELETE` user confirmation before signing. Normal relay
authentication, disconnects, cancellation, timeouts, and lost heartbeats never
authorize deletion.

Send this on the owner's authenticated socket, or as the **first frame of a new
socket**. Prefer a dedicated socket. The new-socket form authenticates only for
deletion, including recovery of an already deleted account; it grants no relay
session.

```json
{"type":"account_delete_begin","void_id":"123"}
```

Server:

```json
{"type":"account_delete_challenge","challenge_id":"<43-character base64url token>","message":"<padded base64>","expires_at":"2026-11-16T00:00:30Z"}
```

The challenge is valid for 30 seconds and belongs to this socket. The overall
exchange has a 35-second deadline. While awaiting proof, this socket accepts only
the confirmation frame. On an authenticated socket, new deliveries are suspended;
already received frames may precede the challenge. Do not pipeline commands.

The decoded UTF-8 message consists of these eight newline-terminated lines:

```text
VOID:delete-account:v1
<challenge_id>
<canonical VOID-ID>
<standard padded base64 of the stored 32-byte Ed25519 authentication public key>
<expires_at, exactly as returned>
<standard padded base64 of the canonical audience UTF-8 bytes>
DELETE
<43-character base64url random nonce>
```

The final newline is signed. The challenge ID and independent nonce each contain
256 random bits. Validate the domain, ID, local authentication public key,
audience, confirmation marker, and expiry before signing. Sign the **decoded
message bytes verbatim** with the existing account Ed25519 private key. Never
send the private key, reserialize the transcript, or sign its base64 text.

Client:

```json
{"type":"account_delete_confirm","challenge_id":"<same token>","confirmation":"DELETE","signature":"<padded base64 of the 64-byte signature>"}
```

The server checks the exact confirmation string in addition to its inclusion in
the signed transcript. The challenge is consumed before verification/database
access. Wrong keys, expired/invalid proofs, cross-socket proofs, and replay cannot
authorize deletion. A normal authentication proof cannot authorize this operation.
Disconnecting before confirmation abandons the challenge.

Only after the database transaction commits, server:

```json
{"type":"account_deleted","void_id":"123","deleted_at":"2026-11-16T00:00:12.345678Z"}
```

This is the unambiguous success acknowledgment. Fresh authorized retries of an
already completed deletion return the identical `deleted_at`. Persist confirmed
own-account deletion before clearing credentials. The socket then closes without
a guaranteed WebSocket close handshake.

All deletion errors have the shape:

```json
{"type":"error","code":"deletion_authorization_failed"}
```

| Code | Meaning |
| --- | --- |
| `deletion_authorization_failed` | Unknown ID, wrong key/signature/challenge, replay, or incorrect confirmation. |
| `unauthorized` | Authenticated socket attempted another account's deletion. |
| `invalid_frame` | Malformed ID or malformed/unexpected frame. |
| `rate_limited` | Account command budget exhausted. |
| `overloaded` | Concurrent-operation capacity exhausted. |
| `unavailable` | Backend/database failure or uncertain transaction outcome. |

Every deletion exchange ends its socket, on success or error. Expiry or transport
failure may close without an error frame. Missing acknowledgment or `unavailable`
does **not** establish that deletion failed.

### Interrupted requests and uncertain acknowledgments

After confirmation is sent, retain the VOID-ID/authentication private key until
success is persisted. On a new socket, send `account_delete_begin` and sign a
**fresh** challenge. Do not replay the old proof or require ordinary relay
authentication first: Deleted accounts cannot authenticate for relay use.

The fresh deletion-only proof verifies against the retained tombstone public key.
If deletion committed, it returns the original acknowledgment; otherwise it
completes deletion. This expresses continuing intent to delete. Persist pending
deletion intent across app crashes and reconcile through this flow after restart.
Do not register a replacement account to resolve uncertain deletion.

If the key was already erased, another Active account can query peer status,
but cannot obtain an owner-authorized deletion acknowledgment.

## Authenticated read-only peer status

Complete the existing audience-bound `auth_begin` / `auth_proof` exchange and
receive `auth_ok`. Then:

```json
{"type":"account_status","request_id":"status:001","void_id":"456"}
```

`request_id` is 1–64 ASCII letters, digits, `_`, `-`, `.`, or `:`. Exactly one
successful response is returned:

```json
{"type":"account_status","request_id":"status:001","void_id":"456","state":"active"}
{"type":"account_status","request_id":"status:001","void_id":"456","state":"deleted","deleted_at":"2026-11-16T00:00:12.345678Z"}
{"type":"account_status","request_id":"status:001","void_id":"456","state":"unknown"}
```

Only Deleted includes `deleted_at`. No identity or prekeys are returned. Normal
command/concurrency limits apply. Errors use `{"type":"error","code":"..."}`:
`invalid_frame` (malformed request, closes socket), `rate_limited`, `overloaded`,
`unavailable`, `not_found` (invalidated requester), or `account_deleted` (Deleted
requester). Existing generic error frames have no request ID: serialize requests
that depend on them and distinguish unsolicited `event` frames.

Active/Unknown are snapshots. Refresh relevant peers by VOID-ID after reconnect
and before re-enabling uncertain peers; there is no bulk status or deletion push
stream. Persist only authoritative Deleted status or an operation's
`account_deleted` error as Deleted. Never infer it from network failure,
authentication failure, Unknown, missing prekeys, expiry, or `not_found`.
Persisted Deleted is terminal and must not be overwritten by a stale Active
response. Backend enforcement does not depend on this client cache.

## Sends, lookup, prekeys, and sessions

Existing `send` and `lookup` request fields remain unchanged. A Deleted peer
returns exactly:

```json
{"type":"send","event_id":"00000000-0000-4000-8000-000000000001","to_void_id":"456","payload_base64":"<ciphertext base64>"}
{"type":"lookup","void_id":"456"}
```

Response to either request:

```json
{"type":"error","code":"account_deleted"}
```

The same applies to `relay_online`. Unknown peers return `not_found`. Deleted
lookup never returns a `person` identity. Preserve chat history and drafts,
disable Send/Resend, and display "This user account has been deleted."

Prekey claims for a Deleted recipient, including retries of previously successful
claim UUIDs, return:

```json
{"type":"prekeys_claim","request_id":"00000000-0000-4000-8000-000000000002","void_id":"456"}
```

Response:

```json
{"type":"prekeys_error","request_id":"<claim UUID>","code":"account_deleted"}
```

A publication racing deletion may return the same shape with its publication
UUID in `request_id`. A command from an invalidated session can instead receive
generic `error/account_deleted` or a closed socket. Deleted accounts cannot
publish or claim prekeys, send, ACK, lookup, query status, or relay online.

The initiator receives its acknowledgment and closes. A different local relay
session for that account is forcibly closed after commit. Other backend processes
enforce durable state on every operation and poll idle authenticated connections
in the one-second delivery loop when operation capacity is available. A socket
may briefly remain connected but cannot perform operations after deletion.

Ordinary `auth_begin` for a Deleted ID returns the existing generic
`authentication_failed`. A proof begun before deletion is rechecked before
`auth_ok`; if deletion wins, it returns `account_deleted` and closes. Do not infer
Deleted solely from `authentication_failed`; use owner deletion recovery or an
authenticated peer-status check after reconnect.

Writes of identity, prekeys, and ciphertext recheck both accounts under locks
through the bounded socket write. If deletion wins the response race, an
operation can receive `account_deleted` and close its socket. Refresh peer status
after reconnect. Bytes written before commit may arrive later through network
buffering and cannot be recalled. A send committed/acknowledged before deletion
can still have its undelivered queue row removed during deletion. `accepted`
never promises recipient delivery or permanent server history.

## Audit and guarantees

| Entry point | Enforcement |
| --- | --- |
| Registration: `account.Service`, `Repository.Insert` | Existing unique ID/key constraints include tombstones. No client-selected ID or reactivation path. |
| Authentication: `AuthenticationKey`, relay activation | Key query excludes Deleted; activation locks and rechecks state through `auth_ok`. |
| Every authenticated relay command | Requester Active check before dispatch. |
| Message send: `QueueStore.Enqueue` | Locks both accounts numerically, reads state, then applies quotas/inserts/commits. |
| Pending and ACK | Recipient checked inside transaction; delivery writes recheck both peers. |
| Signal publication and claims | Same account locks before inventory/cached-claim access, retaining existing allocator/quota locks. |
| Lookup | Deleted returns no key; outgoing identity rechecks both accounts. |
| Online relay | Both accounts locked through bounded forwarding/result write. |
| Account status | Requester checked transactionally; target read without identity fields. |
| Expiry cleanup | Removes expired ciphertext or scrubs claims; cannot reactivate accounts. |

Actual tables are `accounts`, `messages`, `signal_prekey_inventory`, and
`signal_prekey_claims`. Migration 007 adds one nullable column,
`accounts.deleted_at`, one trigger function, and two triggers. NULL means Active.
The first deletion transition assigns database `clock_timestamp()` after locking
the account. Triggers reject identity changes, modification/removal of a deletion
timestamp, account DELETE, and account TRUNCATE. Existing unique constraints
reserve the ID/key forever.

Operations share the existing injective transaction advisory-lock key
`(int32(id >> 32), int32(id))`, sorted numerically. Deletion atomically marks the
account, removes incoming **and outgoing** queued ciphertext, deletes its public
prekey inventory and requester claims, and scrubs recipient/bundle data in other
requesters' claims. Those requesters retain their existing UUID/deadline retry
markers so a used UUID cannot allocate fresh keys for another peer. Previously
expired claims already lack recipient relationships/bundles and remain minimal
retry markers.

Cleanup failure rolls back every change, including the deletion timestamp.
Crashes/disconnects leave either the committed deletion or the previous Active
state. Fresh proof resolves uncertain acknowledgments. No post-deletion send or
prekey mutation can commit. Successful concurrent duplicates return one original
timestamp. Remote device history is untouched; this backend stores no delivered
conversation history.

The retained account row contains its existing UUID, VOID-ID, authentication
**public** key, creation time, and deletion time. The key verifies recovery and
prevents key reuse; it is never returned by Deleted lookup or normal
authentication. No private keys or deletion proofs are persisted/logged.

## Verification and safe deployment

The full suite passed with `-race -count=1` against synthetic PostgreSQL 18
`void_test_` fixtures. The configured connection created/changed no roles or
credentials; the disposable database was removed. The restricted-grant test is
explicitly skipped in this mode. Other database checks run, including readiness,
rollback, concurrent deletion/send/claim/publication, cleanup, uniqueness, and
immutable tombstones. Unit/WebSocket checks cover wrong keys, transcript
domain/audience/ID tampering, expiry, concurrent/copied/cross-socket replay, exact
confirmation, owner restrictions, disconnects, revocation, and lost ACK recovery.
Recreated services/new database connections check restart/reconnect durability;
no production process or database restart is performed.

Reproduce with explicit disposable-database authorization:

```bash
python3 scripts/test-integration.py --allow-database-admin --existing-login --admin-env .env -- go test ./... -race -count=1
go test ./...
go vet ./...
go build -o /tmp/void-account-deletion-review ./cmd/void-server
git diff --check
```

Deployment requires separate approval:

1. Review migration 007 and this change, retain the owner's normal backup, and
   rehearse in disposable fixtures. Coordinate the Rust protocol implementation.
2. Stop/drain **all old backend processes** before applying 007. Old binaries
   ignore Deleted; mixed-version traffic is unsafe. Keep ingress stopped until
   the matching binary validates successfully.
3. With explicit production migration approval, apply
   `migrations/007_account_deletion.sql` using the existing owner-managed
   connection. Do not change `.env`, credentials, roles, cluster, tunnel, or
   listener. If additional runtime grants are required, review/apply
   `migrations/007_account_deletion_privileges.sql`. Its prekey grants assume 006;
   base installations without the extension omit those grants.
4. Install/start the reviewed binary only after migration commit. Startup and
   readiness require the new column, immutable-state triggers, and cleanup
   access; they never apply migrations. Check local health/readiness/logs. Do
   not delete a real account for smoke testing.
5. Enable Rust deletion only when its protocol handling is ready. Rollback must
   use a compatible binary that enforces tombstones; fix forward and retain the
   migration. Restoring an older backup requires replaying every committed
   tombstone before admitting traffic.

Deletion is logical removal, not secure erasure of PostgreSQL MVCC pages, WAL,
backups, remote device history, or previously released prekeys/ciphertext.
Bounded socket writes hold an account transaction/connection and can delay
deletion until their deadline. Administrative owners can disable triggers or
rewrite backups, so operational procedures must preserve tombstones.

For unchanged authentication and prekey bundle formats, see
[WEBSOCKET.md](../WEBSOCKET.md), [MESSAGING_API.md](MESSAGING_API.md), and
[SIGNAL_PREKEYS.md](SIGNAL_PREKEYS.md). This document supersedes their account
lifecycle behavior where applicable.

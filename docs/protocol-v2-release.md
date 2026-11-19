# Coordinated application protocol V2 release

Status: **prepared, not activated**. The normal build still advertises V1 and
blocks outgoing Reply, Reactions and Delete for Everyone. Delete for Me remains
local. No production endpoint, account, database or deployment was used in this task.

## Build policy and the missing deployment signal

`cargo build` uses no default features and advertises `"1"` after authentication.
The opt-in Cargo feature `coordinated-v2-release` selects `"2"` and authorizes
remote feature operations for that session only after successful negotiation.
There is no environment variable or runtime UI switch that activates this policy.
`--all-features` also selects it: never distribute an all-features artifact as the
compatibility V1 build. The feature is an explicit **operator coordination choice**,
not evidence that a backend has been configured correctly.

Each reconnect authenticates and negotiates again. The transport also rejects a
`send_versioned` request whose minimum exceeds that session's advertised version.
Ordinary text and encrypted delivery receipts continue using their existing V1
`send`/`event` shapes, stable UUIDs, Signal format, SQLCipher transactions and ACKs.
Only feature payloads use `send_versioned` with minimum `"2"`. Receiver behavior,
deferred recovery, account identity and encryption formats are unchanged.

**The current backend contract cannot automatically attest deployment readiness.**
Reviewed backend commit: `9eb131933a3c9e95cc8b5dba1bebae6400691419`:

- [Messaging contract](https://github.com/voidjack233/VOIDCHATS-2.0/blob/9eb131933a3c9e95cc8b5dba1bebae6400691419/docs/MESSAGING_API.md)
- [Minimum enforcement and activation](https://github.com/voidjack233/VOIDCHATS-2.0/blob/9eb131933a3c9e95cc8b5dba1bebae6400691419/docs/VERSIONED_DELIVERY.md#mandatory-minimum-protocol-prepared-not-deployed)

Both a minimum-V1 relay and a minimum-V2 relay answer a V2 advertisement with
exactly `{"type":"protocol_ok","version":"2"}`. That frame confirms this
connection's receive version, not the server-wide minimum or another peer's build.
`min_protocol_version` is exposed only when rejecting an insufficient client via
`update_required`; it is absent from successful negotiation. There is no read-only
minimum/capability discovery command. The minimum is fixed at backend startup.

Do not infer enforcement from `protocol_ok`, feature-send acceptance, health/readiness,
prekey availability or a successful V2 message. Do not probe by deliberately
downgrading a live account's socket: decreasing a version is invalid, and a
minimum failure is terminal for the client worker. One successful probe behind a
load balancer also cannot prove every backend instance is enforcing the same minimum.
If automatic client-enforced deployment attestation is required, a separately
reviewed backend contract providing positive authenticated deployment policy is
still necessary. This change does not invent such a field or a substitute handshake.

## Exact activation order — requires separate release authorization

1. **Prepare artifacts and recovery first.** Review the Rust revision, run the
   checks below, and prepare an identified, integrity-verified V2 artifact for
   private staging. Keep an available update path for users who receive Update
   Required. Keep the send-enabled artifact private until enforcement is verified;
   it must not auto-launch against the current minimum-V1 service. Preserve existing operational backups; no identity reset,
   database downgrade or account re-registration is an update procedure.
2. **Stop all permissive relay instances and close their sockets.** During an
   approved maintenance window, drain/stop every old relay process, including
   instances behind proxies and existing authenticated connections. Do not run
   a mixed pool or leave an old listener reachable. Existing sockets cannot
   acquire a new startup minimum through live reload.
3. **Set `VOID_MIN_PROTOCOL_VERSION=2` in every relay's deployment environment.**
   Start only the reviewed minimum-enforcing backend (`9eb1319` or a verified
   descendant). Process environment takes precedence over the backend env file;
   verify the effective process setting, not just an edited file. This is an
   operator deployment step, not a Rust command and not performed here. Migration
   008 is already a prerequisite of this protocol;
   do not replay it or add another migration merely to set the minimum.
4. **Verify enforcement before enabling V2 senders.** Check each actual instance
   and its effective startup configuration. With separately authorized designated
   canary identities, verify that V1 negotiation receives
   `update_required` with minimum `"2"` and disconnects; an authenticated client
   cannot use messaging/prekey commands before negotiation; a V2 client receives
   matching `protocol_ok`, can exchange ordinary messages/receipts, and recovers
   queued V2 events. Confirm there are no bypass listeners or permissive replicas.
   Use isolated canary data, never a user's personal history for release probes.
5. **Then distribute/launch the coordinated V2 client.** The release build command,
   to be used only by that separately approved release procedure, is:

   ```powershell
   cargo build --locked --release --features coordinated-v2-release
   ```

   Bind the artifact identity/checksum to the verified backend rollout. Ensure
   the normal V1 binary is not accidentally published under the V2 artifact name.
   Once the minimum is raised, old clients show Update Required and must update;
   they must not be allowed to continue messaging by lowering the minimum again.
6. **Validate the coordinated release and monitor.** With authorized test accounts,
   check ordinary messaging, verified receipts, replies, reaction changes, both
   deletion scopes, offline queues, reconnect negotiation and interrupted-control
   recovery. Relay acceptance remains distinct from recipient application. Keep
   the existing 30-day queue retention and immutable identity bindings.

Operators may privately stage the prepared artifact before maintenance. Publish
the user download only after enforcement is verified; the send-enabled artifact
must not be launched against minimum V1. No production V2 traffic or
backend configuration change is authorized by this preparation commit.

## Rollback limits

Before any V2 ciphertext is submitted or processed, an operator can evaluate an
aborted rollout using the existing recovery procedure. Do not infer absence of
V2 traffic merely from an empty relay queue: ACKed events can already be stored
on devices and Signal ratchets can have advanced.

After V2 traffic exists, keep minimum V2 and use a V2-capable corrective build.
Reverting to the default V1 artifact at minimum V2 only produces Update Required;
it is not a functional messaging rollback. A V1 recipient cannot drain a V2 queue
head, and later ordinary ciphertext is also held behind it until compatible
processing or normal expiration. Lowering the minimum re-admits incompatible
clients. Rolling back to a backend that ignores version metadata can deliver
unsupported ciphertext. Neither is a safe generic rollback.

Do not relabel queued V2 events as V1, bypass heads, purge ciphertext, edit local
history/Signal sessions or regenerate keys to make a rollback appear successful.
Keep SQLCipher schema compatibility; earlier clients can reject newer schemas,
and no destructive schema downgrade is supplied. Account deletion remains its
separate authenticated protocol. If sending must be suspended, a reviewed fix
can disable new feature sends while retaining V2 receive/recovery support and
minimum enforcement. That corrective artifact is not introduced here.

Mandatory minimum enforcement trusts the client's advertisement. It prevents
compliant incompatible clients from messaging; it is not attestation of an
installed binary and does not prevent a malicious client from falsely claiming V2.

## Verification and artifact safety

Run ordinary checks and the explicitly coordinated profile separately:

```powershell
cargo check --locked
cargo build --locked
cargo test --locked
cargo check --locked --features coordinated-v2-release
cargo test --locked --features coordinated-v2-release
```

The feature-profile `check` validates non-test application code without producing
a production send-enabled executable; feature-profile tests run only isolated
loopback instances. This task does not build or launch the release command above.
Legacy V1 test fixtures remain explicitly V1 even in the coordinated test build;
release-policy tests use the exact selected production policy with only endpoint
and synthetic authentication identity injection. The default executable is built
without the feature.

Mock tests cover negotiation on every reconnect, V1 transport rejection of V2
requests, legacy sends on either build, higher minimums (V1→V2 and V2→V3), terminal
update handling with no automatic reconnect, and the identical `protocol_ok`
responses under minimums 1 and 2. The Slint update fixture verifies the blocking
state and unchanged account/storage data when a newer version is required.

Opt-in tests use the unmodified hash-verified Go `9eb1319`, fresh disposable
PostgreSQL state, and synthetic accounts 101/202 only. Minimum profiles 1, 2 and 3
verify real backend enforcement. At minimum 2 the same coordinated Rust build
drives both frontends through ordinary messaging, verified receipts, replies,
reaction toggles, Delete for Me, Delete for Everyone, reconnects, offline reply
delivery, saved drafts, and recovery of a control interrupted before dispatch.
Existing tests exercise rollback, duplicate/lost acceptance, monotonic revisions
and committed-ciphertext retry. No personal account directories or production
credentials are read. Disposable processes and database directories are removed
after verification.

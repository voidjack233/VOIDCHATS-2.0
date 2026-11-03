# Signal public-prekey extension

Status: implemented and verified in Go source; migration 006 remains unapplied.
The installed backend, existing PostgreSQL database, frontend, credentials and
network settings have not been changed by this implementation.

The contract matches frontend `main` commit `af467490c522e3e0209701a5d6e4311f3a0c1953`.
Signal is used for 1:1 messaging; OpenMLS is dormant for future groups. The Go
backend distributes public setup material and queues opaque ciphertext. It does
not run libsignal, generate private keys, decrypt messages or store ratchets.

## Proposed database changes

[006_signal_public_prekeys.sql](../migrations/006_signal_public_prekeys.sql) adds
only `signal_prekey_inventory`, `signal_prekey_claims` and one partial expiry
index. Existing `accounts` and `messages`, their rows and constraints are untouched.
The migration is one transaction with 5-second lock and 1-minute statement limits.
It is never applied by startup. Historical migrations 001–005 are unchanged.

Inventory is one row per existing VOID-ID, with device 1, registration ID,
33-byte Signal identity, 64-byte account-binding signature, signed EC prekey,
Kyber fallback, available one-time arrays, consumed-ID watermarks, publication
UUID/hash history and timestamps. These are all public objects or distribution
metadata. No caller-selected account owner is accepted.

Each claim has an authenticated requester/UUID primary key, recipient,
public bundle, device and server timestamps. Go assigns `claimed_at` and an
expiry exactly 24 hours later. An exact retry returns the same bundle until
expiry, even after a new Go service instance is created. Reusing its UUID for
another recipient is a conflict. Expired UUIDs return `prekey_claim_expired`
and never allocate new keys.

Cleanup clears both `public_bundle` and `recipient_void_id` after expiry.
Requester, UUID, device, claimed-at and expiry timestamps remain as a bounded
terminal marker. This is explicitly a lifetime quota: cleanup does not refund it.
A partial expiry index supports bounded cleanup on startup and each minute.
Outages or lock contention can delay physical cleanup, but expired bundles are
not returned. MVCC/WAL/backups have separate physical retention behavior.

Proposed fixed limits:

| Resource | Limit |
| --- | --- |
| Claim retry/use window | 24 hours, assigned by Go |
| Claims per requester | 1,024 over its lifetime, including expired markers |
| Claims globally | 100,000 over the database lifetime |
| Published account inventories | 10,000 globally |
| Publication UUID/hash records | 64 over each account's lifetime |
| Available one-time keys | 64 EC and 64 Kyber per account |
| Decoded publication | 256 KiB |
| Selected bundle | 32 KiB maximum; validated key limits keep it below this |
| EC public key | 33 bytes, Signal type prefix 05 |
| Kyber public key | 1–2,048 bytes; native Kyber1024 serialization fits |
| Signature | 64 bytes |
| Prekey command rate | 8/minute, burst 4, retained across reconnects |

Hard quotas are checked in locked transactions. Separate advisory locks coordinate
only prekey publication/claim allocations; ordinary message transactions use
their existing account-scoped locks. Global prekey serialization is a deliberate
first-milestone throughput limitation. Unlimited new-account registration remains
a separate abuse issue; these quotas bound the prekey tables, not account growth.

[The optional privilege file](../migrations/006_signal_public_prekeys_privileges.sql)
grants an existing restricted role SELECT/INSERT on the two new tables and UPDATE
only on fields used for publication, consumption and expiry scrubbing. No DELETE,
account UPDATE, role or password change is required. An existing owner already
has access. Approval of a role change is separate from applying table DDL.

## Authenticated commands

Use the existing WebSocket authentication. Every outer field remains a string;
`payload_base64` contains canonical padded Base64 of bounded public JSON.
The following are templates; UUIDs, keys and payloads must come from the device.

```json
{"type":"prekeys_publish","publication_id":"<UUID>","payload_base64":"<public upload>"}
{"type":"prekeys_published","publication_id":"<same UUID>","ec_remaining":"20","kyber_remaining":"20"}
```

A decoded upload contains exactly `v`, `device_id`, `registration_id`,
`identity_key`, `account_signature`, `signed_prekey`, `kyber_last_resort`,
`one_time_prekeys` and `one_time_kyber_prekeys`. Version/device must be 1;
registration ID must be 1–16380. Key IDs must be 1–16777214. An EC one-time key
has only `key_id` and `public_key`; signed EC and Kyber keys also have `signature`.
Unknown, private, duplicate, alternate-case, null and noninteger fields are rejected.

The same publication UUID requires the exact same decoded upload bytes, including
JSON formatting. SHA-256 history preserves that rule for older publications.
Retries report current counts and do not restore consumed keys. New batches must
use IDs above stored watermarks. A new publication with empty append arrays can
report counts. Account/Signal identity, registration, signed EC and fallback keys
are immutable in this milestone; rotation and recovery are not implemented.

Go verifies the Ed25519 account signature against the registered authentication
key over these UTF-8 bytes, with a final newline:

```text
VOID:signal-identity:v1
<canonical API audience>
<authenticated VOID-ID>
1
<registration ID>
<canonical padded Base64 Signal identity public key>
```

Devices verify the separate Signal prekey signatures using official libsignal.
Go's length checks do not replace that native verification or mark a peer verified.

```json
{"type":"prekeys_claim","request_id":"<UUID>","void_id":"<recipient VOID-ID>"}
{"type":"prekey_bundle","request_id":"<same UUID>","void_id":"<recipient VOID-ID>","payload_base64":"<selected public bundle>"}
```

The decoded bundle contains `v`, `device_id`, `registration_id`, `identity_key`,
`account_signature`, `signed_prekey`, `kyber_prekey`, optional `one_time_prekey`
and server `claim_expires_at` in UTC RFC3339. `kyber_prekey` adds `last_resort`.
The recipient need not be connected. Each new claim locks its inventory,
selects/removes at most one EC and one Kyber one-time key, and inserts the claim
in the same transaction. The fallback remains available. No response is accepted
before commit. A failed insert/commit cannot consume keys independently.

```json
{"type":"prekeys_error","request_id":"<publication or claim UUID>","code":"prekeys_unavailable"}
```

Codes are `prekeys_invalid`, `prekeys_conflict`, `prekeys_capacity`,
`prekey_claim_expired` and `prekeys_unavailable`. A new claim for an unknown
recipient is invalid; a registered recipient without inventory is unavailable.
Rate limits, lifetime caps and bounded-worker admission return correlated
capacity errors. Valid UUIDs on malformed decoded command envelopes receive a
correlated invalid error. Malformed raw JSON, nonstring fields and invalid UUIDs
retain the existing invalid-frame/connection-close behavior.

## Activation and compatibility

The new binary accepts the existing exact two-table layout with the extension
disabled, or the exact four-table layout with full prekey schema/grant validation.
It rejects partial installations and unrelated table inventories. A schema change
while running requires a matching restart. Disabled commands return a correlated
unavailable response; startup never creates or migrates tables.

Registration, authentication transcripts, lookup, queued `send`, `accepted`,
recipient `ack`, online relay and 30-day ciphertext retention are unchanged.
No message-delivery receipt, permanent delivered state, conversation table or
advanced message-retry system is introduced.

The frontend's prepared `PreKeyDirectory` remains inactive. Its frontend owner
must connect it after backend migration/deployment approval. Native libsignal
builds and real device exchange have not been verified here. Recipient private
prekey retention must cover the 24-hour claim window plus the existing 30-day
ciphertext queue; no pruning or rotation is enabled by this backend change.

## Verification

Passed the full Go race suite, vet and server build with database/live opt-ins
unset. Mock and in-memory tests cover strict binding/fields, idempotency,
watermarks, limits, concurrent claims, fallback, terminal expiry and rollback.
Real WebSocket tests cover the new frames, offline claims, authentication,
correlated errors/rates and unchanged encrypted forwarding/ACKs.

All eight integration cases passed on an owned Unix-socket-only PostgreSQL 18.6
cluster under a restricted runtime role. They include registration preservation,
actual catalog validation, concurrent claims, persisted retries across service
replacement, fallback, expiry scrubbing and a forced INSERT failure proving that
key consumption rolls back. A follow-up prekey run covered expired UUID reuse
against an unknown recipient. Real server startup/readiness passed against both
base two-table and extended four-table synthetic databases. Test servers and the
private cluster are removed after evidence is archived. No live account or
message was used, and no production schema, credential or network change was made.

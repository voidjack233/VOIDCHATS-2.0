# VOID 2.0 public-prekey distribution proposal

Status: proposal for approval, not an implemented or deployed backend contract.

Owner-review SQL was pushed to the backend branch in commit `220bb65`: [006_signal_public_prekeys.sql](https://github.com/voidjack233/VOIDCHATS-2.0/blob/220bb65/migrations/006_signal_public_prekeys.sql). It creates two new account-linked tables, `signal_prekey_inventory` and `signal_prekey_claims`; neither existing `accounts` nor `messages` is altered. It has not been executed. The current backend's strict two-table validator must be updated before this schema is applied. No filesystem inventory is approved or implemented.

Backend implementation belongs to the owner's separate backend agent. This frontend checkout supplies proposals describing the protocol, public payloads, storage implications and client dependencies; it does not keep a local backend checkout or implement/deploy backend changes.

## Backend review: unresolved before implementation

The backend agent's findings are valid. The SQL in `220bb65` is a review draft, not an activation-ready migration:

- The current validator requires exactly `accounts` and `messages`. Additional tables would fail readiness and subsequent startup until a coordinated backend validator update explicitly permits and validates the proposed schema.
- `claim_expires_at` does not delete records. Retaining full bundles and requester/recipient relationships indefinitely is not an agreed policy. Bundle retention, replay protection after expiry, cleanup and cumulative per-account/global limits must be specified together before implementation. Expired request IDs must never silently allocate another bundle.
- Inventory entry limits currently permit objects larger than the claim table's 32 KiB whole-bundle limit. Per-key validation and schema limits must guarantee that every selectable complete bundle fits the response and claim-storage limits.
- Persistent Signal public-prekey distribution is an explicit architecture extension required by the new asynchronous Signal first-contact goal. It departs from the previous two-table/OpenMLS design; existing encrypted queue and account behavior remain the boundary to preserve.
- Authenticated publication ownership, bounded parsing, atomic selection/removal and idempotent claim handling still need Go implementation. Account-to-Signal binding and Signal signature verification are implemented in the native client source but remain unverified; Go does not decrypt Signal messages.

Neither this handoff nor the frontend commit authorizes applying the draft SQL or changing a live database/deployment.

The inspected backend is `e81056111d4c5ec367b11025d2acbcf31e5ec9d9`. It already authenticates a VOID-ID using its registered Ed25519 key and provides `lookup`, `send`, recipient `ack`, and `relay_online`. It has no public-prekey inventory. Until a prekey extension is approved and implemented, the client cannot promise asynchronous first-contact delivery.

Existing `auth_begin → auth_challenge → auth_proof → auth_ok`, lookup, opaque queued `send`, acceptance, delivery and ACK frames remain unchanged. Go does not initialize Signal sessions, call libsignal, interpret Signal messages, or store private keys, conversations or message history.

## Minimum additional capability

Add authenticated publication of the caller's public prekeys and atomic retrieval of a recipient's public prekey bundle. Retain one public Signal identity per VOID account for this first milestone (`device_id = 1`); multi-device distribution is a separate feature.

The public inventory contains the Signal identity and registration ID, a signed EC prekey, EC one-time prekeys, signed one-time Kyber prekeys and a signed last-resort Kyber prekey. The recipient verifies prekey signatures using official libsignal. The server selects at most one available EC one-time prekey and one available Kyber one-time prekey per claim. It supplies the signed last-resort Kyber key when no one-time Kyber key is available, and does not remove that fallback on claim. This follows Signal's [PQXDH publication and bundle retrieval model](https://signal.org/docs/specifications/pqxdh/).

## Proposed authenticated commands

Keep the existing strict outer JSON string-field encoding. Encode the bounded public JSON object in canonical padded Base64 inside `payload_base64`; Base64 is transport encoding, not encryption.

```json
{"type":"prekeys_publish","publication_id":"<UUID>","payload_base64":"<public upload>"}
{"type":"prekeys_published","publication_id":"<same UUID>","ec_remaining":"<count>","kyber_remaining":"<count>"}

{"type":"prekeys_claim","request_id":"<UUID>","void_id":"<recipient VOID-ID>"}
{"type":"prekey_bundle","request_id":"<same UUID>","void_id":"<recipient VOID-ID>","payload_base64":"<public bundle>"}
```

Use a correlated extension error response, rather than changing existing relay errors:

```json
{"type":"prekeys_error","request_id":"<publication or claim UUID>","code":"<allowlisted code>"}
```

The implemented native adapter prepares these public upload fields, proposed for the backend extension:

- `v: 1`, `device_id: 1`, `registration_id`, `identity_key`, `account_signature`;
- current `signed_prekey`: `{key_id, public_key, signature}`;
- current `kyber_last_resort`: `{key_id, public_key, signature}`;
- append-only `one_time_prekeys`: `[{key_id, public_key}]`;
- append-only `one_time_kyber_prekeys`: `[{key_id, public_key, signature}]`.

Key and signature fields are canonical padded Base64; IDs are JSON integers. A claimed bundle contains `v`, `device_id`, `registration_id`, `identity_key`, `account_signature`, `signed_prekey`, `kyber_prekey` and optional `one_time_prekey`. The selected `kyber_prekey` additionally has `last_resort: true|false`. It never contains private record serialization. It also includes `claim_expires_at` as a server-generated RFC3339 timestamp; clients must establish their session before that expiry. The claim's idempotency window bounds its use to allocate a fresh initial session.

An upload with empty append batches returns current counts, so a separate status command is unnecessary. Resuming publication after a lost reply uses the same publication ID and exact public payload. Different bytes under that ID are a conflict. Reuploading an old one-time key ID must never make a claimed key available again; key-ID watermarks or tombstones must enforce this across publications and reconnects.

Publication ownership comes exclusively from the authenticated connection. The upload contains no caller-selected owner VOID-ID. A claim's recipient must exist. Errors distinguish unavailable inventory from account lookup failure without exposing storage details.

## Registered-account binding

The existing Ed25519 account key and the new Signal identity are different keys. Neither is converted into or substituted for the other. The Signal identity is bound to the registered account using an existing-account Ed25519 signature verified by clients against the result of `lookup`.

Proposed exact domain-separated UTF-8 binding message, with a final newline:

```text
VOID:signal-identity:v1
<canonical API audience>
<VOID-ID>
<device ID>
<Signal registration ID>
<canonical padded Base64 Signal identity public key>
```

This is an application identity-binding signature, not a replacement handshake or encryption algorithm. The native adapter must sign only this validated message shape. Libsignal continues to verify its own prekey signatures and session identities. The binding does not mark a contact manually verified. A changed pinned Signal identity or account key pauses messaging; publication must not silently replace an existing identity. Identity replacement/recovery needs its own explicit policy.

## Atomicity and retention

Claim selection and removal from available inventory must commit together before the response. Concurrent claims cannot receive the same one-time keys. Scope claim idempotency by authenticated requester, request UUID and recipient; an exact retry returns the same public bundle, whereas reuse for another recipient is rejected. Do not consume more keys merely because a response was lost. Do not return claimed keys to the inventory after timeout.

Persist the selected public bundle for the bounded claim retry window. A retry of an expired claim returns a specific expired result and cannot silently allocate replacement keys. A deliberate new request uses a new UUID. This claim record is public-key distribution metadata, not a conversation or delivery receipt.

Private key generation and persistence happen on the recipient before publication. A server claim does not instruct the device to delete its private prekey. The recipient commits libsignal's successful decryption, consumed-prekey/session changes and local message journal together before returning the existing queue ACK. Any decryption or persistence failure retains the queued event.

Never remove historical private signed/last-resort prekeys merely because a replacement was published. Retention must cover the last allowed use of the old public bundle plus the existing 30-day ciphertext queue lifetime. The client must respect claim expiry when creating the initial message. Exact rotation, claim TTL and local private-key pruning values must be agreed before pruning is enabled; no automatic key cleanup is required for the first two-account milestone.

## Narrow backend scope requiring approval

The necessary server changes are public inventory/claim persistence, two authenticated WebSocket commands with their correlated responses, bounded upload parsing, atomic claim/idempotency handling, per-account/key limits and rate limits. No change to queue deletion on recipient ACK, queue retention, deployment settings, account registration, authentication signing formats, existing relay messages or message payload interpretation is proposed.

The physical database migration and chosen limits/TTL should accompany the backend implementation review after this capability is approved. A TypeScript `PreKeyDirectory` adapter and response decoding are drafted in source, but the adapter is not wired into `MessagingProvider`; the running provider does not send these proposed commands. No Go source, database, deployment or live account was modified while preparing this proposal; only review SQL was pushed separately in `220bb65`.

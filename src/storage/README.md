# Encrypted local storage interface

This package exposes public application records through the Android `VoidStorage` native module. It contains no JavaScript database driver, SQL execution API, encryption key, private signing key, or serialized MLS state. The native implementation opens a SQLCipher database using a device-protected key.

`StorageProvider` wraps the app. On mount it calls `openExisting()`, which opens an existing encrypted store without creating or replacing identity material. A new installation remains `not_initialized`. `createDevelopmentStorage()` is an explicit debug-only action; both this facade and native `BuildConfig.DEBUG` enforce that restriction. It creates a local device identity, not a registered VOID account. `accountVoidId` remains null until a future explicit account association.

`useStorage()` provides `status`, sanitized `message`/`errorCode`, public `identity`, `schemaVersion`, a global `revision`, typed `repositories`, and the lifecycle actions `openExisting`, `createDevelopmentStorage`, `closeStore`, and `removeLocalData`. Status is `opening`, `not_initialized`, `ready`, `locked`, or `error`. Lifecycle actions are serialized, and provider cleanup closes its controller's store. Key or database failures preserve existing data; no fallback creates a replacement identity or plaintext database. The caller must obtain explicit user confirmation before `removeLocalData()`.

## Models and repository operations

IDs are stable strings supplied by the native/client ingestion flow. Public timestamps are nonnegative safe integer epoch milliseconds. The [types](types.ts) define the full DTO contract.

| Repository | Operations and behavior |
| --- | --- |
| `conversations` | `page`, compatibility `list`, `get`, `save`, `markRead`; direct peer or group metadata, members, latest preview, unread count |
| `messages` | `page`, `get`, `save`, `edit`, `delete`, `setDeliveryState`, `setReaction`, `setReceipt` |
| `contacts` | `list`, `get`, `save`, `remove`; public identity data with `unverified` or `changed` trust |
| `requests` | `list`, `save`, `setStatus`; persistent `pending`, `accepted`, `declined`, or `blocked` status |

`messages.page(conversationId, before = null, limit = 50)` returns `{items, nextCursor}`. Items are newest first by `(createdAt, id)`; `nextCursor` contains the last returned row's values when another page exists. Limits are 1–100. This ordering handles timestamp ties without offset-based duplicates after newer inserts.

`conversations.page(before = null, limit = 50)` uses the same bounded result shape with an `(updatedAt, id)` activity cursor. The conversation screen loads this API incrementally. Its decoder rejects duplicate or unordered rows, mismatched cursors, oversized pages, and rows outside the requested cursor range. The compatibility `list()` method remains available, but the screen does not use it.

`conversations.markRead(conversationId, through, readAt)` requires a cursor belonging to that conversation. Per-message read timestamps distinguish late arrivals from already read history. An edit preserves message identity and creation time. Deletion stores a tombstone, removes the body/reactions, and cannot resurrect the message. Delivery and receipt state cannot regress. Duplicate transport saves preserve existing edits, read state, and tombstones.

Repository writes only persist records. They do not send messages, contact the backend, verify a sender, or create an account. Outgoing records enter as `pending`; delivery transitions belong to a future real delivery adapter. Saving a contact cannot choose verified trust. A changed public key is marked `changed`. Accepting a saved request is a local status change and does not verify the sender.

## Native boundary and schema

Native lifecycle methods return JSON strings. `execute(operation, payloadJSON)` accepts only the operation names in [types.ts](types.ts), and native code validates each payload's exact fields before binding SQL parameters. Public operations cannot name tables or access `native_mls_state` or `crypto_events`. The TypeScript facade validates JSON responses and reports fixed error messages without native exception details.

The initial [SQL migration](../../android/app/src/main/assets/storage/001_initial.sql) defines constraints, foreign keys, indexes, and transition triggers. Native connections enable foreign keys. Numbered migrations run transactionally; their checksums and `user_version` are verified on open. SQL assets use LF line endings. Add a new migration instead of editing an applied one.

Private MLS state is a native-only singleton snapshot with a revision used for compare-and-swap checkpoints. Native cryptographic transactions couple those checkpoints with a protected journal. Decryption entries retain authenticated sender identity, sender public key, exact decimal-u64 epoch, and checkpoint revision alongside their encrypted-at-rest content. `openMls.readStoredDecryptions()` exposes a bounded subset of accepted application payloads and authenticated public metadata for recovery; it exposes no private MLS state or database keys. The journal is not yet an application delivery queue. Transport ingestion, visible-record materialization, deduplication and retention remain unfinished. Deleting a visible message does not currently remove its independent journal entry; see [the local storage boundaries](../../docs/local-storage.md). Replaying a decrypt operation is not the recovery mechanism.

## Hooks and refresh

`useContacts()` and `useRequests(status?)` return `{items, loading, error, refresh}`. `useConversations(limit?)` and `useMessages(conversationId, limit?)` also provide `loadMore` and `hasMore`. Successful repository mutations publish a new global revision, causing subscribed views to refresh. List data stays visible while refreshing within the same identity. Conversation and message refresh retain the loaded depth.

Cached results are scoped to the current local device ID; messages are also scoped to their conversation. Closed storage and a replacement identity immediately hide old records before effects run. Pending requests are canceled logically with generation checks so stale responses cannot overwrite a newer view. A chat read-marking effect should guard on a stable unread batch/cursor to avoid a revision-refresh loop.

## Checks

From the repository root:

```sh
npm run typecheck
npm run lint
node --test src/storage/storage.test.mjs
```

The host tests use Node's built-in `node:sqlite` and isolated synthetic fixtures, including a temporary database outside the app for reopen verification. They cover schema constraints, stable pagination, read state/late arrivals, edits/deletion/reactions, delivery/receipts, contact trust, request transitions, native engine CAS, bridge restrictions, sanitized failures, controller lifecycle, and cached-view scope. This host-only test driver is never imported by the application.

SQLCipher encryption, device-key handling, native migration execution, and real OpenMLS persistence require the separate Android/Rust tests. See [local storage architecture](../../docs/local-storage.md) and [native MLS documentation](../../native/openmls/README.md) for those boundaries and recorded platform verification.

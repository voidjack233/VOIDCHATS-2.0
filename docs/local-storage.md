# Encrypted local storage

VOID stores conversation metadata, messages, contacts, and requests in an on-device SQLCipher database. OpenMLS checkpoints use the same database and native transaction boundary. The local store operates independently of any remote service.

The direct-messaging update opens a local thread and composer immediately after
VOID-ID lookup. Text entered before MLS setup completes is saved as a pending
outgoing message in SQLCipher, without sending plaintext to the backend. Automatic
setup exchanges travel through the durable opaque event queue, so users can open
the app at different times. Native code encrypts the saved text once the group
is usable and retains its stable message ID and exact ciphertext for retries.

## Components

| Location | Responsibility |
| --- | --- |
| `src/storage/types.ts` | Public records, pagination cursors, lifecycle states, allowed native operations |
| `src/storage/bridge.ts`, `native.ts`, `decode.ts` | Native module lookup, sanitized failures, validated public responses |
| `src/storage/repositories.ts` | Typed conversation/message/contact/request operations; no SQL or secret-table access |
| `src/storage/controller.ts`, `provider.tsx` | Explicit lifecycle, committed-write notifications, identity-scoped read hooks |
| `android/app/src/main/java/com/voidchats/storage/` | SQLCipher, Android Keystore key wrapping, migrations, repositories, shared native queue/transactions |
| `android/app/src/main/assets/storage/` | Reviewed, numbered SQL migrations |
| `native/openmls/src/storage.rs` | Native storage callback contract; unavailable to the React Native bridge |
| `native/openmls/src/persistence.rs` | Complete versioned engine/provider serialization and restoration |
| `native/openmls/src/engine.rs` | Checkpointed MLS mutations, ratchet persistence, stale-engine rejection |

The JavaScript-facing storage module accepts named, validated operations and parameterized values. It exposes neither arbitrary SQL nor database keys nor serialized MLS state. Message plaintext is available to the UI when storage is open; private MLS state stays inside Rust/native code.

## Schema and migrations

Migration `001_initial.sql` creates these tables:

- `conversations`, `conversation_members`: stable IDs, direct/group metadata, public members, activity ordering and local read position. Conversation pages use an `(updated_at, id)` cursor.
- `messages`: sender ID, body, direction, delivery state, timestamps, edits, deletion tombstones and read time. Pagination orders by `(created_at, id)` and uses that pair as a cursor.
- `message_reactions`, `message_receipts`: foreign keys, uniqueness and monotonic receipt states.
- `contacts`: public identity information with `unverified`/`changed` state. Saving a public key does not verify its owner.
- `message_requests`: legacy public request records with optional conversation association and persistent status. The current message inbox classifies Requests through local session metadata and does not require a request-acceptance action.
- `local_identity`: public device identity metadata and primary native client handle. Account association remains null for local development initialization.
- `native_mls_state`: one private, versioned engine checkpoint and monotonically increasing transaction revision.
- `crypto_events`: encrypted native inbox/outbox material, authenticated sender metadata, epoch and checkpoint revision. This table is not exposed through public repository operations.
- `key_packages`: public package bookkeeping; private package records and consumption state belong to the OpenMLS checkpoint.
- `app_settings`, `storage_migrations`: bounded preferences and applied SQL history/checksums.

Foreign keys, indexes, checks and triggers reject invalid identities, duplicate direct peers, message repointing, tombstone resurrection, invalid delivery/request transitions, read-receipt regression and reactions on deleted messages. Public repository duplicate insertion preserves existing edits, delivery/read state and tombstones. There are no seed users, conversations or messages in application migrations or initialization.

`002_account_registration.sql` adds durable registration state. The additive
`003_messaging.sql` adds local direct sessions, exact-ciphertext operations,
online-relay replay bookkeeping, staged/applied/rejected inbox records and local
blocks. These are device records; the backend has no conversation or KeyPackage
inventory. `004_optional_account_username.sql` makes the local registration name
optional by rebuilding its constrained table and copying existing values. It
does not replace the protected identity, private keys, legacy names or account
association. Installed migrations 001–004 remain unchanged; the direct-message
update uses a forward migration for pending outgoing text and local Requests
classification, preserving already established groups and account keys.

`StorageMigrations` discovers contiguous numbered SQL assets, validates applied names/checksums and SQLite's `user_version`, then applies pending migrations in one transaction. It rejects edited, incomplete, unknown/future history. Add a new numbered migration rather than editing installed SQL. `.gitattributes` preserves LF bytes across platforms so checksums remain stable. A schema version change does not grant permission to reinterpret an incompatible MLS checkpoint.

## Protection at rest

The Android app uses `net.zetetic:sqlcipher-android:4.19.1` with `androidx.sqlite:sqlite:2.7.0`, following the [official Android integration](https://github.com/sqlcipher/sqlcipher-android). Opening verifies `cipher_version` and `cipher_status`, reads the schema with the supplied key, and requires the configured security pragmas. There is no ordinary SQLite fallback. SQLCipher 4 encrypts database pages with AES-256-CBC and authenticates them with HMAC-SHA512; its journals encrypt page data as well. See [SQLCipher's design](https://www.zetetic.net/sqlcipher/design/).

A fresh 32-byte database passphrase is generated with Android `SecureRandom`. A non-exportable Android Keystore AES-256-GCM key wraps that passphrase using authenticated additional data bound to the app's storage alias. The wrapping key never enters TypeScript. The wrapped key file uses `AtomicFile`; unwrapped passphrase buffers are cleared when the database closes. A shared directory-scoped lock serializes initialization and removal across overlapping React contexts so they cannot replace the same wrapping alias concurrently. The current manifest uses one app process; a future multiprocess Android design would require cross-process lifecycle coordination. Keystore hardware protection varies by device and is not inferred from running on an emulator. See [Android Keystore](https://developer.android.com/privacy-and-security/keystore).

The database, wrapped key and auxiliary files live under `noBackupFilesDir`. Android backup is disabled, with explicit backup/data-extraction exclusions. The connection uses rollback-journal `DELETE` mode, `synchronous=FULL`, foreign keys, `secure_delete`, memory-only temporary storage and SQLCipher memory security. SQLCipher's default corruption handler can delete files; VOID supplies a handler that preserves them and fails closed. No plaintext database, key file, export or backup is created.

Encryption at rest protects closed files; it does not make an unlocked app safe against a compromised OS, runtime or debugger. Message text reaches JavaScript for display. Native buffer cleanup is best effort, not a complete process-memory erasure guarantee. Deletion/secure-delete settings do not guarantee forensic erasure on flash storage or previously copied encrypted files.

## Durable MLS state and commit ordering

The engine checkpoint includes every client and stable handle, signing identity, complete OpenMLS provider record map, group IDs, pending commits, KeyPackage records, sender/receiver ratchets, replay state and single-use snapshot ownership. It pins its envelope format, OpenMLS version and provider storage version. Restoration reloads signers and groups from their provider records; it does not create replacements when records are missing or incompatible.

Every mutation validates the loaded revision, advances OpenMLS, flushes cached group ratchet state using OpenMLS's supported storage API, and saves a complete checkpoint with compare-and-swap revision advancement. This includes protocol rejection paths: a failed authentication can still consume a generation. Restoring earlier state to retry such a message would be unsafe.

On Android, one bounded native queue serializes database and MLS operations. A native transaction contains the checkpoint and corresponding encrypted crypto-event/public bookkeeping writes. The transaction commits before a Promise can return plaintext, ciphertext, a KeyPackage or a commit to JavaScript. A protocol error still commits a successful checkpoint while rejecting the operation. Storage, journal or final-commit failure poisons/closes the engine and emits no operation result.

The crypto journal preserves material needed if the process dies after native commit but before the visible history update. Sender identity, signing public key, epoch and checkpoint revision are saved with accepted incoming plaintext. `openMls.readStoredDecryptions()` reads accepted application payloads and public sender metadata without advancing a ratchet. Its cursor uses `(createdAt, eventId)`, pages contain at most 100 records and 4 MiB of encoded data, and malformed metadata fails closed. Private checkpoints and keys are never part of this response.

`MessagingStore` receives the authenticated socket event and verifies its local
session, peer signing key/credential and encrypted application envelope. Its
first transaction commits the receiving ratchet, authenticated plaintext journal
and `staged` inbox row. A second transaction materializes the visible message and
marks that inbox row `applied`. Only then does native code return an ACK token to
the socket coordinator. Reopening messaging or replaying an event recovers staged
rows from the journal without decrypting twice. Duplicate sender/event IDs must
have the same saved payload hash. Invalid messages are durably rejected before
ACK so they cannot hold the backend's delivery window indefinitely. The journal
retains accepted plaintext independently of visible message tombstones; deleting
a visible message does not erase that separate record. Journal-retention and
deletion policy remain separate work.

The SQLite transaction gives crash consistency within the supported filesystem's commit guarantees. A revision rejects concurrent/stale engines; it does **not** prevent an attacker from restoring an entire older authenticated database/key-file copy. There is no claim of hardware monotonic antirollback protection. Automatic backups are excluded, but root access, deliberate filesystem rollback, faulty storage and OS compromise remain outside that guarantee.

## Lifecycle and UI

| Situation | Behavior |
| --- | --- |
| First launch | Checks for an existing encrypted store; creates no identity or account automatically |
| Explicit development setup | Settings creates a protected local device identity and empty store; debug-only native guard; no account registration or sample data |
| Existing store | Opens with the original key and verifies primary public metadata against the restored native identity |
| Process restart | Reopens committed SQL records and MLS checkpoints; registered accounts reconnect with the same signing key and recover staged inbox writes |
| Missing/invalid key or corrupted/incompatible state | Reports a sanitized error and preserves files; no new key, identity or plaintext fallback |
| Close local storage | Closes native engine/database and hides identity-scoped rows and chat metadata |
| Delete local data | Requires explicit UI confirmation, closes storage, deletes this store's files and Keystore alias; irreversible local identity removal |
| Interrupted initialization/removal | Preserves inconsistent remaining material and fails closed until explicit cleanup; never treats it as an existing healthy identity |
| App data deletion/uninstall | OS removes local application data/Keystore ownership; reopening has no recoverable identity |
| Release build | Development initialization and self-tests are rejected natively; key-only registration and native messaging are available |

In a debug build, complete startup acknowledgments, open **Preview VOID → Profile → Settings → Local storage**, then select **Set up local storage**. This creates no registered account. Opening an existing store restores it automatically. Registered accounts use authenticated VOID-ID lookup and can compose immediately. Native setup and pending-message encryption run automatically; no invitation approval or user-managed key establishment is needed. First incoming messages from an unfamiliar peer remain in Requests until the local user replies. Chats render saved chronological history and local read counts; server acceptance of application ciphertext appears as **Sent to server**. Pending text awaiting encryption is not marked sent. Delivery/read receipts are deferred, and contact identity remains unverified or changed. UI preview alone cannot execute the native crypto engine.

The current backend has no prepublished recipient KeyPackages. A first-time
conversation therefore needs alternating connections to finish the queued
bootstrap: the sender must reconnect after the recipient supplies its package
before the pending application text can be encrypted. Simultaneous presence is
unnecessary. Existing groups can encrypt and queue messages immediately while
their recipient is offline. See the [bootstrap boundary](offline-first-bootstrap.md).

Closing storage preserves the device identity and records for reopening. Removing local data deletes this device's protected store and signing identity; it does not affect any remote accounts. If another app context still holds the same store open, removal fails before deleting files or the wrapping alias; that context must close first. Registered-account logout, remote account removal and recovery require a future account/session design.

Legacy JavaScript methods that could replace, suspend or release the protected primary identity are rejected. Account removal uses the explicit storage lifecycle instead. Rust's isolated memory-engine lifecycle and self-tests remain available for automated verification; secret snapshots never become JavaScript values. The primary handle is public local metadata, not an account identifier.

## Build and checks

Use the existing commands:

```sh
npm install
npm run typecheck
npm run lint
node --test src/storage/storage.test.mjs
node --test src/crypto/recovery.test.mjs
npm run crypto:test
npm run crypto:bridgecheck
npm run android:build
```

The Android native build needs the configured SDK, NDK 27.1.12297006, JDK 17, and the pinned Rust toolchain. Set `JAVA_HOME` to your JDK 17 installation and put its `bin` directory first on `PATH` before building. Start Metro with `npm start`; in another terminal, `npm run android` installs the custom build on a connected device. Expo Go, iOS and web do not provide this encrypted native storage integration and report it as unavailable.

Android instrumentation uses isolated storage namespaces and Keystore aliases, separate from the application's real store. Tests may create synthetic records only in these test stores. Cross-process tests seed committed state in one instrumentation process and verify it in a later process. Host Rust restart tests use a vetted AES-GCM fixture store and child OS processes; they verify the engine's restart behavior and do not substitute for testing Android SQLCipher/Keystore.

Verification completed on 2026-10-15:

| Check | Actual result |
| --- | --- |
| Storage schema/repository/controller/bridge tests | 18 passed, using real host SQLite for SQL behavior and isolated typed bridge fixtures |
| Stored-decryption response tests | 4 passed, including malformed/private fields, ordering, cursor binding, u64 bounds and page size |
| TypeScript and ESLint | Passed; no lint warnings |
| Locked Rust tests | 14 passed; the intentionally ignored worker was executed by its parent in 10 separate OS processes |
| Rust formatting and Clippy | Passed, including `-D warnings` |
| Generated Kotlin/JNA bridge | 2 host JVM tests passed against the current Rust library |
| Android encrypted storage instrumentation | 15 passed, 0 failures/errors/skips on Android 16 / API 36 / x86_64 |
| Android process interruption/restart | Seed and verify each passed in separate processes; the intervening crash was intentional and reported as a crashed process |
| Installed release restrictions | 1 passed against a non-debuggable x86_64 release APK: development identity creation and native self-test both rejected without creating a key/store |
| ARM64 builds | Debug and release APK assembly passed; both contain SQLCipher and OpenMLS for `arm64-v8a` |
| Actual React Native UI | Startup, explicit setup, close/reopen, force-stop/relaunch restoration, empty conversation/contact/request lists, and confirmed removal passed through the native bridge |

The Android suite verifies encrypted file contents and Keystore roundtrip, missing/tampered keys, corruption and empty/truncated-file preservation, checksum/future-schema rejection, concurrent initialization, refusal to remove another context's active store, stable message/conversation cursors, unread/tombstone/contact behavior, large native snapshot reads, and bounded committed-decryption recovery without ratchet advancement. Its XML is preserved locally at `android/app/build/outputs/androidTest-results/encrypted-storage-15-tests.xml`.

The interruption scenario used distinct app process IDs: seed 5082, crash 5126, verify 5166. Before killing the crash process inside an uncommitted MLS checkpoint/journal transaction, the test enabled cache spilling and asserted a nonempty rollback journal. After death, the encrypted journal was 52,760 bytes. Reopening retained the exact seeded revision and event count, restored identity/history, kept replay rejection, accepted a subsequent reply, and committed the rejection-path checkpoint. A forced journal insertion failure separately withheld the encrypted output and rolled back its checkpoint. These checks demonstrate the tested process-failure cases; they do not simulate sudden power loss or every filesystem/device failure.

For emulator instrumentation, build/install the x86_64 app and test APK once. From a terminal with JDK 17 and `adb` configured:

```powershell
cd android
.\gradlew.bat :app:assembleDebug :app:assembleDebugAndroidTest -PreactNativeArchitectures=x86_64 --console=plain
cd ..
adb install -r android/app/build/outputs/apk/debug/app-debug.apk
adb install -r android/app/build/outputs/apk/androidTest/debug/app-debug-androidTest.apk
adb shell am instrument -w -e class com.voidchats.storage.EncryptedStorageTest com.voidchats.test/androidx.test.runner.AndroidJUnitRunner
```

To reproduce process phases, use that same installation and run these commands in order. Separate Gradle connected-test invocations can uninstall the app during teardown, so they must not be used between phases:

```sh
adb shell am instrument -w -e class com.voidchats.storage.StorageProcessRestartTest -e phase seed com.voidchats.test/androidx.test.runner.AndroidJUnitRunner
adb shell am force-stop com.voidchats
adb shell am instrument -w -e class com.voidchats.storage.StorageProcessRestartTest -e phase crash com.voidchats.test/androidx.test.runner.AndroidJUnitRunner
# The preceding process must crash before completing its transaction.
adb shell am instrument -w -e class com.voidchats.storage.StorageProcessRestartTest -e phase verify com.voidchats.test/androidx.test.runner.AndroidJUnitRunner
```

The current ARM64 APKs are `android/app/build/outputs/apk/debug/app-debug.apk` and `android/app/build/outputs/apk/release/app-release.apk`. Emulator verification APKs are preserved under `android/app/build/outputs/apk/verification/`. Release assembly and restriction checks use the repository's existing development signing configuration; production signing remains unfinished.

The checks above describe the 2026-10-15 storage build. Physical-device hardware
security and sudden-power-loss behavior remain unverified. Current messaging
changes add registered account binding, authenticated socket transport and local
message ingestion; their two-device runtime check is the owner's
[phase 5 task](two-device-messaging-test.md). There is no cloud backup, account
recovery or multi-device synchronization.

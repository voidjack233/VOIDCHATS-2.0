# VOID 2.0

React Native and TypeScript frontend with official libsignal for 1:1 messaging and SQLCipher protected local storage. Android is implemented first; screens, navigation, WebSocket coordination and the messaging interface remain shared with future iOS integration.

**Current status:** source integration only. No Signal build, test, APK or device exchange has run. The Android source version is **2.0.0**, code **5**; existing 1.0.3 APKs predate this change. First-contact messaging still needs the [public-prekey backend proposal](docs/signal-prekey-backend-proposal.md) finalized and implemented. The drafted client adapter is inactive, and the running provider keeps such messages saved locally. Backend validator compatibility, claim retention and size bounds remain unresolved in the SQL review draft.

## Messaging

Account setup uses the protected Ed25519 account identity, with no username. Authentication uses the backend's audience-bound WebSocket challenge. VOID-ID lookup retrieves the registered public key without marking the contact verified.

A separate native Signal identity is bound to that account key. Official libsignal handles signed and one-time EC/Kyber prekeys, session establishment, encryption and decryption. Native SQLCipher transactions persist ratchet/prekey changes with ciphertext, history and deduplication. Recipient ACK follows a successful durable commit. Backend `accepted` means **Sent to server**; delivery receipts remain deferred.

The composer and Requests UI are retained. There is no conversation acceptance step. Once public prekey distribution is available, a published recipient bundle supports first-contact sending while the recipient is offline. Established sessions use the existing opaque `send` queue. No Go implementation or database/deployment change has been made for the proposed extension.

See [Signal implementation and current limitations](docs/signal-direct-messaging.md) for storage, key preservation and the prekey approval boundary.

## Dormant OpenMLS

The entire `native/openmls/` directory is preserved. OpenMLS is reserved for future groups and is not used in the new 1:1 flow. Android no longer builds or packages its Rust library/generated bindings, JNA or React module. Existing local MLS checkpoints, groups, legacy outbox records and history remain stored; they are not converted or automatically retransmitted. Historical MLS source, scripts and verification records remain in the repository.

## Development

Tests, builds, APK generation, migrations and deployments are paused until explicitly authorized. The following commands describe the existing workflow and have not been run for this integration.

Use `npm install` to install frontend dependencies. `npm start` runs Metro; `npm run android` builds/installs the custom Android app when authorized. Expo Go can preview UI but cannot run native libsignal/SQLCipher. Configure the Android SDK, NDK 27.1.12297006 and JDK for the React Native toolchain; Gradle obtains the pinned official Signal artifacts from Signal's Maven repository.

`npm run android:apk` publishes a standalone APK when authorized. The existing publisher keeps one `build/apk/v<versionName>/VOID.apk` per version, overwrites that version on rebuild and retains only two versions. `build/apk/latest.json` points to the latest actually produced artifact; it does not yet refer to Signal 2.0.0. No extra APK copies were created.

The existing iOS React Native project remains. Its Signal native adapter is not implemented; missing native support fails explicitly with no plaintext fallback.

## Source map

- `src/messaging/`: shared coordinator, existing authenticated relay client and platform-independent native/prekey interfaces.
- `android/app/src/main/java/com/voidchats/storage/`: account-key preservation, official Signal adapter, encrypted repositories and native React bridges.
- `android/app/src/main/assets/storage/`: additive local schema assets; no migration was executed during this change.
- `src/chat/`, `src/conversations/`, `src/requests/`: current chat UI.
- `native/openmls/`, `src/crypto/`: dormant future-group source and historical integration.

The Go relay is maintained on the [backend branch](https://github.com/voidjack233/VOIDCHATS-2.0/tree/backend) by the owner's separate backend agent. Backend changes needed by this frontend are handed off as proposals; this checkout does not contain a backend clone. Group messaging, account recovery, multiple enrolled devices, delivery receipts and automatic Signal prekey rotation/replenishment are outside this implementation.

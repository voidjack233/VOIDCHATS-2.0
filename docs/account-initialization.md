# Account initialization

The Android app now connects its startup account screen to the signed registration API on the [backend branch](https://github.com/voidjack233/VOIDCHATS-2.0/tree/backend). Registration creates an account association for this device's existing protected signing identity. It does not establish a login session or enable message delivery.

## Run and configure

Build and install the custom native Android app using [the native setup instructions](../native/openmls/README.md). The registration bridge is available in debug and release builds. Rebuilding is required for the new Rust, generated UniFFI, Kotlin, and storage methods; refreshing JavaScript in an older app is insufficient. Expo Go, web, and the unfinished iOS native integration cannot initialize accounts. Debug **Preview VOID** still opens the UI without registering.

The public API base is configured in [src/config/api.ts](../src/config/api.ts):

```text
https://api.voidchats.xyz/dm9pZG5vdGhpbmduZXNz
```

The client requires HTTPS and sends JSON to `/challenges` and `/accounts` under that base, with `User-Agent: VOIDCHATS/2.0`. It uses a bounded request timeout, rejects unexpected responses, and respects a returned `Retry-After` delay. `SOCKET_URL` remains empty because message transport is not connected.

The Android client uses text XMLHttpRequest transport so the native connection failure can be classified before React Native's fetch wrapper reduces it to a generic network error. Setup distinguishes DNS, HTTPS certificate, timeout, refused/reset connection, and missing network-route errors. The error includes the public endpoint and any HTTP status, without displaying raw native exception text. The APK uses the API hostname and does not discover a running backend automatically; changing the configured base requires rebuilding. The main `voidchats.xyz` website has separate routing.

### Expo Go shows a native-build requirement

Scanning the Expo QR code opens Expo Go, whose installed binary does not contain VOID's storage or Rust signing modules. JavaScript refreshes cannot add these modules. The account screen explains this requirement before submission and keeps development preview available. Install the custom VOID Android app to initialize an account.

For a standalone installation, run `npm run android:apk` on a computer with the native build tools. Transfer `android/app/build/outputs/apk/release/app-release.apk` to your Android phone, open it, approve installation from that source if Android asks, and launch **VOID**. The release variant embeds JavaScript and all required native modules, so it runs without Metro, Expo Go, a QR code, USB debugging, or an emulator. This testing APK uses the existing development signing key; it is not configured for store distribution.

For live development after installing the Android SDK/NDK, JDK 17, and Rust toolchain described in the native setup instructions, connect an Android device with USB debugging enabled. Start Metro with `npm start`. In a second terminal, verify `adb devices`, run `adb reverse tcp:8081 tcp:8081`, and use `npm run android` to build and install VOID. Open the installed **VoidChats** app instead of scanning the Expo Go QR code. `npm run android:build` continues to build the debug variant, which uses Metro.

## User flow

Complete the startup guidance and acknowledgments, enter a username, then choose **Create account**. Names may contain 1–32 Unicode code points, without surrounding whitespace, control characters, format characters, or invalid Unicode surrogate values. A username is display metadata; it is not a password or a recovery secret.

The screen reports preparation, connection, identity confirmation, registration, and local saving. While a request is in progress, it disables repeated submission, editing, back navigation, and development preview. Errors remain visible with an explicit retry action. Successful local completion opens the messenger, and Profile and Settings display the stored VOID-ID. Profile also displays the username. On subsequent launches, the app opens the protected store and resumes the registered account without calling registration again.

An existing unregistered identity from development storage setup is reused. It remains in startup on later launches until the user completes account setup or explicitly opens development preview.

## Registration protocol and local persistence

1. `prepareAccountRegistration(username, apiUrl)` creates an encrypted store and native MLS identity only when no store exists. It reuses existing identity material and records the intended username and API URL.
2. `POST /challenges` sends `{username, auth_public_key}` using the identity's public Ed25519 key. The response contains `{challenge_id, message, expires_at}`.
3. The client validates the challenge against the expected ID, expiry, public key, and UTF-8 username. The native bridge independently validates the constrained registration message before Rust signs it. Private keys and serialized MLS state stay native.
4. `POST /accounts` sends `{challenge_id, signature}`. A successful response contains `{void_id, username, created_at}`. The VOID-ID is validated and retained as a string, preserving its full integer precision.
5. `completeAccountRegistration` checks the response against the pending username, API URL, and signing key, then atomically saves the public account association in SQLCipher. The storage snapshot updates only after that commit.

The signed message uses the domain `VOID-NOTHINGNESS:register:v1`, followed by newline-separated challenge ID, RFC 3339 expiry, padded public-key base64, padded username UTF-8 base64, nonce, and a final newline. The bridge exposes a registration-specific signer rather than an arbitrary signing API.

Migration `002_account_registration.sql` adds public registration metadata and prevents replacing an existing non-null account binding. The initial migration remains unchanged. Missing, incompatible, or damaged existing storage fails closed; setup does not silently regenerate an identity.

## Retry and recovery boundaries

The client does not automatically repeat account POSTs. A challenge can expire or be consumed even when a submission fails, so another setup attempt requests a fresh challenge using the same persisted signing identity. Network failures, rejected proof, rate limits, and server capacity errors do not create replacement keys.

If the app receives a successful account response but local saving fails, it retains that response in memory. **Retry account setup** retries the local commit before contacting the backend again; the pending username remains fixed. This pending response survives screen changes within the provider's lifetime, but not application process termination.

If the server creates an account and its response is lost, or the process exits before a received response is saved, a later attempt may return `key_registered`. The backend currently exposes no account lookup or recovery endpoint, so the app preserves local data and reports that recovery is unavailable. It cannot reconstruct an unknown VOID-ID, assume registration succeeded, or replace the device identity to hide the failure. Removing local data does not delete the server account and loses the protected identity kept by this installation.

Account initialization does not implement authenticated sessions, logout, remote account deletion, key rotation, multi-device setup, account recovery, remote key exchange, VOID-ID discovery, or network message transport. Conversation and message screens still read local records, with sending disabled.

## Verification

Useful host checks are:

```sh
npm run typecheck
npm run lint
node --test src/account/account.test.mjs src/storage/storage.test.mjs
```

The account host tests exercise protocol validation, request handling, and retry coordination with controlled responses; they do not create a production account or prove the Android bridge works on a device. Native verification additionally requires `npm run crypto:test`, `npm run crypto:bridgecheck`, Android build/tests, and a device check of first setup, failed setup, local-save retry, and restart restoration.

For this change, TypeScript, lint, all 63 account/storage/crypto recovery host tests, and the production Android JavaScript bundle passed. No production account was created by these checks. Rust formatting and the isolated SQL migration checks also passed. Portable JDK 17, Android SDK 36, Build Tools 36.0.0 and NDK 27.1 are now installed in the ignored `android/build/tooling` folder. With that toolchain, the native Rust suite passed 15 tests; its one ignored fixture is invoked by the process-restart test. This includes the new verbatim Ed25519 account-proof test. Actual phone verification of first registration, encrypted persistence and restart restoration remains necessary. Earlier native build and storage results in other documents describe the implementation at the time they were recorded.

The standalone release APK was built successfully with Gradle 9.4.1 and AGP 9.2.1. Its v2 signature and 16 KiB APK alignment passed verification. Archive inspection confirmed the bundled JavaScript, both SQL migrations, native OpenMLS/SQLCipher/JNA libraries and the storage/crypto module classes. The installable artifact is copied to `build/VOID-2.0.apk`, with a SHA-256 file beside it. It targets physical ARM64 Android phones running Android 7 or later. These artifact checks do not replace a phone registration test.

The connection-reporting update is version 1.0.1 (code 2), also available as `build/VOID-2.0-connection-check.apk`. Its certificate matches the first APK, so install it as an update over the existing app to retain its protected identity. Its package, signature, alignment, configured API URL and bundled error codes were verified. A read-only deployment check returned the expected API errors for GET and an invalid registration payload; no account or valid challenge was created. The reported phone connection failure still needs its new in-app error code to identify the cause.

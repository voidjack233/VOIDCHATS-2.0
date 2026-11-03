# Account initialization

The Android app connects its startup account screen to the signed registration API on the [backend branch](https://github.com/voidjack233/VOIDCHATS-2.0/tree/backend). Registration creates an account association for this device's existing protected signing identity. Registration itself is separate from the authenticated WebSocket messaging connection.

## Run and configure

Build and install the custom native Android app using [the native setup instructions](../native/openmls/README.md). The registration bridge is available in debug and release builds. Rebuilding is required for the new Rust, generated UniFFI, Kotlin, and storage methods; refreshing JavaScript in an older app is insufficient. Expo Go, web, and the unfinished iOS native integration cannot initialize accounts. Debug **Preview VOID** still opens the UI without registering.

The public API base is configured in [src/config/api.ts](../src/config/api.ts):

```text
https://api.voidchats.xyz/dm9pZG5vdGhpbmduZXNz
```

The client requires HTTPS and sends JSON to `/challenges` and `/accounts` under that base, with `User-Agent: VOIDCHATS/2.0`. It uses a bounded request timeout, rejects unexpected responses, and respects a returned `Retry-After` delay.

The Android client uses text XMLHttpRequest transport so the native connection failure can be classified before React Native's fetch wrapper reduces it to a generic network error. Setup distinguishes DNS, HTTPS certificate, timeout, refused/reset connection, and missing network-route errors. The error includes the public endpoint and any HTTP status, without displaying raw native exception text. The APK uses the API hostname and does not discover a running backend automatically; changing the configured base requires rebuilding. The main `voidchats.xyz` website has separate routing.

### Expo Go shows a native-build requirement

Scanning the Expo QR code opens Expo Go, whose installed binary does not contain VOID's storage or Rust signing modules. JavaScript refreshes cannot add these modules. The account screen explains this requirement before submission and keeps development preview available. Install the custom VOID Android app to initialize an account.

For a standalone installation, run `npm run android:apk` on a computer with the native build tools. Transfer the latest `build/apk/v<versionName>/VOID.apk` to your Android phone, open it, approve installation from that source if Android asks, and launch **VOID**. `build/README.md` and `build/apk/latest.json` identify the latest build. Rebuilds overwrite the same version and retain only the newest two version folders. The release variant embeds JavaScript and all required native modules, so it runs without Metro, Expo Go, a QR code, USB debugging, or an emulator. This testing APK uses the existing development signing key; it is not configured for store distribution.

For live development after installing the Android SDK/NDK, JDK 17, and Rust toolchain described in the native setup instructions, connect an Android device with USB debugging enabled. Start Metro with `npm start`. In a second terminal, verify `adb devices`, run `adb reverse tcp:8081 tcp:8081`, and use `npm run android` to build and install VOID. Open the installed **VoidChats** app instead of scanning the Expo Go QR code. `npm run android:build` continues to build the debug variant, which uses Metro.

## User flow

Complete the startup guidance and acknowledgments, then choose **Create account**. Setup registers the protected identity and public signing key already stored on this device. There is no username input or generated username.

The screen reports preparation, connection, identity confirmation, registration, and local saving. While a request is in progress, it disables repeated submission, back navigation, and development preview. Errors remain visible with an explicit retry action. Successful local completion opens the messenger, and Profile and Settings display the stored VOID-ID. A legacy username remains available for existing accounts; new profiles use the VOID-ID. On subsequent launches, the app opens the protected store and resumes the registered account without calling registration again.

An existing unregistered identity from development storage setup is reused. It remains in startup on later launches until the user completes account setup or explicitly opens development preview.

## Registration protocol and local persistence

1. `prepareAccountRegistration(apiUrl)` creates an encrypted store and native MLS identity only when no store exists. It reuses existing identity material and records the API URL. An unfinished legacy registration intent is converted to username-free registration; a registered account is preserved.
2. `POST /challenges` sends only `{auth_public_key}` using the identity's public Ed25519 key. The response contains `{challenge_id, message, expires_at}`.
3. The client validates the challenge against the expected ID, expiry, public key, reserved empty line, and nonce. The native bridge independently validates the constrained registration message before Rust signs it. Private keys and serialized MLS state stay native.
4. `POST /accounts` sends only `{challenge_id, signature}`. A successful response contains `{void_id, created_at}`. The VOID-ID is validated and retained as a string, preserving its full integer precision.
5. `completeAccountRegistration` checks the response against the pending API URL and signing key and requires a signed proof, then atomically saves the public account association in SQLCipher. The storage snapshot updates only after that commit.

The deployed signed message retains the domain `VOID-NOTHINGNESS:register:v1`, followed by newline-separated challenge ID, RFC 3339 expiry, padded public-key base64, an empty reserved line, nonce, and a final newline. A nonempty value in the reserved line is rejected. This matches backend commit `219847aa`. The bridge exposes a registration-specific signer rather than an arbitrary signing API.

Migration `002_account_registration.sql` adds public registration metadata and prevents replacing an existing non-null account binding. Its checksum and SQL remain unchanged. Migration `004_optional_account_username.sql` rebuilds only the public registration table inside the existing migration transaction, making usernames nullable while copying every existing metadata value. Identity rows, account bindings, native checkpoints, private keys, encrypted history, and the wrapped database key are unchanged. The immutable account-binding trigger remains attached to `local_identity`. Missing, incompatible, or damaged existing storage fails closed; setup does not silently regenerate an identity.

## Retry and recovery boundaries

The client does not automatically repeat account POSTs. A challenge can expire or be consumed even when a submission fails, so another setup attempt requests a fresh challenge using the same persisted signing identity. Network failures, rejected proof, rate limits, and server capacity errors do not create replacement keys.

If the app receives a successful account response but local saving fails, it retains that response in memory. **Retry account setup** retries the local commit before contacting the backend again. This pending response survives screen changes within the provider's lifetime, but not application process termination.

If the server creates an account and its response is lost, or the process exits before a received response is saved, a later attempt may return `key_registered`. The backend currently exposes no account lookup or recovery endpoint, so the app preserves local data and reports that recovery is unavailable. It cannot reconstruct an unknown VOID-ID, assume registration succeeded, or replace the device identity to hide the failure. Removing local data does not delete the server account and loses the protected identity kept by this installation.

Account initialization does not implement remote account deletion, key rotation, multi-device enrollment, or account recovery. Authenticated discovery and messaging use the separate WebSocket flow after the local registration is ready.

## Verification

For username-free registration, the account/storage host suite passes 62 tests, including signed registration framing with an empty reserved line, rejection of nonempty username metadata, migration preservation of existing account/history/checkpoint bytes, nullable new account names, and refresh isolation after committed messaging operations. The Android suite now includes a real version-2 SQLCipher migration/reopen fixture preserving the wrapped database key and native checkpoint. Device execution remains part of the owner's phase-5 checks.

Useful host checks are:

```sh
npm run typecheck
npm run lint
node --test src/account/account.test.mjs src/storage/storage.test.mjs
```

The account host tests exercise protocol validation, request handling, and retry coordination with controlled responses; they do not create a production account or prove the Android bridge works on a device. Native verification additionally requires `npm run crypto:test`, `npm run crypto:bridgecheck`, Android build/tests, and a device check of first setup, failed setup, local-save retry, and restart restoration.

Earlier version **1.0.1** predates WebSocket messaging and does not contain this
implementation. Use the current `build/apk/v1.0.2/VOID.apk` and
[phase 5 checklist](two-device-messaging-test.md). Install it
over the existing app with the same signing certificate to preserve its identity.
Current check results are recorded in [messaging verification](messaging-verification.md);
older evidence remains in the dated [OpenMLS verification record](openmls-verification.md).

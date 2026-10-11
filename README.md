# VOID-E2EE 2.0

Native React Native and TypeScript UI foundation for VOID. The app opens a first-launch startup flow and stops at the account-initialization boundary. The messenger shell uses empty states until account and messaging services are connected.

## Run

Install dependencies with `npm install`. OpenMLS requires a custom native Android development build; Expo Go cannot load the Rust module. Configure the Android SDK, NDK **27.1.12297006**, **JDK 17** (`JAVA_HOME`) and rustup, then follow [native setup and verification](native/openmls/README.md).

Run `npm start` for Metro and `npm run android` to build and install on a connected physical Android phone with USB debugging enabled. The Android command uses that Metro server instead of launching another with the Expo preview configuration. Use `adb reverse tcp:8081 tcp:8081` for Metro over USB. `npm run android:build` builds an APK without requiring an emulator or phone. `npm run crypto:test` tests Rust; `npm run crypto:bridgecheck` tests generated Kotlin bindings against the actual host Rust library. In the phone's React Native DevTools console, `await globalThis.voidMlsSelfTest()` runs the native bridge diagnostic.

The existing Expo setup remains available for UI preview with `npx expo start` and Expo Go. Cryptographic calls in Expo Go fail with an explicit unavailable error. The Android custom build uses `native-entry.js` and `metro.native.config.js`, preserving the same startup UI and theme. Expo's entry and Metro configuration remain available for Expo Go.

On macOS, install the iOS native dependencies with `bundle install` and `cd ios && bundle exec pod install`, then run `npm run ios` from the project root.

Use `npm run typecheck` for TypeScript validation and `npm run lint` for linting.

## Source layout

```text
src/
  app/             app bootstrap, providers, navigation composition
  startup/         welcome, identity guidance, acknowledgments, initialization boundary
  onboarding/      future entry flow UI
  conversations/   conversation list UI and reusable display model
  chat/            conversation detail UI
  crypto/          typed native OpenMLS interface, independent of screens
  requests/        message requests UI
  contacts/        contacts and VOID-ID discovery UI
  profile/         profile UI
  settings/        settings UI
  ui/              shared interface primitives
  theme/           semantic dark theme and typography/spacing tokens
```

Startup progresses through Welcome, How VOID Works, Protect Your Identity, Terms of Service, and a pending initialization screen. Security acknowledgment and acceptance of the development Terms copy are required to continue. State stays in memory; relaunching starts at Welcome. The initialization screen offers a development-only **Preview VOID** action that opens the messenger without creating an account.

The frontend displays empty states until account and messaging services are connected. Message composition and VOID-ID lookup remain disabled. Startup does not generate VOID-IDs, cryptographic keys, or account data, and does not call the backend.

## Native MLS

`native/openmls/` implements official OpenMLS **0.9.1** with RustCrypto and the RFC 9420 X25519/AES-128-GCM/SHA-256/Ed25519 ciphersuite. UniFFI generates the Android Kotlin bindings; private keys and MLS state remain in Rust. The typed `src/crypto/` interface is not connected to messaging screens. Alice/Bob diagnostics use real OpenMLS operations and do not create application accounts.

State serialization/restoration currently uses single-use native memory snapshots. Durable secure storage and identity binding to future VOID-ID registration remain future work. See [security findings and boundaries](docs/openmls-security.md); the dependency audit does not make VOID audited. There is no new messaging transport, account registration flow, or backend cryptographic processing.

See [verification results and remaining device checks](docs/openmls-verification.md) for the Rust and native binding test results and platform limitations.

## Backend

The Go backend provides a health endpoint and an in-memory account creation/retrieval flow using client-generated VOID-IDs and identity public keys. See [backend/README.md](backend/README.md) for setup, API examples, checks, and identity boundaries.

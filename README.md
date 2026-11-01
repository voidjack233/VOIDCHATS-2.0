# VOID-E2EE 2.0

Native React Native and TypeScript Android application for VOID, with encrypted local records and protected native OpenMLS persistence. A native Go account-registration backend is available in `backend/`; mobile registration integration and network message delivery remain unfinished.

## Run

Install dependencies with `npm install`. OpenMLS requires a custom native Android development build; Expo Go cannot load the Rust module. Configure the Android SDK, NDK **27.1.12297006**, **JDK 17** (`JAVA_HOME`) and rustup, then follow [native setup and verification](native/openmls/README.md).

Run `npm start` for Metro and `npm run android` to build and install on a connected physical Android phone with USB debugging enabled. The Android command uses that Metro server instead of launching another with the Expo preview configuration. Use `adb reverse tcp:8081 tcp:8081` for Metro over USB. `npm run android:build` builds an APK without requiring an emulator or phone. `npm run crypto:test` tests Rust; `npm run crypto:bridgecheck` tests generated Kotlin bindings against the actual host Rust library. In the phone's React Native DevTools console, `await globalThis.voidMlsSelfTest()` runs the native bridge diagnostic.

The existing Expo setup remains available for UI preview with `npx expo start` and Expo Go. Cryptographic calls in Expo Go fail with an explicit unavailable error. The Android custom build uses `native-entry.js` and `metro.native.config.js`, preserving the same startup UI and theme. Expo's entry and Metro configuration remain available for Expo Go.

On macOS, install the iOS native dependencies with `bundle install` and `cd ios && bundle exec pod install`, then run `npm run ios` from the project root.

Use `npm run typecheck` for TypeScript validation and `npm run lint` for linting.

## Go registration backend

`backend/` contains the native Go account-initialization API, Ed25519 registration challenges, Snowflake VOID-IDs, PostgreSQL persistence, and health endpoints. It uses an existing PostgreSQL database and never provisions database objects or credentials automatically. Configure `backend/.env` with an owner-approved existing connection; real environment files and compiled binaries are excluded from Git.

From the repository root:

```bash
cd backend
go build ./cmd/void-server
./void-server
```

See [backend API, configuration, and verification](backend/README.md). The mobile application is not yet wired to this API.

## Source layout

```text
src/
  app/             app bootstrap, providers, navigation composition
  startup/         welcome, identity guidance, acknowledgments, initialization boundary
  onboarding/      future entry flow UI
  conversations/   conversation list UI and reusable display model
  chat/            conversation detail UI
  crypto/          typed native OpenMLS interface, independent of screens
  storage/         typed local repositories, storage lifecycle and data hooks
  requests/        message requests UI
  contacts/        contacts and VOID-ID discovery UI
  profile/         profile UI
  settings/        settings UI
  ui/              shared interface primitives
  theme/           semantic dark theme and typography/spacing tokens
```

First-launch startup progresses through Welcome, How VOID Works, Protect Your Identity, Terms of Service, and the account-initialization boundary. The development-only **Preview VOID** action opens the messenger. In Settings, explicit local storage setup creates an unregistered device identity and an empty encrypted store. Subsequent debug launches restore that identity and local preview; missing or damaged state never silently creates a replacement.

Conversations, chat history, contacts, and requests read persisted local records and preserve empty states when none exist. Message composition and VOID-ID lookup remain disabled. Opening storage does not initialize an account or contact a remote service. See [local storage architecture, lifecycle and verification](docs/local-storage.md).

## Native MLS

`native/openmls/` implements official OpenMLS **0.9.1** with RustCrypto and the RFC 9420 X25519/AES-128-GCM/SHA-256/Ed25519 ciphersuite. UniFFI generates the Android Kotlin bindings; private keys and MLS state remain in Rust. The typed `src/crypto/` interface is not connected to messaging screens. Alice/Bob diagnostics use real OpenMLS operations and do not create application accounts.

The Android storage coordinator persists complete native MLS checkpoints and application records in SQLCipher, with its database key wrapped by Android Keystore. Checkpoints and crypto-event records commit before cryptographic results reach JavaScript. Identity binding to future VOID-ID registration, public authentication and messaging transport remain future work. See [local storage boundaries](docs/local-storage.md) and [security findings](docs/openmls-security.md); the dependency audit does not make VOID audited.

See [verification results and remaining device checks](docs/openmls-verification.md) for the Rust and native binding test results and platform limitations.

## Current scope

This repository contains the mobile application, native OpenMLS integration, encrypted on-device storage, and the Go account-initialization backend in `backend/`.

The backend implements account registration. Mobile registration integration, remote key exchange, and network message transport remain unfinished. UI preview and local storage remain independent of a server.


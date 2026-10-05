# VOID-E2EE 2.0

Native React Native and TypeScript UI foundation for VOID. The app opens a first-launch startup flow and stops at the account-initialization boundary. The messenger shell uses empty states until account and messaging services are connected.

## Run

Install dependencies with `npm install`, then run `npx expo start` and open the project in Expo Go using the QR code.

To run the existing native Android project, configure the Android SDK and a JDK, then use `npm run android`.

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
  requests/        message requests UI
  contacts/        contacts and VOID-ID discovery UI
  profile/         profile UI
  settings/        settings UI
  ui/              shared interface primitives
  theme/           semantic dark theme and typography/spacing tokens
```

Startup progresses through Welcome, How VOID Works, Protect Your Identity, Terms of Service, and a pending initialization screen. Security acknowledgment and acceptance of the development Terms copy are required to continue. State stays in memory; relaunching starts at Welcome. The initialization screen offers a development-only **Preview VOID** action that opens the messenger without creating an account.

The frontend displays empty states until account and messaging services are connected. Message composition and VOID-ID lookup remain disabled. Startup does not generate VOID-IDs, cryptographic keys, or account data, and does not call the backend.

## Backend

The Go backend provides a health endpoint and an in-memory account creation/retrieval flow using client-generated VOID-IDs and identity public keys. See [backend/README.md](backend/README.md) for setup, API examples, checks, and identity boundaries.

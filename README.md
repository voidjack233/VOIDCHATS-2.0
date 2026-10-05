# VOID-E2EE 2.0

Native React Native and TypeScript UI foundation for VOID. The app currently opens a dark messenger shell with empty states until account and messaging services are connected.

## Run

Install dependencies with `npm install`, then run `npx expo start` and open the project in Expo Go using the QR code.

To run the existing native Android project, configure the Android SDK and a JDK, then use `npm run android`.

On macOS, install the iOS native dependencies with `bundle install` and `cd ios && bundle exec pod install`, then run `npm run ios` from the project root.

Use `npm run typecheck` for TypeScript validation and `npm run lint` for linting.

## Source layout

```text
src/
  app/             app bootstrap, providers, navigation composition
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

The frontend displays empty states until account and messaging services are connected. Message composition and VOID-ID lookup remain disabled. No networking, message delivery, identity verification, or E2EE behavior is implemented in the frontend foundation.

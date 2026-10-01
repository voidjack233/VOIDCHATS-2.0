# VOID-E2EE 2.0

Native React Native and TypeScript UI foundation for VOID. The app currently opens a dark messenger shell with sample conversations so the interface can be developed before the client and encryption core are integrated.

## Run

Install dependencies with `npm install`, then start Metro with `npm start`. In another terminal, run `npm run android` with the Android SDK and a JDK configured.

On macOS, install the iOS native dependencies with `bundle install` and `cd ios && bundle exec pod install`, then run `npm run ios` from the project root.

Use `npm run tsc` for TypeScript validation and `npm run lint` for linting.

## Source layout

```text
src/
  app/             app bootstrap, providers, navigation composition
  onboarding/      future entry flow UI
  conversations/   conversation list and sample conversation data
  chat/            conversation detail UI and sample messages
  requests/        message requests UI and sample requests
  contacts/        contacts and VOID-ID discovery UI
  profile/         profile UI
  settings/        settings UI
  ui/              shared interface primitives
  theme/           semantic dark theme and typography/spacing tokens
```

Sample content is kept in `mock*.ts` files next to its domain UI. No networking, message delivery, identity verification, or E2EE behavior is implemented in this foundation.

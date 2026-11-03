# Messaging verification — 2026-11-03

The current direct-messaging update targets version **1.0.3**, code **4**. It
removes conversation approval and visible key-setup steps, opens the composer
immediately and advances native setup through durable opaque events. The
current update's checks are pending below; earlier results are historical.

Phase 5 remains the owner's two-device Android test. No backend account records,
database schema or deployment settings are changed by this frontend/native work.

## Current update checks

| Check | Status |
| --- | --- |
| TypeScript and ESLint | Pending |
| Account/storage and messaging host suites | Pending |
| Rust tests, formatting and Clippy | Pending |
| Kotlin/Rust host bridge | Pending |
| Android main and instrumentation compilation | Pending |
| Version 1.0.3 APK assembly, signature, contents and alignment | Pending |
| Two actual Android accounts, including alternating connections | Owner-run; not performed |

The direct-message design saves outgoing text in SQLCipher immediately. It
marks application messages sent only after their encrypted event is accepted.
First incoming messages appear in Requests until the local user replies; opening
the request does not require accepting an invitation or marking identity trusted.

Users do not need simultaneous presence. Current backend limits still require
the sender to reconnect after the recipient first supplies its KeyPackage before
the first application text can be encrypted. See the
[bootstrap boundary](offline-first-bootstrap.md) and
[current test checklist](two-device-messaging-test.md).

After a successful build, `build/README.md` and `build/apk/latest.json` point to
`build/apk/v1.0.3/VOID.apk`. They continue pointing to the last successful artifact
until publication completes. Same-version rebuilds overwrite; only two version
folders are retained. Do not use the historical checksum below for a newer APK.

## Historical version 1.0.2 checks

| Check | Result | Coverage |
| --- | --- | --- |
| TypeScript and ESLint | Passed | Native bridge, socket coordinator and screens |
| Account/storage host suite | 62 passed | Username-free proof, full schema upgrade preserving account/history/checkpoint bytes, lifecycle isolation |
| Messaging host suite | 22 passed | Actual synthetic Ed25519 auth proof, challenge rejection before signing, command serialization, reconnect isolation, native-before-ACK ordering, exact-ciphertext retry, uncertain acceptance |
| Rust suite | 24 passed; one child-worker helper intentionally ignored by the harness | Real OpenMLS exchange, account-key binding, checked Welcome, membership/commit guards, authenticated application envelopes, durable restart/ratchet recovery |
| Kotlin/Rust host bridge | 3 passed | Actual generated UniFFI/JNA calls, including all new direct-conversation APIs and v2 signing |
| Rust formatting and Clippy | Passed | `cargo fmt --check`, all targets with warnings denied |
| Android main and instrumentation Kotlin compilation | Passed | Native modules, SQLCipher integration and device fixtures compile |
| Standalone Android APK | Passed | Release assembly, v2 signature, 16 KiB APK alignment, packaged JS/native libraries/all four schema assets |

Host test command:

```powershell
node --test src/account/account.test.mjs src/storage/storage.test.mjs src/messaging/messaging.test.mjs
```

For version 1.0.2, the Android instrumentation suite was compiled, **not run**. Its isolated
synthetic fixtures cover real SQLCipher schema-2 upgrades, unchanged wrapped
database keys and native checkpoints, complete online setup, existing contacts,
both-side restart, exact handshake replay after an initiator-only restart,
simultaneous invitations, malformed relays, native auth binding and interruption
after decryption but before visible history insertion. Running these tests is
separate from proving connectivity between the two actual accounts.

## Historical version 1.0.2 artifact

Previously verified artifact: `build/apk/v1.0.2/VOID.apk`, version **1.0.2** (code **3**),
ARM64, Android 7 or newer; **24,360,704 bytes**. It bundles JavaScript and needs no
Metro server. Its development signing certificate matches the existing
prior version **1.0.1**, allowing an in-place update.

SHA-256:

```text
48cdde49785479c7fadc1550e1b70e495f40d4e8dd207ea37909fd5c252f7133
```

The checksum is saved beside the APK as `VOID.apk.sha256`; `build.json` records
its version and build metadata. `build/README.md` and `build/apk/latest.json`
point to the latest artifact without storing another APK copy. Rebuilding a
version replaces its files, and only the newest two version folders are kept.

Use the [phase 5 checklist](two-device-messaging-test.md) for in-place installation,
automatic first-message setup, messages in both directions, restarts and queued
delivery after the recipient reconnects. Expo Go cannot run these native components.

The backend's duplicate-event error does not prove matching payload acceptance.
If acceptance was lost, the app preserves the original ciphertext and displays
unknown send status. It does not convert that error to sent or delivered, and
does not automatically mint a replacement message. Delivery receipts remain
deferred. See the [verified backend contract](messaging-backend-handoff.md).

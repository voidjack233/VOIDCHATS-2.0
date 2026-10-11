# OpenMLS integration verification

Validation completed on **2026-10-11** in the Windows checkout of `VOIDCHATS-2.0`. Dependency/security review was performed on 2026-10-10; see [security findings](openmls-security.md).

## Completed checks

| Check | Result | What it verifies |
| --- | --- | --- |
| `npm run crypto:test` | 7 Rust tests passed | Actual independent OpenMLS clients, authenticated encrypted exchange, tamper rejection, commit transitions, and internal serialization/restoration |
| `cargo fmt --check` | Passed | Rust formatting |
| `cargo clippy --locked --all-targets -- -D warnings` | Passed | Rust wrapper and test static checks |
| `npm run typecheck` | Passed | Typed frontend interface and existing application |
| `npm run lint` | Passed | Existing frontend and native development entry/configuration |
| React Native Android JS bundle using `metro.native.config.js` and `native-entry.js` | Passed | Native entry resolves and bundles the unchanged application and lazy diagnostic interface |
| `npm run crypto:bridgecheck` with JDK 17 | 2 Kotlin/JVM tests passed | Generated UniFFI Kotlin/JNA bindings call the actual host Rust library, including separate client/key/group/message operations |
| `npm run android:build` with JDK 17 | Debug APK assembled successfully | Android adapter/generated bindings compile and native libraries are packaged for arm64 |

The Alice/Bob Rust diagnostic checks both message directions and compares authenticated sender credentials and signature public keys. It rejects modified ciphertext, processes a self-update commit to epoch 2, restores a suspended client, and exchanges a message in that epoch. Other Rust tests cover replay rejection across restoration, consumed Welcome/KeyPackage behavior, pending commits, a fresh engine/provider loaded from internal serialized bytes, preserved sending generations, and malformed/trailing/wrong-kind/wrong-group inputs.

The Kotlin tests independently create two Rust clients through generated bindings and exchange authenticated messages in both directions. A second test runs the Rust diagnostic across the same FFI boundary. JUnit reports **2 tests, 0 skipped, 0 failures, 0 errors**, with no stdout/stderr content. This is a host test using the Rust DLL, not a phone test.

## Android APK

`android/app/build/outputs/apk/debug/app-debug.apk` was rebuilt successfully with JDK 17 on 2026-10-11. It is **41,080,375 bytes**, with SHA-256 `1c6dd2eb980c43a2ecea441f48f927964ef5ae1c6b7701b58fe2e4cd0e86fe8c`. The APK contains:

- `lib/arm64-v8a/libvoid_openmls.so`: **2,543,192 bytes**, matching the current AGP-stripped Rust artifact; SHA-256 `73310d6d2b6f97f0234d302b95aaee6d15cb0bb146f56584e2bec2853a45f6b6`.
- `lib/arm64-v8a/libjnidispatch.so`: **165,992 bytes**, the pinned JNA native library.

Both libraries' ELF load segments have **16 KiB alignment (`0x4000`)**. The rebuilt APK passed `zipalign -c -P 16 4`. APK metadata confirms minimum SDK **24**, compile/target SDK **36**, and **arm64-v8a**. Build warnings concerned SDK XML tooling versions, copied files in place of cross-drive hard links, and the already stripped JNA library; assembly completed with exit code 0. These packaging checks do not establish runtime behavior on a phone.

## Build configuration

The custom Android build uses Rust **1.99.0**, OpenMLS **0.9.1**, UniFFI **0.32.2**, JNA **5.19.1**, JDK **17**, Gradle **8.14.5**, SDK/build tools **36**, NDK **27.1.12297006**, and default ABI **arm64-v8a**. Build scripts handle the Windows GNU host `dlltool` requirement using the installed NDK tooling; generated binaries/bindings remain in ignored build directories.

The prior Gradle 9.4.1/Kotlin plugin combination, SDK 37 path, JDK 24 Prefab tooling, and mismatched React Native 0.87.1 development configuration packages blocked validation. The native project now uses the compatible toolchain above. React Native remains at its existing resolved **0.86.3**, with matching exact development configuration versions. Expo's entry/configuration remain available for Expo Go preview; the custom native build has its own entry and Metro configuration.

## Remaining verification and implementation

`adb devices` reported no attached Android phone. The Android Rust library has not been executed on a phone, and the complete JavaScript → React Native Promise adapter → Kotlin → Rust path has not been verified on a device. Connect a physical phone, follow [native development instructions](../native/openmls/README.md), and run `await globalThis.voidMlsSelfTest()` in React Native DevTools. No emulator is required and no phone success is claimed.

iOS bindings/runtime are not implemented or tested. Only portable Rust code and future Swift binding configuration exist.

State restoration is native memory only. Process termination loses all clients/snapshots. Durable encrypted storage, atomic persistence/rollback protection, and authenticated identity binding to future VOID-ID registration remain required before production messaging. The dependency audit excludes VOID's bridge, storage and application. Existing npm advisory findings and the unsuccessful `cargo-audit` installation are documented separately; successful build/tests do not resolve them.

Startup, theme, frontend screens, existing Expo entry/configuration, and the Go backend were not changed. No messaging transport, account initialization, or production messaging connection was added. No changes were pushed or merged.

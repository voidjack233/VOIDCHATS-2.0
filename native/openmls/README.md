# VOID native OpenMLS engine

This independent Rust module uses official OpenMLS **0.9.1**, RustCrypto provider **0.6.0**, and RFC 9420 suite `MLS_128_DHKEMX25519_AES128GCM_SHA256_Ed25519`. Direct dependencies are exact pins; `Cargo.lock` pins the complete graph. Rust is pinned to **1.99.0**, UniFFI to **0.32.2**, Android JNA to **5.19.1**. No experimental MLS extensions, debug secret logging, or custom MLS algorithms are enabled. See [security review](../../docs/openmls-security.md) for advisories and audit scope.

## Structure

- `src/engine.rs`: isolated providers, credentials/signing keys, KeyPackages, groups, Welcome, messages, commits, lifecycle and limits.
- `src/persistence.rs`, `src/storage.rs`: complete versioned engine/provider serialization, restoration and the trusted native storage callback.
- `src/self_test.rs`, `src/tests.rs`, `src/durable_tests.rs`: independent Alice/Bob fixtures, rejection/state/restoration tests and actual child-process restart/failure tests.
- `uniffi.toml`: generated Kotlin configuration and future Swift configuration. iOS is not implemented or verified.
- `../../src/crypto`: typed Promise interface, public binary artifacts transported as canonical base64, opaque client/snapshot handles, u64 epochs transported as decimal strings.
- `../../android/app/src/main/java/com/voidchats/crypto`: React Native adapter to generated UniFFI Kotlin, with serialized work off the JS thread.

Normal startup opens existing protected state without creating an identity. Explicit debug-only local storage setup in Settings creates the first local device identity; it does not register an account. Credentials are caller-supplied bytes; BasicCredentials authenticate possession of a signing key and do not verify a VOID account. Future registration must bind these public keys to account identity. A public VOID-ID is never key material.

## Local validation

Install rustup and the pinned toolchain. For Android install SDK platform/build tools **36**, NDK **27.1.12297006**, and **JDK 17**; configure `ANDROID_HOME` and `JAVA_HOME`, and run:

```sh
rustup target add --toolchain 1.99.0 aarch64-linux-android
npm run crypto:test
npm run crypto:bridgecheck
npm run android:build
npm run typecheck
npm run lint
```

`crypto:test` runs locked Rust tests first. `crypto:bridgecheck` builds a host Rust library and runs actual generated Kotlin/JNA operations through Android host unit tests. It requires the Android build tools and tests the FFI ABI separately from a physical phone. Gradle generates matching bindings and builds the Android `.so` into ignored build directories. The default ABI is `arm64-v8a`; other installed targets can be selected with `npm run native:android -- arm64-v8a,armeabi-v7a,x86_64` and the matching Gradle `reactNativeArchitectures` property.

The debug APK is a custom React Native development build. Expo Go remains available for UI preview, but cannot load this native engine. Start Metro with `npm start`; connect an Android device or emulator, verify `adb devices`, run `adb reverse tcp:8081 tcp:8081`, then `npm run android` to install using that Metro server. In React Native DevTools, run `await globalThis.voidMlsSelfTest()` to exercise the actual React Native Promise adapter and obtain the five boolean checks. This lazy hook and the isolated native self-test are available only in development builds. No diagnostic users are inserted into the application database or UI.

## State and persistence boundary

`addMember` and `selfUpdate` produce a pending local commit. Call `mergePendingCommit` explicitly after a future delivery layer accepts that commit. Peers call `processCommit`; new members call `processWelcome`. Encryption while a local commit is pending fails. The fixed ciphersuite uses private handshake framing and the standard ratchet-tree extension for self-contained Welcome processing.

Message processing can advance receive-ratchet state even when ciphertext authentication fails. The tamper test rejects a modified message and confirms that a fresh subsequent message works; retrying that consumed generation is not promised. Error handling must not restore older ratchets as a workaround.

`suspendClient` serializes all private provider records and group IDs **inside Rust**, destroys the active client, and returns a random snapshot handle. `restoreClient` consumes that handle once, creates a new provider, restores the signing key and groups, and returns only public identity information. `discardSnapshot` and `releaseClient` release their native state. Serialized buffers and storage copies are wiped on release where the involved types permit it; no complete process-memory erasure guarantee is made.

`MlsEngine::new()` retains the isolated memory-engine behavior for diagnostics. `MlsEngine::open(storage, initialize_if_missing)` uses a native storage callback to restore the complete versioned engine checkpoint, stable client handles, provider records, group state, signing identities, pending commits and replay/ratchet state. It checkpoints every mutation, including protocol errors, with compare-and-swap revision advancement. The Android callback writes inside the protected SQLCipher transaction; native code commits before returning any cryptographic result to JavaScript. Secret checkpoint bytes pass only between Rust and trusted native storage, never through the React Native module.

Missing/incompatible/corrupt checkpoint data and storage failures fail closed. Cached receiving ratchets are flushed through OpenMLS's supported unchanged-policy setter before checkpointing because rejection paths can mutate secrets before the ordinary provider write. Host tests use authenticated encrypted fixtures and real child-process restarts; Android tests separately verify SQLCipher/Keystore and native transaction boundaries. See [local storage architecture](../../docs/local-storage.md) for the actual protection mechanism and remaining limits. Revision checks do not detect external rollback of a complete older encrypted store. Hardware security, account ownership binding, transport-ID ingestion/retention and production signing require further work. This wrapper/application is not covered by the dependency's audit.

See [local storage verification](../../docs/local-storage.md#build-and-checks) for current persistence checks and [the earlier integration report](../../docs/openmls-verification.md) for the original bridge validation.

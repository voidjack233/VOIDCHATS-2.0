# OpenMLS frontend interface

`openMls` is a typed, lazy facade for the Android `VoidOpenMls` native module. Cryptographic operations execute in the Rust engine through UniFFI. Importing the interface does not initialize an account, create keys, or change the startup UI.

Call `getOpenMlsAvailability()` to inspect native module registration, then `await openMls.initialize()` to verify that the native Rust library loads. A custom Android development build is required. Expo Go, iOS, and web cannot execute this integration; unavailable platforms reject operations explicitly. There is no JavaScript encryption fallback.

The interface carries public credentials, signature public keys, KeyPackages, Welcome messages, MLS commits, ciphertext, and application plaintext as standard base64 strings. Base64 is transport encoding, not encryption. MLS epochs use decimal strings so the native u64 value is preserved exactly. Native errors reject the operation; this interface does not log message content or key material.

`createClient(credentialIdentityBase64)` requires explicit application identity bytes. An MLS BasicCredential and its signing key authenticate a member within the MLS protocol, but do not prove ownership of a VOID account. The future account registration and verification flow must bind that public signing key to the registered identity. A public VOID-ID is not a cryptographic key.

`addMember()` and `selfUpdate()` stage commits. The sender must call `mergePendingCommit()` before using the new epoch, and other existing members must receive and process the returned commit. A new member joins with `processWelcome()`. Message transport and membership authorization are not implemented here.

Client handles refer to Rust clients owned by the native module. `suspendClient()` removes the active client and returns an opaque snapshot handle; secret snapshot bytes remain in native memory. `restoreClient()` consumes that handle, and `discardSnapshot()` removes it. `releaseClient()` drops the active native client. These handles are valid only for the current native module lifetime and must never be treated as account identifiers.

Durable persistence is unavailable (`durablePersistence: false`). Native module destruction, application termination, and process restart lose the in-memory clients and snapshots. Do not store secret MLS state in AsyncStorage or route it through JavaScript. Secure persistence requires a separately reviewed native storage design, encryption keys protected by the platform, and rollback protection for MLS state.

In a development build, `await openMls.runSelfTest()` runs isolated Alice/Bob Rust clients through the native bridge and returns a report containing no secrets. This diagnostic is not connected to the messenger screens and does not create frontend sample accounts. A successful desktop Rust test does not verify an Android phone or the React Native runtime.

See [Rust integration and development builds](../../native/openmls/README.md) and [security boundaries](../../docs/openmls-security.md).

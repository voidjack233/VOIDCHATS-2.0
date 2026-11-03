# OpenMLS frontend interface

This OpenMLS facade is dormant in VOID 2.0. The application does not register or package `VoidOpenMls`; direct messaging uses official libsignal. The source and the historical integration notes below are retained for future group work. See [current direct messaging](../../docs/signal-direct-messaging.md).

`openMls` is a typed, lazy facade for the former Android `VoidOpenMls` native module. Cryptographic operations in that integration execute in Rust through UniFFI. Importing the interface does not initialize an account or create keys.

Call `getOpenMlsAvailability()` to inspect native module registration. After opening protected local storage, `await openMls.initialize()` reports the active native engine, including `durablePersistence`. A custom Android development build is required. Expo Go, iOS, and web cannot execute this integration; unavailable platforms reject operations explicitly. There is no JavaScript encryption fallback.

The interface carries public credentials, signature public keys, KeyPackages, Welcome messages, MLS commits, ciphertext, and application plaintext as standard base64 strings. Base64 is transport encoding, not encryption. MLS epochs use decimal strings so the native u64 value is preserved exactly. Native errors reject the operation; this interface does not log message content or key material.

The primary client is created only by explicit native storage initialization and restored with its original public signing key and stable handle. `listClients()` returns public restoration metadata. The legacy React Native `createClient()` method rejects direct identity replacement; account initialization uses the reviewed storage lifecycle. An MLS BasicCredential authenticates a member within MLS but does not alone prove ownership of a VOID account. The native messaging coordinator validates the KeyPackage signature key and joined two-member group against the peer's registered account key and pins the peer credential. This binding leaves identity trust unverified.

`addMember()` and `selfUpdate()` stage commits. The sender must call `mergePendingCommit()` before using the new epoch, and other existing members must receive and process the returned commit. A new member joins with `processWelcome()`. The messaging bridge uses checked native operations for direct groups and carries initial public protocol objects over the authenticated online relay. It enables application messages only after device-level MLS setup completes.

Client handles refer to protected native clients and survive reopening the same store. They are not account identifiers. Legacy React Native suspend/restore/release/discard methods reject operations on the protected local identity; closing and deleting local storage use explicit lifecycle actions. The Rust memory-engine diagnostics retain single-use snapshot methods internally.

The active Android engine persists complete native provider/group/identity state through SQLCipher and Android Keystore-backed protection. Mutations, including rejected-message ratchet changes, checkpoint before any successful Promise result. Missing or incompatible native state fails closed rather than creating a new identity. See [local storage](../../docs/local-storage.md) for commit ordering and the limits of whole-database rollback protection.

`readStoredDecryptions(before, limit)` reads bounded pages of previously committed, authenticated application data after a crash, without decrypting the ciphertext again or advancing a ratchet. It exposes application plaintext and authenticated public sender/epoch metadata, not private MLS snapshots or database keys. The native messaging coordinator binds transport IDs and encrypted application envelopes, recovers interrupted visible-message writes from its crypto journal and commits local processing before allowing recipient ACK. Deleting a visible message does not erase an independent native crypto-journal record.

In a development build, `await openMls.runSelfTest()` runs isolated Alice/Bob Rust clients through the native bridge and returns a report containing no secrets. This diagnostic is not connected to the messenger screens and does not create frontend sample accounts. A successful desktop Rust test does not verify an Android phone or the React Native runtime.

See [Rust integration and development builds](../../native/openmls/README.md) and [security boundaries](../../docs/openmls-security.md).

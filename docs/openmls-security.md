# OpenMLS dependency and security review

Reviewed on **2026-10-10**. This document covers the OpenMLS dependency selection and the boundaries required for the VOID native integration. It is not an audit of VOID, its bridge, its providers, or its storage design.

## Selected dependency versions

Use the official `openmls` crate **0.9.1**, with exact direct dependency pins and a checked-in Cargo lockfile. Version 0.9.1 was published to crates.io on 2026-10-07, is not yanked, and is tagged upstream as `v0.9.1` at commit `1762090a4523cdd241f454b4058e91b64bbdc6de`. The GitHub "latest release" page still pointed at 0.9.0 during this review; that page alone is insufficient for security version selection. Sources: [crate version metadata](https://crates.io/api/v1/crates/openmls/0.9.1), [upstream version manifest](https://github.com/openmls/openmls/blob/v0.9.1/openmls/Cargo.toml).

The compatible upstream supporting crates are:

| Dependency | Exact version | Purpose |
| --- | --- | --- |
| `openmls` | `=0.9.1` | RFC 9420 implementation |
| `openmls_rust_crypto` | `=0.6.0` | Official RustCrypto provider |
| `openmls_traits` | `=0.6.0` | Provider interfaces |
| `openmls_basic_credential` | `=0.6.0` | Basic credentials and signing keys |
| `openmls_memory_storage` | `=0.6.0` | In-memory OpenMLS state |

These are the versions named in the [v0.9.1 workspace manifest](https://github.com/openmls/openmls/blob/v0.9.1/Cargo.toml). OpenMLS requires Rust **1.91.0 or newer**. Exact pins prevent accidental direct version changes; `Cargo.lock` pins transitive dependencies. Update both deliberately after reviewing upstream changes and advisories.

The native bridge uses **UniFFI `=0.32.2`**, the maintained non-yanked patch published on 2026-09-23. Its patch fixes Kotlin compiler warnings in generated bindings and retains 0.32.1's Android aarch64 checksum fix. Generate bindings with the same pinned UniFFI version used by the Rust library, and keep runtime checksum checks enabled. Sources: [version metadata](https://crates.io/api/v1/crates/uniffi/0.32.2), [tagged changelog](https://github.com/mozilla/uniffi-rs/blob/v0.32.2/CHANGELOG.md).

## Provider and ciphersuite

Select the official `OpenMlsRustCrypto` provider and **`MLS_128_DHKEMX25519_AES128GCM_SHA256_Ed25519` (`0x0001`)**. The OpenMLS Book identifies RustCrypto as its default provider and lists this suite as supported. It is the mandatory MLS 1.0 suite in RFC 9420: X25519, AES-128-GCM, SHA-256/HKDF, and Ed25519. Sources: [OpenMLS provider documentation](https://book.openmls.tech/traits/traits.html), [RFC 9420 section 17.1](https://www.rfc-editor.org/rfc/rfc9420.html#section-17.1).

Keep all MLS draft features disabled, including `extensions-draft`, `virtual-clients-draft`, `targeted-messages-draft`, and `draft-ietf-mls-pq-ciphersuites`. Keep `crypto-debug`, `content-debug`, `unchecked-conversions`, and the dependency's test utilities disabled in application builds. Do not expose signing private keys or MLS secret exports through TypeScript.

The provider's published manifest internally enables HPKE's `experimental` primitive feature. This is an upstream dependency feature, not a VOID opt-in to draft MLS extensions. The wrapper still selects only the RFC 9420 suite above. Review transitive dependency features and advisories when upgrading. Source: [RustCrypto provider manifest](https://github.com/openmls/openmls/blob/v0.9.1/openmls_rust_crypto/Cargo.toml).

## Published security advisories

The following upstream OpenMLS advisories were reviewed. Version 0.9.1 is outside their published affected ranges as of the review date; this is not a guarantee that no other vulnerability exists.

| Advisory | Impact | Published fix |
| --- | --- | --- |
| [GHSA-gc79-23g3-8g52](https://github.com/openmls/openmls/security/advisories/GHSA-gc79-23g3-8g52) | Invalid nested extensions can cause unbounded recursion and abort the process, including through an FFI. Published 2026-10-08; 0.9.0 is affected. | 0.9.1 (also backported to 0.8.2) |
| [GHSA-w62v-gv48-63rh](https://github.com/openmls/openmls/security/advisories/GHSA-w62v-gv48-63rh) | Quadratic duplicate-extension checks consume CPU before authentication. | 0.9.0 |
| [GHSA-rrmv-c79f-cf5r](https://github.com/openmls/openmls/security/advisories/GHSA-rrmv-c79f-cf5r) | Malformed bytes can panic in manual byte-slice deserialization before authentication. | 0.9.0 |
| [GHSA-8x3w-qj7j-gqhf](https://github.com/openmls/openmls/security/advisories/GHSA-8x3w-qj7j-gqhf) | Truncated membership/confirmation tags can bypass secondary authentication under affected configurations. | 0.7.2 / 0.8.0 |
| [GHSA-qr9h-x63w-vqfm](https://github.com/openmls/openmls/security/advisories/GHSA-qr9h-x63w-vqfm) | Message processing did not persist secret-tree updates, affecting forward secrecy and restart behavior. | 0.7.1 |

The upstream RustCrypto provider uses the HPKE 0.7 family, beyond the 0.6.0 fixes described in [GHSA-g433-pq76-6cmf](https://github.com/celabshq/hpke-rs/security/advisories/GHSA-g433-pq76-6cmf). Earlier provider dependencies also carried libcrux advisory issues; upstream [issue 2126](https://github.com/openmls/openmls/issues/2126) is closed. Scan the resolved lockfile rather than inferring transitive security solely from the OpenMLS version.

### Resolved dependency scan

On 2026-10-10, a query to the [OSV batch API](https://google.github.io/osv.dev/post-v1-querybatch/) checked all **248 crates.io package/version pairs** in `native/openmls/Cargo.lock`, after the UniFFI 0.32.2 update. It returned these matches:

| Locked dependency | Advisory | Assessment for this build |
| --- | --- | --- |
| `libcrux-kem 0.0.9` | [RUSTSEC-2026-0330](https://rustsec.org/advisories/RUSTSEC-2026-0330.html), [RUSTSEC-2026-0331](https://rustsec.org/advisories/RUSTSEC-2026-0331.html) | Short hybrid keys or seeds can cause a panic; fixed in 0.0.10. The dependency is optional and absent from the selected build graph. |
| `proc-macro-error2 2.0.1` | [RUSTSEC-2026-0173](https://rustsec.org/advisories/RUSTSEC-2026-0173.html) | Informational notice that the crate is unmaintained; absent from the selected build graph. |

`cargo tree --locked --offline` confirmed neither package participates in the host dependency graph or the Android arm64 graph, including a check with the `bindgen` feature enabled. Cargo's lockfile retains optional dependencies even when those dependencies are not built. The active feature graph had no MLS draft, `crypto-debug`, `content-debug`, or dependency `test-utils` features. This reachability assessment must be repeated if provider selection or features change; it is not a blanket advisory waiver. The locked HPKE/libcrux optional provider pins `libcrux-kem` exactly to 0.0.9, so an ordinary `cargo update` cannot apply that package's fix.

No other matches were returned by this query. The scanned lockfile's SHA-256 was `ac44542745775b4fae7f8edbeee6154fe32168182517badf7ffe85a994ea0b50`. Recheck after any lockfile update. This database query does not establish that the compiled library or application is vulnerability-free, and the current OpenMLS advisories above were reviewed separately.

`cargo-audit 0.22.2` installation was attempted but its own `aws-lc-sys` dependency could not compile because the Windows GNU environment lacks a host GCC C compiler. No successful `cargo audit` result is claimed. The OSV check and upstream/RustSec advisory review provide the dependency triage recorded here; add a reproducible Cargo audit gate on a supported build host in the next phase.

### Existing npm dependency findings

On 2026-10-10, `npm audit --json --package-lock-only` reported **35 affected packages: 7 moderate, 27 high, and 1 critical**. The same command against isolated copies of `package.json` and `package-lock.json` from baseline commit `d910490c248035effb1729af7480572d46c73f74` returned the identical affected-package, severity, and advisory set. All findings predate this integration. No JavaScript dependencies were added: React Native was pinned to its existing resolved 0.86.3, and its four development configuration packages were aligned from 0.87.1 to 0.86.3 for native build compatibility.

The 35-package count includes dependent packages inheriting advisories from these four underlying dependencies. Their locked versions are unchanged from the baseline:

| Dependency | Advisory and effect | Current dependency path and fix status |
| --- | --- | --- |
| `shell-quote 1.10.0` | **Critical**, [GHSA-pqg4-j6r4-53mv](https://github.com/ljharb/shell-quote/security/advisories/GHSA-pqg4-j6r4-53mv): command injection when `quote()` receives a comment token followed by an attacker-controlled token with a line terminator, and its output is executed by a shell. | Included by `launch-editor` and `react-devtools-core`. Affects versions >=1.8.4 and <1.11.0; fixed in 1.11.0. |
| `braces 3.0.3` | **High**, [GHSA-vfj7-8cjw-p6xm](https://github.com/advisories/GHSA-vfj7-8cjw-p6xm): deeply nested patterns can exhaust the Node.js call stack. | Included through `micromatch` in Metro/CLI file processing. No patched version was listed in the reviewed advisory. |
| `node-forge 1.4.0` | **High**, [GHSA-86w9-cpqp-85rv](https://github.com/advisories/GHSA-86w9-cpqp-85rv): malformed nested RSA PKCS#1 v1.5 digest structures can pass signature verification with low-exponent keys. | Included by Expo CLI and `@expo/code-signing-certificates`. No patched version was listed in the reviewed advisory. |
| `uuid 7.0.3` | **Moderate**, [GHSA-w5hq-g745-h8pq](https://github.com/uuidjs/uuid/security/advisories/GHSA-w5hq-g745-h8pq): missing output-buffer bounds checks in affected v3/v5/v6 functions. | Included by `xcode` in Expo native configuration. Fixed releases include 11.1.1, 12.0.1, and 13.0.1; these require compatibility review rather than a blind major-version override. |

These are unresolved findings in the existing frontend/build dependency tree. They do not identify OpenMLS cryptographic vulnerabilities, and this review does not establish that every vulnerable function is reachable from VOID. In particular, the critical advisory requires the specific token and shell-execution conditions above; no end-to-end exploitability test was performed. Native integration checks passing do not resolve these dependency findings. Their targeted remediation remains separate work; no broad `npm audit fix` or unrelated dependency upgrade was applied.

## Audit scope and remaining responsibilities

The published [SRLabs OpenMLS assessment](https://blog.openmls.tech/SRL-OpenMLS_security_assurance_assessment.pdf), version 1.2 dated 2026-03-11, reviewed `openmls`, `traits`, and `basic_credential`. Crypto providers, storage providers, clients, and delivery services were outside its scope. It reports eight findings: one high, three medium, two low, and two informational. Its low finding about divergence between in-memory group state and storage after storage errors was acknowledged, and excessive allocations remained an accepted informational risk. A library audit does not cover VOID or subsequent library changes.

The maintainers' [May 2026 audit announcement](https://blog.phnx.im/openmls-independent-security-audit/) says seven findings had been addressed in 0.8.1/0.7.3 while a low finding remained under work. The advisory fixes published after this assessment also explain why the audit cannot replace ongoing dependency review.

## Native and identity boundaries

Cryptographic operations and private signing/MLS key material belong in Rust. TypeScript exchanges opaque client and snapshot handles, public group IDs, public credentials and signing keys, KeyPackages, protocol ciphertext, and authenticated application data. Returning plaintext to the UI is necessary for display and makes the UI/runtime part of the confidentiality boundary; plaintext cannot be assumed to disappear immediately from JavaScript or native managed memory.

The bridge must bound incoming sizes, reject malformed or trailing protocol bytes, serialize operations on each client, return controlled errors, and release native handles. FFI input validation, lifecycle management, byte conversion, native library loading, and crash behavior require their own review. Never log signing keys, secret group state, snapshots, or plaintext. Dependency debug features remain disabled even in a development build.

Basic credentials prove possession of a signing key in the MLS group; they do not independently prove ownership of a public VOID-ID or a person's real-world identity. Future account registration must coordinate the credential identity and public signature key with the VOID identity model, and contact verification must establish trust in that binding. A public VOID-ID is an identifier, never a cryptographic key. The account server may receive public identity material and encrypted delivery objects; private material remains on the client. Source: [RFC 9420 credential and authentication requirements](https://www.rfc-editor.org/rfc/rfc9420.html#section-5.3).

## Persistence boundary and next phase

In-memory state and restart harnesses establish the engine's behavior; they do not provide production secure persistence. Serialized OpenMLS state includes private keys, epoch secrets, secret trees, and consumed-message/key-package state. Keep it out of plaintext AsyncStorage, application logs, backend requests, and backups. If a development harness serializes it, retain the bytes inside Rust/native memory and do not expose a plaintext state-export API to JavaScript.

OpenMLS 0.9 requires self-describing storage codecs; use its existing JSON representation or an explicitly reviewed compatible codec. Non-self-describing codecs are unsupported in this release. Source: [OpenMLS 0.9 release notes](https://github.com/openmls/openmls/releases/tag/openmls-v0.9.0).

Before real account initialization or messaging is enabled, implement an encrypted native store whose encryption key is protected by Android Keystore, with an equivalent iOS Keychain design. Use established platform/library encryption instead of custom cryptography. Define authenticated format/version metadata, crash-safe writes, operation transactions, backup exclusion, key invalidation and logout handling, migration, and rollback protection. Treat storage failures as engine failures: do not continue with divergent memory/disk state. Persist sender ratchet changes before ciphertext becomes eligible for delivery, and receiver changes before accepting a message into durable application history.

Restart tests must preserve replay protection, consumed KeyPackage state, group epochs, and pending commits. Reusing an old snapshot can roll back ratchets and restore erased secrets; snapshot restoration therefore requires a protected freshness policy before production use. Keep past-epoch retention minimal and define a deletion policy alongside future offline-delivery requirements.

This phase stops before account registration, user discovery, message transport, and production messaging. Secure persistence and authenticated identity binding are prerequisites for that later phase.

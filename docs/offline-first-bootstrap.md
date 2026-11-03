# Offline first-message bootstrap: current boundary and follow-up proposal

This is a reviewable **proposal**, not a deployed backend API or an implemented
cryptographic replacement. The direct-messaging frontend/native update does not
change backend data or deployment settings.

## Current behavior

Entering a VOID-ID opens a composer immediately. The app saves the outgoing text
inside SQLCipher and automatically carries setup messages through the backend's
existing durable opaque queue. The recipient can open VOID later, generate its
MLS KeyPackage and queue a reply. The sender then reconnects to obtain that
package, create the group and encrypt the locally pending text. Remaining native
confirmation exchanges also proceed through the queue.

This needs no simultaneous online presence and no recipient approval. It does
require several connections. If Alice closes VOID after the first Send and never
reconnects after Bob first opens it, Bob cannot yet receive Alice's application
text. Server acceptance of a public setup envelope must not mark that text sent.
Established conversations already support encrypting and queuing messages while
the recipient is offline.

## What one-way offline first delivery needs

Before Alice sends to a never-seen offline Bob, Alice needs Bob's valid public
MLS KeyPackage. A registered Ed25519 signing public key is an identity binding;
it is not Bob's private MLS initialization state. MLS defines KeyPackages for
asynchronous addition and generally intends them for single use.
[RFC 9420 §10](https://www.rfc-editor.org/rfc/rfc9420.html#section-10)

Converting the long-term signing key to an encryption key would introduce a
different bootstrap protocol, new security properties and account-key reuse.
It is outside this change. Distinct signing and encryption keys are recommended
by the [official Libsodium guidance](https://libsodium.gitbook.io/doc/advanced/ed25519-curve25519).

## Proposed public KeyPackage publication and claim capability

A future backend extension could add these authenticated operations to the
current socket contract. These command names and fields are proposed only:

| Operation | Required behavior |
| --- | --- |
| Publish `{type: "key_packages_publish", packages_base64: [...]}` | Bind public KeyPackages to the authenticated account, bound batch/body sizes, store expiry and stable fingerprints, and reject republishing a consumed package as available. |
| Claim `{type: "key_package_claim", claim_id, to_void_id}` | Atomically reserve one unexpired package for this claimant and stable claim ID. Return the original result on identical retry, reject conflicting reuse, and never give the same package to a second claim. |
| Claim result | Include stable public package ID/bytes and recipient account key. Report no-package availability distinctly without implying the recipient is currently online. |

The app would generate packages natively while online, publish only public bytes,
and keep each corresponding private key inside its protected OpenMLS checkpoint.
Alice would verify the package's actual MLS signature and account-key binding,
create/add the group, then durably queue its Welcome and first application
ciphertext in a defined recipient order. Bob could process both after Alice has
gone offline. Recipient ACK would still follow local native/history commits.

No server conversation records, plaintext messages or private MLS material are
needed. Public package inventory would be a new backend responsibility and would
change the original no-inventory boundary; it needs a separately reviewed backend
implementation, additive schema/deployment plan and tests. Required tests include
concurrent claims, lost claim responses, package expiry/consumption, account-key
mismatch, stale publications and first delivery while the sender remains offline.

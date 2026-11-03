# Owner-run Android messaging verification

Run this after the current update passes its local checks. The owner performs
device testing, using only the two existing test accounts. No backend migration,
account deletion, database cleanup or deployment change is part of this test.

## Install the custom native build

Expo Go cannot execute native OpenMLS or SQLCipher. The direct-messaging update
targets **1.0.3**, code **4**. Check [current verification](messaging-verification.md)
before installing. After successful publication, its APK is
`build/apk/v1.0.3/VOID.apk`; `build/README.md` and `build/apk/latest.json` identify
the latest successful build. Rebuilding a version overwrites it, and only the
newest two version folders are kept.

To build from source:

```powershell
npm run android:apk
```

Update the existing VOID app in place with the same signing key to retain its
protected identity. Preserve the installed app and data if Android reports a
signature mismatch. The test APK bundles JavaScript and needs no Metro server.
Default builds target **ARM64**, Android 7 or newer. An x86_64 emulator needs a
matching native ABI build with Gradle's `-PreactNativeArchitectures=x86_64`.

For a Metro-based custom debug build, use `npm start` and `npm run android`.
Each USB device can reach Metro through
`adb -s <device-serial> reverse tcp:8081 tcp:8081`.

## Identity and immediate composition

1. Record both existing VOID-IDs before updating. Open the updated apps and
   confirm those IDs are unchanged, with no request to register again. Existing
   legacy names, history and protected keys must remain intact.
2. On Alice, enter Bob's exact VOID-ID. Lookup uses the authenticated socket and
   retrieves Bob's registered account key. It does not mark Bob trusted.
3. Open the chat and type immediately. There must be no acceptance gate, public
   key establishment screen or requirement to keep both apps open together.
4. Send distinct multiline and Unicode text, such as `Hello\n你好 🔒` with a real
   newline. Text that is only saved locally must display a pending state, not
   **Sent to server**.

Keep account setup tests restricted to an intentionally unregistered test
installation; do not reset either registered account to exercise registration.

## First message with different online times

This case requires the two test accounts not to have an established shared
conversation already. If they do, record first-message coverage as unavailable
for this pair instead of deleting accounts or their saved state.

1. Close or disconnect Bob. Alice looks up Bob, sends the first message and
   closes VOID. The text must survive in Alice's encrypted local outbox.
2. Open Bob later. Native setup runs automatically and queues the required reply;
   it must not show an invitation that needs approval. Close Bob again.
3. Open Alice later so it can process Bob's response and continue automatic
   encryption setup. Alternate connections as needed until the queued first text
   can be encrypted and delivered. Neither user needs to overlap online time.
4. On Bob, the actual first message appears in **Requests**. Open it and read it
   directly. There must be no Accept/Decline action before reading or replying.
5. Reply from Bob. The thread moves into Bob's regular message list and the reply
   reaches Alice. Opening or replying must not automatically mark identity trusted.
6. Confirm exact text appears once and no backend conversation/request records
   or KeyPackage inventory are required by this flow.

The current server has no prepublished recipient KeyPackages. Alice therefore
must reconnect after Bob first provides one before the first application text
can be encrypted. A test where Alice never reconnects after Bob's first opening
cannot yet deliver that text. This is a known bootstrap limitation, described in
the [separate extension proposal](offline-first-bootstrap.md), not a claim of
one-way offline first-message delivery.

## Established messages, restarts and reconnects

1. Exchange a message in each direction in the established thread. Outgoing
   application ciphertext becomes **Sent to server** only on `accepted`.
   Neither app should claim recipient delivery or read status.
2. Restart both apps without clearing data. Confirm the same accounts, thread
   and history load. Send another message each way.
3. Close or disconnect Bob. Alice sends a uniquely labeled message and reaches
   **Sent to server**, then Alice closes VOID.
4. Open Bob later. It authenticates, decrypts and saves that queued message using
   the existing group without a fresh KeyPackage or Welcome.
5. Restart Bob again and confirm the message remains readable exactly once.
   Bob's reply must later decrypt on Alice.
6. If recipient processing is deliberately interrupted, confirm journal recovery
   finishes the visible record before ACK. An ordinary restart does not prove
   an interruption between decryption and history insertion; deterministic native
   interruption tests cover that boundary separately.

## Record results

Record app version/code, device models and Android versions, both test VOID-IDs,
the failed step and the exact visible error or sanitized native error category.
Keep private keys, signing proofs, databases and native checkpoints out of logs.
Preserve app data after a failure; re-registering replaces neither a missing
protocol result nor a valid recovery path.

The backend offers no delivery receipts or exactly-once send guarantee. In a
lost-acceptance case, retain the saved ciphertext and record unknown send status
instead of treating `duplicate_event` as proof of delivery.

# Mandatory protocol update handling

Contract: backend commit `9eb1319`, `docs/MESSAGING_API.md` and
`docs/VERSIONED_DELIVERY.md`. The documented response after authentication is:

```json
{"type":"error","code":"update_required","min_protocol_version":"2"}
```

The minimum is a canonical positive decimal JSON string through 2147483647.
Missing, null, numeric, duplicate, unknown, and noncanonical fields are rejected.
The minimum field is allowed only with update_required. A minimum no higher than
the current advertisement is contradictory and does not activate the update UI.

The client still advertises application protocol V1. Negotiation completes
before an online session is exposed, so an incompatible connection cannot
publish prekeys, send queued ciphertext, or ACK events. A valid update response
ends the worker, closes its socket/command queues, and disables manual and
automatic reconnect for that worker. Ordinary disconnections retain existing
backoff/reconnect behavior. A fresh worker/application authenticates normally.

The required version is latched only in memory. The Slint screen replaces
messaging, setup, account-removal, and tooltip content while active, shows the
required minimum and update instruction, and offers Close application. Send and
resend callbacks also reject submission while blocked. Late status callbacks
cannot clear the gate. Closing uses normal application shutdown and worker
cleanup; no credentials, SQLCipher data, Signal keys, or account state are reset.

Tests use strict decoder fixtures, authenticated loopback relays, isolated
SQLCipher directories, and the Slint test backend. They cover V1 success, V2
minimum rejection, stopped retries/commands, a fresh worker, malformed minimums,
ordinary network recovery, a blocking UI latch, and preservation of history,
drafts, enrollment, and the protected database key. A separate native rendering
fixture captures the screen at 850x560 and 1600x900 without accounts or network.

No updater or download URL is provided. This verifies the documented response
with mock relays, not a production deployment or live server minimum. At the
pinned backend commit the mandatory minimum is documented as prepared, not
activated. Actual update distribution and server activation remain separate
release work; this change does not infer peer V2 capability or enable feature
sending.

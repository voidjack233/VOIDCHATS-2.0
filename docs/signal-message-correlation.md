# Correlate the blocked B to A message

Status: package inspection confirmed User A's installed APK contains these
diagnostics and matches the workspace APK. Existing recipient logs contain the
target event's fingerprints; the owner confirmed PostgreSQL's payload hash matches.
User B's older application build lacks the sender-hash diagnostic action, and no
sender fingerprint was found in existing logs. The historical cause remains unproven.
No backend code, schema, production queue row or real account state was changed.
Further builds, tests and live processing require owner authorization; the completed
isolated tests do not authorize new processing on either real installation.

## Scope and evidence

- User A, recipient phone: `110895178721525760`.
- User B, sender in LDPlayer: `111017620001521664`.
- Existing User A evidence: `SIGNAL_MESSAGE_DECRYPT_INVALID_MESSAGE`, candidate
  reason `MAC_MISMATCH`.
- A to B messages are visible on both devices. The missing direction is B to A.

Identify the oldest unacknowledged event in this two-account scope. Compare its
event ID, routing IDs and payload fingerprint before assigning a cause.

## Backend-agent read-only lookup

This queries the **messages table** only. Do not delete or update the event.
`sha256(bytea)` and `encode` are built-in PostgreSQL functions; no extension or
migration is required. See the [PostgreSQL documentation](https://www.postgresql.org/docs/14/functions-binarystring.html).

```sql
SELECT
    event_id::text AS event_id,
    sender_void_id::text AS sender_void_id,
    recipient_void_id::text AS recipient_void_id,
    created_at,
    expires_at,
    octet_length(payload) AS payload_bytes,
    encode(sha256(payload), 'hex') AS payload_sha256
FROM messages
WHERE sender_void_id = 111017620001521664::bigint
  AND recipient_void_id = 110895178721525760::bigint
  AND expires_at > now()
ORDER BY created_at, sender_void_id, event_id
LIMIT 1;
```

Return these metadata fields, omitting the payload itself. A missing row shows
only that no matching unexpired event was observed at lookup time; it does not
establish a historical delivery outcome. This scoped query does not inspect any
other account's events.

## Frontend metadata collection after an authorized APK update

On B, open the chat with A and choose **menu > Log send diagnostics**. This reads
up to 128 recent saved outgoing records, including accepted and failed records,
from the existing encrypted local store. It emits `stage=stored_send` lines to
the local Android `VoidMessaging` log. It does not upload, decrypt, resend or
acknowledge a message. If the target is older than that window, its absence from
this output is inconclusive.

On A, existing logs already contain this metadata. Any newly authorized
processing of the already queued event emits a
`stage=message_decrypt` failure line with the event ID, routing IDs,
`payload_sha256`, `ciphertext_sha256`, and fixed candidate-reason codes.
The new **Reconnect** action performs real socket replacement and authentication;
triggering another queued-event attempt still requires owner authorization.

Both fingerprint fields are SHA-256 over decoded bytes. `payload_sha256` covers
the full opaque payload and is directly comparable to the database query.
`ciphertext_sha256` covers only the serialized Signal ciphertext and is useful
between the endpoints. The backend does not need to parse the Signal envelope.
Unavailable metadata is reported as `UNKNOWN`. The existing text-based inbox
deduplication digest is unchanged and is not this transport fingerprint.

Read existing lines for a specific device with PowerShell:

```powershell
adb -s <device-serial> logcat -d -v threadtime -s VoidMessaging:W
```

For the same event and routing IDs, matching sender/server/recipient payload
hashes exclude byte alteration along that path, subject to SHA-256 collision
resistance. They do not prove historical receiving-chain eviction. Sender
message creation time and server acceptance time are distinct; additional
chronology is needed to establish the retention scenario. The diagnostic
`message_created_at` is copied from draft creation, not actual encryption time.

This inspection adds no ACK eligibility, key/session reset, server protocol,
database column or automatic resend policy. The native logger emits metadata
and allowlisted reason codes only, without plaintext, keys or raw native logs.

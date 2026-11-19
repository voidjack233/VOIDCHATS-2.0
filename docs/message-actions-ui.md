# Reply, reaction and message deletion UI

The reference is NOTE2EE at `641ea431e48c78b0c4c153c9a9c8cacf1558ab7a`:
`VOID0000-www/src/components/Chat/Messages/MessageItem.tsx`, `ReactionBar.tsx`,
and `Composer/MessageInput.tsx`. The existing native palette, bubbles, dates,
delivery indicators, pagination and quote navigation remain in use.

## Interactions

Hovering a bubble reveals a compact side rail for Reply, React and Delete. Its
180 ms leave grace period allows crossing to the rail. It is positioned outside
the row layout, with constrained placement at narrow widths. The bubble footer
also provides a keyboard-accessible Message actions button. Menus can be closed
with their X, Escape or an outside click; none of those actions deletes anything.

Reply selects the stable author/event reference and saves it with the current
draft on the storage worker. The composer banner shows the resolved author and
preview, with X to cancel without clearing the draft text. The input receives
focus. Restart and conversation switching restore the reference and preview;
deleted/unavailable originals use placeholders. Sending uses the existing reply
queue, encrypted payload, receipts and retry path. Quotes remain above bubbles,
with the existing cross-window jump-to-original behavior.

React opens a 24-emoji picker. Existing reactions are also available in the
picker and on the timeline, with your own reactions highlighted. Clicking a pill
toggles the worker's current persisted state, not a potentially stale UI value.
At ten distinct emojis the new-choice grid is disabled; existing emoji choices
remain available for joining/removing. SQLCipher remains the final limit and
revision authority. Controls never create ordinary chat bubbles.

Delete opens a message-specific menu. Delete for Me works locally for either
author and retains replay evidence. Delete for Everyone is offered only for an
undeleted outgoing message and requires a separate confirmation. The worker
rechecks authorship and scope. It queues the existing encrypted deletion intent,
applies the tombstone and blocks resending the original. A hidden viewport anchor
is replaced with a surviving neighbor while retaining its offset. Account deletion
and account initialization are not involved.

## Protocol gate and recovery

The default build still advertises V1. Reply, reactions and Delete for Everyone are
disabled with an explanation; Delete for Me and cancelling a saved reply remain
available. The worker independently enforces this gate even if UI flags are
stale. Loopback-only V2 fixtures and the explicitly opt-in coordinated V2 build
enable remote sends. See the [release runbook](protocol-v2-release.md); successful
negotiation does not attest the server-wide minimum.
No capability handshake, new encrypted format or Go change was introduced.

An already-submitted action can persist under its verified V2 session when the
socket closes before the worker reaches it. Transmission still requires an
active V2 session. This avoids losing the local intent during shutdown without
granting V1 sessions permission to save remote feature operations.

After compatible authentication and key publication, the existing worker recovers
pending control UUIDs from SQLCipher. It dispatches one per turn, leaves queued UI
work priority, reuses committed ciphertext and attempts at most five times per
connection, with exponential delays capped at 30 seconds. Failed intents remain
durable and offer Retry pending actions. Reconnect/restart reloads them. Ordinary
message startup retransmission is not added. Control failures are kept separately
from receipt/network notices so unrelated connection success cannot erase them.
Acceptance means relay acceptance, not recipient application. Existing 30-day
relay retention and cryptographic constraints are unchanged.

## Verification

Routine isolated Slint tests cover restoring/cancelling reply drafts, V1 send
blocking, worker enforcement against stale UI flags, local deletion, preserved
deduplication and replacement of a deleted viewport anchor without losing offset.
Existing Signal and storage tests cover failed writes, duplicate events,
out-of-order revisions, receipts and ciphertext reuse.

The opt-in `native_callbacks_exchange_features_offline_and_restore_reply_drafts`
test binds two real frontends and storage workers to synthetic accounts 101/202
on the unchanged disposable Go relay at `2faffd2`. It drives frontend callbacks
through Signal encryption, verified receipts, reply banners, reaction toggles,
an interrupted action recovered after restart, offline controls, tombstone and
quote refresh, and local deletion. It never opens personal account directories.

Native snapshot fixtures cover 850×560, 1080×680, 1280×720 and 1600×900, including
resizing, picker, deletion choices, confirmation, V1 explanation and hover rail.
These opt-in tests are excluded from routine runs because they create native
fixture windows or require explicit provisioning of the local Go/PostgreSQL relay.

Before release, coordinate the backend minimum V2 requirement, client V2
advertisement and production send gate. Existing mandatory-update handling remains
the compatibility policy. No production V2 activation has been performed.

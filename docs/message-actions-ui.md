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
deleted originals are no longer selected, and unavailable originals use a fallback. Sending uses the existing reply
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
applies the tombstone and blocks resending the original. A removed viewport anchor
is replaced with a surviving neighbor at that neighbor's measured screen position. Account deletion
and account initialization are not involved.

## Messaging readiness and recovery

All completed actions belong to initial V1 in normal builds. Reply and React
require messaging readiness, with connection/setup hints when unavailable.
Both deletion scopes work offline once protected local storage is open. Delete
for Everyone records the existing durable control and tombstone, removes the row
locally, and waits for messaging readiness to transmit. Its confirmation explains
this delay; it does not claim the recipient has applied the deletion. Pending
actions remain visible after an offline restart. Delete for Me creates no remote
control. Either scope prevents retrying a removed outgoing message, including a
failed message that has never left the local queue. Cancelling a saved reply also works offline.

Queued-action feedback stays in the existing connection footer, so it does not
shrink the chat viewport when a deletion is queued. Row removal captures settled
content coordinates and the current scroll offset before reconciliation, then
restores the surviving anchor after reconciliation. Reflow is marked before model
notifications to avoid treating layout correction as user scrolling. The geometry
cache is pruned to the bounded timeline and does not write the message model on
scroll frames.
Restored reply drafts can be queued offline like ordinary messages; Resend is
explicit. The worker independently validates account readiness and targets.
There is no release feature, protocol-V2 explanation or capability handshake.

An already-submitted action can persist under its negotiated session if the
socket closes before the worker processes it. Transmission requires an active
connection. This preserves durable intent during shutdown.

After compatible authentication and key publication, the existing worker recovers
pending control UUIDs from SQLCipher. It dispatches one per turn, leaves queued UI
work priority, reuses committed ciphertext and attempts at most five times per
connection, with exponential delays capped at 30 seconds. Failed intents remain
durable and offer Retry pending actions. Reconnect/restart reloads them. Ordinary
message startup retransmission is not added. Control failures are kept separately
from receipt/network notices so unrelated connection success cannot erase them.
Acceptance means relay acceptance, not recipient application. Existing 30-day
relay retention and cryptographic constraints are unchanged.

Pending reaction changes are coalesced per conversation, original author/event,
and emoji. Only `pending` controls without ciphertext can be superseded by a
newer durable revision. The latest local reaction references its control by the
existing primary keys, so selection does not scan every historical revision.
Older intent rows remain immutable for submission deduplication; they are excluded
from pending work and are never marked as relay-accepted. Dispatcher refreshes
remove stale jobs, transport skips superseded jobs, and encryption checks the rule
inside the Signal transaction before advancing the ratchet.

Add → Remove before encryption sends one explicit inactive update with the latest
revision. Add → Remove → Add sends one active update. A committed encrypted Add
followed by Remove still sends both: ciphertext, UUIDs, and uncertain acceptance
are preserved. This does not compact events already queued on the relay or enable
reaction controls while the sender is disconnected. No wire or schema migration
is required.

## Verification

Reaction-coalescing verification passes with 147 routine tests (14 opt-in fixtures
ignored). New isolated SQLCipher/Signal cases cover rapid toggles, reversed clock
timestamps, inactive delivery without a prior Add, immutable ciphertext, uncertain
acceptance, duplicate submissions, restart recovery, scope isolation, write rollback,
and removal of stale dispatcher jobs. Existing offline UI guards remain covered.

Routine isolated Slint tests cover restoring/cancelling reply drafts, offline
action guarding, worker enforcement against stale UI flags, local deletion, preserved
deduplication and replacement of a deleted viewport anchor without losing offset.
Existing Signal and storage tests cover failed writes, duplicate events,
out-of-order revisions, receipts and ciphertext reuse.

The offline deletion regression fixture activates the actual Slint menu and
confirmation buttons on failed, never-transmitted messages. It checks both
deletion scopes, preserved drafts, blocked original retries, durable pending
controls, and reopening storage without a relay connection. The full suite passes
141 tests (13 opt-in fixtures ignored); the native offline fixture passes separately
at 1080x680 and 850x560 with snapshots inspected. Cargo check, rustfmt and diff
checks pass. No production accounts or network connections are used by this fixture.

Deletion scroll-position verification also passes: 142 routine tests, 14 opt-in
fixtures ignored, plus the native pixel-position fixture run separately with the
normal optimized build. It checks a partially visible anchor, deletions above and
below it, batched removal, bottom following, and invariant viewport height when
offline actions become pending. The headless fixture evaluates layout and allows
restoration callbacks to settle before comparing coordinates, matching native
frame processing. Compilation, changed-file formatting and diff checks pass.

The opt-in `native_callbacks_exchange_features_offline_and_restore_reply_drafts`
test binds two real frontends and storage workers to synthetic accounts 101/202
on the unchanged disposable Go relay at `9eb1319`, minimum V1. It drives frontend callbacks
through Signal encryption, verified receipts, reply banners, reaction toggles,
an interrupted action recovered after restart, offline controls, tombstone and
quote refresh, and local deletion. It never opens personal account directories.

Native snapshot fixtures cover 850×560, 1080×680, 1280×720 and 1600×900, including
resizing, picker, deletion choices, confirmation, offline explanation and hover rail.
These opt-in tests are excluded from routine runs because they create native
fixture windows or require explicit provisioning of the local Go/PostgreSQL relay.

The [initial V1 baseline](protocol-v1.md) replaces the prerelease V2 plan.
Mandatory-update handling remains for future incompatible releases.
No production deployment or personal-account modification is performed.

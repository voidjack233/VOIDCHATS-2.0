# Encrypted message presentation

Reviewed the finalized receiver contract at e2373c3 before UI changes. Ordinary
text remains v2 with optional reply_to(author_void_id,event_id); receipts remain
v1. Reactions use explicit active state and positive per-reactor/emoji revisions.
Delete-for-Everyone is sender-authorized and records permanent tombstones.
Deferred recovery preserves authenticated plaintext and original identifiers.
No wire, receiver, migration, or production negotiation changes are needed here.

NOTE2EE MessageItem.tsx and ReactionBar.tsx (under VOID0000-www/src/components/
Chat/Messages) provide the presentation reference: separate compact quoted
previews above bubbles and rounded reaction/count pills below, with an accent
for the current participant. The [message actions UI](message-actions-ui.md) connects the completed outgoing controls.

SQLCipher resolves originals through inbox/outbox/queued-operation mappings,
scoped to the selected conversation, independently of the loaded window. Deleted
messages are excluded before history pagination and conversation preview selection;
their quotes, reactions, retry actions, and timeline rows are absent. A reply to a
deleted original keeps its own text without a quote. Missing originals still use
Message unavailable. No quoted body snapshots are stored. Stable cursor identities and bounded restoration
are reused for quote navigation and metadata updates.

The storage worker applies up to 64 contiguous, already-queued deliveries before
refreshing the timeline. Each encrypted event is still committed and acknowledged
independently. Existing tombstones and deferred recovery apply before opening local
history. This reduces offline synchronization flashes when the deletion is already
queued; it cannot conceal an original in anticipation of a deletion that has not
arrived, or one expired from the relay. Delete for Me remains local-only.

The receiver accepts bounded Unicode reaction tokens, not a full Unicode emoji
classification. The outgoing picker constrains choices and SQLCipher enforces
the ten-emoji limit. All completed operations are part of the initial
[application V1 baseline](protocol-v1.md).

Historical presentation-stage verification (before outgoing integration):
Verification: 116 Rust tests passed (three opt-in tests ignored in the normal
suite). The native rendering test was run separately and passed at 850x560,
1280x720, and 1600x900, with snapshots inspected. Receiver fixtures cover a
136-message history, originals outside the loaded 120 rows, missing/deleted
targets, reactions, and restart persistence. A Slint worker test invokes quote
navigation and verifies bounded restoration and unchanged drafts. Native fixture
renders are kept under ignored target/presentation-renders; no saved account is
opened. Cargo check/build and changed-file formatting passed.

## Silent deletion verification

The normal Rust suite passes 140 tests (12 opt-in fixtures remain ignored).
The isolated native silent-deletion fixture also passes separately at 850x560,
1080x680, and 1600x900, with snapshots inspected. Tests cover authenticated
deletion between two Signal identities, offline/restart recovery, duplicates,
deletion before the original, local-only removal, preview fallback, removal of a
quoted original, draft preservation, and pages with more tombstones than visible
rows. No protocol, schema, dependency, or production-account changes are needed.
`cargo check`, changed-file rustfmt checks, and `git diff --check` pass. The final
`cargo build` could not replace `target/debug/voidchats-native.exe` because an
existing VOID process has it open. Close that instance and rebuild to refresh the
normal executable; no running personal account was closed for this verification.
The current client has no separate message-content search to exercise; conversation
preview selection uses the same deletion-aware visibility filters as the timeline.

Implementation files:

- `src/storage/messaging.rs`: filter tombstones before history/preview limits.
- `src/storage/presentation.rs`: hide deleted originals and their quotes/navigation.
- `src/storage/outgoing.rs`: keep draft text without selecting a deleted original.
- `src/storage_ui.rs`: batch queued deliveries and preserve surviving timeline anchors.
- `ui/message-bubble.slint`: remove the obsolete deleted-placeholder text fallback.
- `src/storage/signal/crypto_tests/receiver.rs`, `outgoing.rs`, and `go_relay.rs`:
  verify silent visibility while retaining authenticated replay evidence.
- `src/storage_ui/tests/message_actions.rs`: expect actual row removal after deletion.
- `src/storage_ui/tests/silent_deletion.rs`: isolated model, anchor, restart, and native rendering checks.
- `docs/encrypted-event-receiver.md` and this document: current visibility behavior.

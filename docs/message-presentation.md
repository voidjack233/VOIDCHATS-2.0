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
originals resolve to Message deleted, missing originals to Message unavailable.
No quoted body snapshots are stored. Deleted messages suppress their own quote,
reactions, and retry actions. Stable cursor identities and bounded restoration
are reused for quote navigation and metadata updates.

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

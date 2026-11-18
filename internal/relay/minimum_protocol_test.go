package relay

import (
	"context"
	"testing"
	"time"

	"github.com/coder/websocket"
	"void-nothingness/internal/queue"
)

func expectUpdateRequired(t *testing.T, conn *websocket.Conn) {
	t.Helper()
	assertFields(t, read(t, conn), map[string]string{"type": "error", "code": "update_required", "min_protocol_version": "2"})
	ctx, cancel := context.WithTimeout(context.Background(), time.Second)
	defer cancel()
	if _, _, err := conn.Read(ctx); err == nil {
		t.Fatal("outdated connection remained open")
	}
}

func TestMinimumProtocolBlocksOutdatedCommands(t *testing.T) {
	frames := []map[string]string{
		{"type": "protocol", "version": "1"},
		queuedSend(firstEventID, "2", "AQ=="),
		versionedSend(firstEventID, "2", "AQ==", "2"),
		send("online", "2", "AQ=="),
		{"type": "ack", "sender_void_id": "2", "event_id": firstEventID},
		{"type": "lookup", "void_id": "2"},
		{"type": "prekeys_publish"},
		{"type": "prekeys_claim"},
	}
	for _, frame := range frames {
		t.Run(frame["type"], func(t *testing.T) {
			f := newFixture(t, Options{MinProtocolVersion: 2}, 2)
			conn := f.authenticate(t, "1")
			write(t, conn, frame)
			expectUpdateRequired(t, conn)
			rows, err := f.server.opts.Queue.Pending(context.Background(), "2", 100)
			if err != nil || len(rows) != 0 {
				t.Fatalf("outdated command enqueued ciphertext: %+v %v", rows, err)
			}
		})
	}
}

func TestMinimumProtocolHoldsQueueUntilCompatibleReconnect(t *testing.T) {
	f := newFixture(t, Options{MinProtocolVersion: 2}, 2)
	ctx := context.Background()
	first, err := f.server.opts.Queue.Send(ctx, "1", firstEventID, "2", []byte{1})
	if err != nil {
		t.Fatal(err)
	}
	if _, err = f.server.opts.Queue.SendVersioned(ctx, "1", secondEventID, "2", []byte{2}, 2); err != nil {
		t.Fatal(err)
	}
	old := f.authenticate(t, "2")
	// Complete an explicit delivery attempt; even a legacy head must be withheld.
	if !f.activeSession(t, "2").deliverOne() {
		t.Fatal("delivery pump stopped")
	}
	write(t, old, map[string]string{"type": "protocol", "version": "1"})
	expectUpdateRequired(t, old) // Any leaked event would precede this error.
	rows, err := f.server.opts.Queue.Pending(ctx, "2", 100)
	if err != nil || len(rows) != 2 || !rows[0].ExpiresAt.Equal(first.ExpiresAt) {
		t.Fatalf("outdated reconnect changed queue: %+v %v", rows, err)
	}
	updated := f.authenticate(t, "2")
	negotiate(t, updated, "2") // Must precede the first pending event.
	expectEvent(t, read(t, updated), firstEventID, "1", "AQ==")
	ackEvent(t, updated, "1", firstEventID)
	expectVersionedEvent(t, read(t, updated), secondEventID, "1", "Ag==", "2")
	// A replacement must negotiate again, including after a previous capable socket.
	replacement := f.authenticate(t, "2")
	if !f.activeSession(t, "2").deliverOne() {
		t.Fatal("replacement pump stopped")
	}
	write(t, replacement, map[string]string{"type": "ack", "sender_void_id": "1", "event_id": secondEventID})
	expectUpdateRequired(t, replacement)
	compatible := f.authenticate(t, "2")
	negotiate(t, compatible, "2")
	expectVersionedEvent(t, read(t, compatible), secondEventID, "1", "Ag==", "2")
	ackEvent(t, compatible, "1", secondEventID)
	ackEvent(t, compatible, "1", secondEventID)
	rows, err = f.server.opts.Queue.Pending(ctx, "2", 100)
	if err != nil || len(rows) != 0 {
		t.Fatalf("ACK did not drain queue: %+v %v", rows, err)
	}
}

func TestMinimumProtocolGatesOnlineAndQueuedMessaging(t *testing.T) {
	f := newFixture(t, Options{MinProtocolVersion: 2}, 2)
	alice, bob := f.authenticate(t, "1"), f.authenticate(t, "2")
	negotiate(t, alice, "2")
	write(t, alice, send("held", "2", "AQ=="))
	assertFields(t, read(t, alice), map[string]string{"type": "online_result", "exchange_id": "held", "status": "unavailable"})
	negotiate(t, bob, "2") // Would instead read online_event if forwarding bypassed the gate.
	write(t, alice, send("ready", "2", "Ag=="))
	assertFields(t, read(t, bob), map[string]string{"type": "online_event", "exchange_id": "ready", "from_void_id": "1", "payload_base64": "Ag=="})
	assertFields(t, read(t, alice), map[string]string{"type": "online_result", "exchange_id": "ready", "status": "forwarded"})
	write(t, alice, queuedSend(firstEventID, "2", "Aw=="))
	expectAccepted(t, read(t, alice), firstEventID)
	expectEvent(t, read(t, bob), firstEventID, "1", "Aw==")
	ackEvent(t, bob, "1", firstEventID)
}

func TestMinimumProtocolPreservesExpiryAndAuthentication(t *testing.T) {
	f := newFixture(t, Options{MinProtocolVersion: 2}, 2)
	created := time.Now().Add(-queue.Retention - time.Hour)
	if err := f.store.Enqueue(context.Background(), queue.Event{EventID: firstEventID, SenderVoidID: "1", RecipientVoidID: "2", Payload: []byte{1}, CreatedAt: created, ExpiresAt: created.Add(queue.Retention)}); err != nil {
		t.Fatal(err)
	}
	if _, err := f.server.opts.Queue.Send(context.Background(), "1", secondEventID, "2", []byte{2}); err != nil {
		t.Fatal(err)
	}
	failed := f.dial(t)
	challenge := begin(t, failed, "1")
	prove(t, failed, challenge, f.keys["2"])
	assertFields(t, read(t, failed), map[string]string{"type": "error", "code": "authentication_failed"})
	bob := f.authenticate(t, "2")
	negotiate(t, bob, "2")
	expectEvent(t, read(t, bob), secondEventID, "1", "Ag==")
	if n, err := f.server.opts.Queue.Cleanup(context.Background(), 100); err != nil || n != 1 {
		t.Fatalf("cleanup: %d %v", n, err)
	}
}

func TestMinimumVersionOneAllowsLegacyClients(t *testing.T) {
	f := newFixture(t, Options{MinProtocolVersion: 1}, 2)
	alice, bob := f.authenticate(t, "1"), f.authenticate(t, "2")
	write(t, alice, queuedSend(firstEventID, "2", "AQ=="))
	expectAccepted(t, read(t, alice), firstEventID)
	expectEvent(t, read(t, bob), firstEventID, "1", "AQ==")
	ackEvent(t, bob, "1", firstEventID)
	if _, err := New(f.server.auth, Options{Queue: f.server.opts.Queue, MinProtocolVersion: -1}); err == nil {
		t.Fatal("negative minimum accepted")
	}
}

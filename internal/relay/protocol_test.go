package relay

import (
	"context"
	"strconv"
	"testing"
	"time"

	"github.com/coder/websocket"

	"void-nothingness/internal/queue"
)

const thirdEventID = "10000000-0000-4000-8000-000000000003"

func versionedSend(id, target, payload, version string) map[string]string {
	frame := queuedSend(id, target, payload)
	frame["type"] = "send_versioned"
	frame["min_protocol_version"] = version
	return frame
}

func negotiate(t *testing.T, conn *websocket.Conn, version string) {
	t.Helper()
	write(t, conn, map[string]string{"type": "protocol", "version": version})
	assertFields(t, read(t, conn), map[string]string{"type": "protocol_ok", "version": version})
}

func expectVersionedEvent(t *testing.T, frame map[string]string, id, sender, payload, version string) {
	t.Helper()
	if frame["type"] != "event_versioned" || frame["min_protocol_version"] != version || len(frame) != 7 {
		t.Fatalf("versioned delivery = %#v", frame)
	}
	legacy := make(map[string]string, len(frame)-1)
	for key, value := range frame {
		if key != "min_protocol_version" {
			legacy[key] = value
		}
	}
	legacy["type"] = "event"
	expectEvent(t, legacy, id, sender, payload)
}

func (f *fixture) activeSession(t *testing.T, id string) *session {
	t.Helper()
	f.server.mu.Lock()
	defer f.server.mu.Unlock()
	client := f.server.active[id]
	if client == nil {
		t.Fatal("missing active recipient")
	}
	return client
}

// Run a complete delivery attempt before the lookup barrier. A mistaken event
// write would precede the person response, without a timeout closing the socket.
func expectHeld(t *testing.T, f *fixture, conn *websocket.Conn, id string) {
	t.Helper()
	client := f.activeSession(t, id)
	if !client.deliverOne() {
		t.Fatal("incompatible event stopped recipient")
	}
	client.deliveryMu.Lock()
	inFlight := client.inFlightID
	client.deliveryMu.Unlock()
	if inFlight != "" {
		t.Fatalf("withheld event marked in flight: %s", inFlight)
	}
	write(t, conn, map[string]string{"type": "lookup", "void_id": id})
	frame := read(t, conn)
	if frame["type"] != "person" || frame["void_id"] != id || frame["auth_public_key"] == "" || len(frame) != 3 {
		t.Fatalf("delivery bypassed incompatible queue head: %#v", frame)
	}
}

func ackEvent(t *testing.T, conn *websocket.Conn, sender, id string) {
	t.Helper()
	frame := map[string]string{"type": "ack", "sender_void_id": sender, "event_id": id}
	write(t, conn, frame)
	frame["type"] = "ack_ok"
	assertFields(t, read(t, conn), frame)
}

func TestProtocolCommandsRequireAuthentication(t *testing.T) {
	for _, frame := range []map[string]string{
		{"type": "protocol", "version": "2"},
		versionedSend(firstEventID, "2", "AQ==", "2"),
	} {
		t.Run(frame["type"], func(t *testing.T) {
			f := newFixture(t, Options{}, 2)
			conn := f.dial(t)
			write(t, conn, frame)
			assertFields(t, read(t, conn), map[string]string{"type": "error", "code": "unauthorized"})
			if f.server.ActiveCount() != 0 {
				t.Fatal("version command authenticated a connection")
			}
		})
	}
}

func TestProtocolRejectsInvalidVersionsAndShapes(t *testing.T) {
	for _, kind := range []string{"protocol", "send_versioned"} {
		for _, version := range []string{"", "0", "-1", "+2", "02", " 2", "2 ", "2.0", "2e0", "2147483648", "999999999999999999999999"} {
			t.Run(kind+"/"+strconv.Quote(version), func(t *testing.T) {
				f := newFixture(t, Options{}, 2)
				conn := f.authenticate(t, "1")
				frame := map[string]string{"type": "protocol", "version": version}
				if kind == "send_versioned" {
					frame = versionedSend(firstEventID, "2", "AQ==", version)
				}
				write(t, conn, frame)
				assertFields(t, read(t, conn), map[string]string{"type": "error", "code": "invalid_frame"})
				rows, err := f.server.opts.Queue.Pending(context.Background(), "2", 1)
				if err != nil || len(rows) != 0 {
					t.Fatalf("invalid version persisted: %+v %v", rows, err)
				}
			})
		}
	}
	for _, body := range []string{
		`{"type":"protocol","version":2}`,
		`{"type":"protocol","version":null}`,
		`{"type":"protocol","version":"2","version":"1"}`,
		`{"type":"protocol","version":"2","void_id":"1"}`,
		`{"type":"send","event_id":"` + firstEventID + `","to_void_id":"2","payload_base64":"AQ==","min_protocol_version":"2"}`,
		`{"type":"send_versioned","event_id":"` + firstEventID + `","to_void_id":"2","payload_base64":"AQ=="}`,
	} {
		t.Run(body, func(t *testing.T) {
			f := newFixture(t, Options{}, 2)
			conn := f.authenticate(t, "1")
			writeRaw(t, conn, websocket.MessageText, []byte(body))
			assertFields(t, read(t, conn), map[string]string{"type": "error", "code": "invalid_frame"})
		})
	}
}

func TestProtocolSupportOnlyIncreasesWithinConnection(t *testing.T) {
	f := newFixture(t, Options{}, 1)
	conn := f.authenticate(t, "1")
	for _, version := range []string{"1", "2", "2", "2147483647"} {
		negotiate(t, conn, version)
	}
	write(t, conn, map[string]string{"type": "protocol", "version": "2"})
	assertFields(t, read(t, conn), map[string]string{"type": "error", "code": "invalid_frame"})
}

func TestVersionOneSendRetainsLegacyDelivery(t *testing.T) {
	f := newFixture(t, Options{}, 2)
	alice, bob := f.authenticate(t, "1"), f.authenticate(t, "2")
	write(t, alice, versionedSend(firstEventID, "2", "AP8BAA==", "1"))
	expectAccepted(t, read(t, alice), firstEventID)
	expectEvent(t, read(t, bob), firstEventID, "1", "AP8BAA==")
	ackEvent(t, bob, "1", firstEventID)
	negotiate(t, bob, "2")
	write(t, alice, queuedSend(secondEventID, "2", "Ag=="))
	expectAccepted(t, read(t, alice), secondEventID)
	expectEvent(t, read(t, bob), secondEventID, "1", "Ag==")
}

func TestIncompatibleHeadHoldsLaterCiphertextAndResumesInOrder(t *testing.T) {
	f := newFixture(t, Options{}, 3)
	alice, bob, carol := f.authenticate(t, "1"), f.authenticate(t, "2"), f.authenticate(t, "3")
	write(t, alice, queuedSend(firstEventID, "2", "AQ=="))
	expectAccepted(t, read(t, alice), firstEventID)
	expectEvent(t, read(t, bob), firstEventID, "1", "AQ==")
	write(t, alice, versionedSend(secondEventID, "2", "AP8BAA==", "2"))
	expectAccepted(t, read(t, alice), secondEventID)
	write(t, alice, queuedSend(thirdEventID, "2", "Aw=="))
	expectAccepted(t, read(t, alice), thirdEventID)
	write(t, carol, queuedSend(firstEventID, "2", "BA=="))
	expectAccepted(t, read(t, carol), firstEventID)
	ackEvent(t, bob, "1", firstEventID)
	expectHeld(t, f, bob, "2")
	// A duplicate cannot downgrade the queued event or change its ciphertext.
	write(t, alice, versionedSend(secondEventID, "2", "BQ==", "1"))
	assertFields(t, read(t, alice), map[string]string{"type": "error", "code": "duplicate_event"})
	rows, err := f.server.opts.Queue.Pending(context.Background(), "2", 100)
	if err != nil || len(rows) != 3 || rows[0].EventID != secondEventID || rows[0].MinProtocolVersion != 2 || string(rows[0].Payload) != string([]byte{0, 255, 1, 0}) {
		t.Fatalf("withheld queue changed: %+v %v", rows, err)
	}
	expectHeld(t, f, bob, "2")
	negotiate(t, bob, "2")
	expectVersionedEvent(t, read(t, bob), secondEventID, "1", "AP8BAA==", "2")
	// Repeating negotiation must not bypass the single-event ACK window.
	negotiate(t, bob, "2")
	ackEvent(t, bob, "1", secondEventID)
	expectEvent(t, read(t, bob), thirdEventID, "1", "Aw==")
	ackEvent(t, bob, "1", thirdEventID)
	expectEvent(t, read(t, bob), firstEventID, "3", "BA==")
	ackEvent(t, bob, "3", firstEventID)
	rows, err = f.server.opts.Queue.Pending(context.Background(), "2", 100)
	if err != nil || len(rows) != 0 {
		t.Fatalf("ACK did not drain ordered queue: %+v %v", rows, err)
	}
}

func TestOlderReconnectWithholdsPreviouslyOfferedVersionedEvent(t *testing.T) {
	f := newFixture(t, Options{}, 2)
	alice := f.authenticate(t, "1")
	write(t, alice, versionedSend(firstEventID, "2", "AP8BAA==", "2"))
	expectAccepted(t, read(t, alice), firstEventID)
	write(t, alice, queuedSend(secondEventID, "2", "Ag=="))
	expectAccepted(t, read(t, alice), secondEventID)
	bob := f.authenticate(t, "2")
	expectHeld(t, f, bob, "2")
	negotiate(t, bob, "2")
	first := read(t, bob)
	expectVersionedEvent(t, first, firstEventID, "1", "AP8BAA==", "2")
	// Replace the capable connection while its ciphertext is unacknowledged.
	older := f.authenticate(t, "2")
	expectHeld(t, f, older, "2")
	client := f.activeSession(t, "2")
	client.deliveryMu.Lock()
	version := client.protocolVersion
	client.deliveryMu.Unlock()
	if version != 1 {
		t.Fatalf("reconnect inherited support %d", version)
	}
	rows, err := f.server.opts.Queue.Pending(context.Background(), "2", 100)
	if err != nil || len(rows) != 2 {
		t.Fatalf("older reconnect removed ciphertext: %+v %v", rows, err)
	}
	compatible := f.authenticate(t, "2")
	expectHeld(t, f, compatible, "2")
	negotiate(t, compatible, "2")
	assertFields(t, read(t, compatible), first)
	ackEvent(t, compatible, "1", firstEventID)
	expectEvent(t, read(t, compatible), secondEventID, "1", "Ag==")
	ackEvent(t, compatible, "1", secondEventID)
	ackEvent(t, compatible, "1", firstEventID) // Existing duplicate ACK behavior.
}

func TestExpiredIncompatibleHeadAllowsLegacyDelivery(t *testing.T) {
	f := newFixture(t, Options{}, 2)
	created := time.Now().Add(-queue.Retention - time.Hour)
	if err := f.store.Enqueue(context.Background(), queue.Event{EventID: firstEventID, SenderVoidID: "1", RecipientVoidID: "2", Payload: []byte{255}, MinProtocolVersion: 2, CreatedAt: created, ExpiresAt: created.Add(queue.Retention)}); err != nil {
		t.Fatal(err)
	}
	alice := f.authenticate(t, "1")
	write(t, alice, queuedSend(secondEventID, "2", "Ag=="))
	expectAccepted(t, read(t, alice), secondEventID)
	bob := f.authenticate(t, "2")
	expectEvent(t, read(t, bob), secondEventID, "1", "Ag==")
	removed, err := f.server.opts.Queue.Cleanup(context.Background(), 100)
	if err != nil || removed != 1 {
		t.Fatalf("incompatible expiry cleanup = %d %v", removed, err)
	}
}

func TestHeldConnectionResumesAfterIncompatibleHeadExpires(t *testing.T) {
	f := newFixture(t, Options{}, 2)
	ctx := context.Background()
	expires := time.Now().UTC().Truncate(time.Microsecond).Add(time.Hour)
	if err := f.store.Enqueue(ctx, queue.Event{EventID: firstEventID, SenderVoidID: "1", RecipientVoidID: "2", Payload: []byte{255}, MinProtocolVersion: 2, CreatedAt: expires.Add(-queue.Retention), ExpiresAt: expires}); err != nil {
		t.Fatal(err)
	}
	alice := f.authenticate(t, "1")
	write(t, alice, queuedSend(secondEventID, "2", "Ag=="))
	expectAccepted(t, read(t, alice), secondEventID)
	bob := f.authenticate(t, "2")
	expectHeld(t, f, bob, "2")
	// Advance the cleanup clock directly, without changing retention or
	// waiting for a wall-clock deadline. Only the old head expires.
	if removed, err := f.store.Expire(ctx, expires, 100); err != nil || removed != 1 {
		t.Fatalf("withheld expiry = %d %v", removed, err)
	}
	f.server.wakeRecipient("2")
	expectEvent(t, read(t, bob), secondEventID, "1", "Ag==")
	ackEvent(t, bob, "1", secondEventID)
}

func TestHigherRequiredVersionStaysHeldUntilAdvertised(t *testing.T) {
	f := newFixture(t, Options{}, 2)
	alice, bob := f.authenticate(t, "1"), f.authenticate(t, "2")
	negotiate(t, bob, "2")
	write(t, alice, versionedSend(firstEventID, "2", "AP8BAA==", "3"))
	expectAccepted(t, read(t, alice), firstEventID)
	expectHeld(t, f, bob, "2")
	negotiate(t, bob, "3")
	expectVersionedEvent(t, read(t, bob), firstEventID, "1", "AP8BAA==", "3")
}

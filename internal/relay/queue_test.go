package relay

import (
	"bytes"
	"context"
	"encoding/base64"
	"encoding/json"
	"fmt"
	"testing"
	"time"

	"void-nothingness/internal/queue"
)

const firstEventID = "10000000-0000-4000-8000-000000000001"
const secondEventID = "10000000-0000-4000-8000-000000000002"

func queuedSend(id, target, payload string) map[string]string {
	return map[string]string{"type": "send", "event_id": id, "to_void_id": target, "payload_base64": payload}
}

func expectAccepted(t *testing.T, frame map[string]string, id string) {
	t.Helper()
	if frame["type"] != "accepted" || frame["event_id"] != id || len(frame) != 4 {
		t.Fatalf("acceptance = %#v", frame)
	}
	created, err := time.Parse(time.RFC3339Nano, frame["created_at"])
	if err != nil {
		t.Fatal(err)
	}
	expires, err := time.Parse(time.RFC3339Nano, frame["expires_at"])
	if err != nil || expires.Sub(created) != queue.Retention || time.Since(created) > time.Minute {
		t.Fatalf("server retention timestamps = %#v (%v)", frame, err)
	}
}

func expectEvent(t *testing.T, frame map[string]string, id, sender, payload string) {
	t.Helper()
	if frame["type"] != "event" || frame["event_id"] != id || frame["from_void_id"] != sender || frame["payload_base64"] != payload || frame["created_at"] == "" || frame["expires_at"] == "" || len(frame) != 6 {
		t.Fatalf("delivery = %#v", frame)
	}
}

func TestQueuePersistenceAcceptanceForwardingAndACKDeletion(t *testing.T) {
	f := newFixture(t, Options{}, 3)
	alice, bob, outsider := f.authenticate(t, "1"), f.authenticate(t, "2"), f.authenticate(t, "3")
	write(t, alice, map[string]string{"type": "lookup", "void_id": "2"})
	person := read(t, alice)
	if person["type"] != "person" || person["void_id"] != "2" || person["auth_public_key"] == "" || len(person) != 3 {
		t.Fatalf("lookup = %#v", person)
	}
	write(t, alice, queuedSend(firstEventID, "2", "AP8BAA=="))
	expectAccepted(t, read(t, alice), firstEventID)
	expectEvent(t, read(t, bob), firstEventID, "1", "AP8BAA==")
	rows, err := f.server.opts.Queue.Pending(context.Background(), "2", 100)
	if err != nil || len(rows) != 1 || rows[0].SenderVoidID != "1" {
		t.Fatalf("transport write deleted queued event: %+v %v", rows, err)
	}
	write(t, outsider, map[string]string{"type": "ack", "sender_void_id": "1", "event_id": firstEventID})
	assertFields(t, read(t, outsider), map[string]string{"type": "ack_ok", "sender_void_id": "1", "event_id": firstEventID})
	rows, err = f.server.opts.Queue.Pending(context.Background(), "2", 100)
	if err != nil || len(rows) != 1 {
		t.Fatalf("wrong recipient deleted ciphertext: %+v %v", rows, err)
	}
	write(t, alice, queuedSend(secondEventID, "2", "AgM="))
	expectAccepted(t, read(t, alice), secondEventID)
	write(t, bob, map[string]string{"type": "ack", "sender_void_id": "1", "event_id": firstEventID})
	assertFields(t, read(t, bob), map[string]string{"type": "ack_ok", "sender_void_id": "1", "event_id": firstEventID})
	expectEvent(t, read(t, bob), secondEventID, "1", "AgM=")
	write(t, bob, map[string]string{"type": "ack", "sender_void_id": "1", "event_id": secondEventID})
	read(t, bob)
	rows, err = f.server.opts.Queue.Pending(context.Background(), "2", 100)
	if err != nil || len(rows) != 0 {
		t.Fatalf("ACK did not remove ciphertext: %+v %v", rows, err)
	}
}

func TestQueueOfflineDeliveryReconnectAndNoPersistentRetryState(t *testing.T) {
	f := newFixture(t, Options{}, 2)
	alice := f.authenticate(t, "1")
	write(t, alice, queuedSend(firstEventID, "2", "AAH+/w=="))
	expectAccepted(t, read(t, alice), firstEventID)
	write(t, alice, queuedSend(firstEventID, "2", "AAH+/w=="))
	assertFields(t, read(t, alice), map[string]string{"type": "error", "code": "duplicate_event"})
	_ = alice.CloseNow()
	eventually(t, "sender did not disconnect", func() bool { return f.server.ActiveCount() == 0 })
	bob := f.authenticate(t, "2")
	first := read(t, bob)
	expectEvent(t, first, firstEventID, "1", "AAH+/w==")
	_ = bob.CloseNow()
	eventually(t, "recipient did not disconnect", func() bool { return f.server.ActiveCount() == 0 })
	bob = f.authenticate(t, "2")
	assertFields(t, read(t, bob), first)
	write(t, bob, map[string]string{"type": "ack", "sender_void_id": "1", "event_id": firstEventID})
	read(t, bob)
	write(t, bob, map[string]string{"type": "ack", "sender_void_id": "1", "event_id": firstEventID})
	assertFields(t, read(t, bob), map[string]string{"type": "ack_ok", "sender_void_id": "1", "event_id": firstEventID})
	// There is intentionally no retained identifier after deletion. Reusing an
	// ID can enqueue a new row; this is a documented limitation, not idempotency.
	alice = f.authenticate(t, "1")
	write(t, alice, queuedSend(firstEventID, "2", "AQ=="))
	expectAccepted(t, read(t, alice), firstEventID)
	expectEvent(t, read(t, bob), firstEventID, "1", "AQ==")
}

func TestQueueRejectsSenderTimestampsLegacyShapesAndImpersonation(t *testing.T) {
	for _, field := range []string{"created_at", "expires_at", "sender_void_id", "conversation_id"} {
		t.Run(field, func(t *testing.T) {
			f := newFixture(t, Options{}, 2)
			alice := f.authenticate(t, "1")
			frame := queuedSend(firstEventID, "2", "AQ==")
			frame[field] = "2030-01-01T00:00:00Z"
			write(t, alice, frame)
			assertFields(t, read(t, alice), map[string]string{"type": "error", "code": "invalid_frame"})
			rows, err := f.server.opts.Queue.Pending(context.Background(), "2", 100)
			if err != nil || len(rows) != 0 {
				t.Fatalf("invalid frame persisted: %+v %v", rows, err)
			}
		})
	}
	f := newFixture(t, Options{}, 2)
	alice := f.authenticate(t, "1")
	write(t, alice, map[string]string{"type": "send", "request_id": "old", "recipient_id": "2", "payload": "AQ=="})
	assertFields(t, read(t, alice), map[string]string{"type": "error", "code": "invalid_frame"})
	unauthenticated := f.dial(t)
	write(t, unauthenticated, queuedSend(firstEventID, "2", "AQ=="))
	assertFields(t, read(t, unauthenticated), map[string]string{"type": "error", "code": "unauthorized"})
}

func TestQueueExpiredEventIsNotDeliveredAndCleanupDeletes(t *testing.T) {
	f := newFixture(t, Options{}, 2)
	created := time.Now().Add(-31 * 24 * time.Hour)
	expired := queue.Event{EventID: firstEventID, SenderVoidID: "1", RecipientVoidID: "2", Payload: []byte{0, 255, 1}, CreatedAt: created, ExpiresAt: created.Add(queue.Retention)}
	if err := f.store.Enqueue(context.Background(), expired); err != nil {
		t.Fatal(err)
	}
	alice := f.authenticate(t, "1")
	write(t, alice, queuedSend(secondEventID, "2", "AQ=="))
	expectAccepted(t, read(t, alice), secondEventID)
	bob := f.authenticate(t, "2")
	expectEvent(t, read(t, bob), secondEventID, "1", "AQ==")
	count, err := f.server.opts.Queue.Cleanup(context.Background(), 100)
	if err != nil || count != 1 {
		t.Fatalf("expired cleanup = %d %v", count, err)
	}
}

func TestQueueRateLimitSurvivesConnectionReplacement(t *testing.T) {
	f := newFixture(t, Options{}, 2)
	for i := 0; i < 40; i++ {
		if !f.server.allowCommand("1") {
			t.Fatalf("command %d unexpectedly limited", i)
		}
	}
	alice := f.authenticate(t, "1")
	write(t, alice, map[string]string{"type": "lookup", "void_id": "2"})
	assertFields(t, read(t, alice), map[string]string{"type": "error", "code": "rate_limited"})
	if !f.server.allowCommand("2") {
		t.Fatal("one account consumed another's rate budget")
	}
	f.server.mu.Lock()
	for i := len(f.server.rates); i < 4096; i++ {
		f.server.rates[fmt.Sprintf("synthetic-%d", i)] = commandBucket{tokens: 40, seen: time.Now()}
	}
	f.server.mu.Unlock()
	if f.server.allowCommand("new-account") {
		t.Fatal("unbounded rate-account admission")
	}
}

type gatedQueue struct {
	queue.Store
	entered, release chan struct{}
}

func (s *gatedQueue) Enqueue(ctx context.Context, event queue.Event) error {
	close(s.entered)
	select {
	case <-s.release:
		return s.Store.Enqueue(ctx, event)
	case <-ctx.Done():
		return ctx.Err()
	}
}

func TestQueueAcceptanceWaitsForSuccessfulPersistence(t *testing.T) {
	f := newFixture(t, Options{}, 2)
	gate := &gatedQueue{Store: f.store, entered: make(chan struct{}), release: make(chan struct{})}
	service, err := queue.New(gate)
	if err != nil {
		t.Fatal(err)
	}
	f.server.opts.Queue = service
	alice := f.authenticate(t, "1")
	write(t, alice, queuedSend(firstEventID, "2", "AQ=="))
	mustReceive(t, gate.entered)
	result := make(chan map[string]string, 1)
	readErrors := make(chan error, 1)
	go func() {
		ctx, cancel := context.WithTimeout(context.Background(), 2*time.Second)
		defer cancel()
		_, body, err := alice.Read(ctx)
		if err != nil {
			readErrors <- err
			return
		}
		var frame map[string]string
		if err := json.Unmarshal(body, &frame); err != nil {
			readErrors <- err
			return
		}
		result <- frame
	}()
	select {
	case frame := <-result:
		t.Fatalf("acceptance before persistence completed: %#v", frame)
	case err := <-readErrors:
		t.Fatal(err)
	case <-time.After(30 * time.Millisecond):
	}
	close(gate.release)
	select {
	case frame := <-result:
		expectAccepted(t, frame, firstEventID)
	case err := <-readErrors:
		t.Fatal(err)
	case <-time.After(time.Second):
		t.Fatal("committed event received no acceptance")
	}
	rows, err := service.Pending(context.Background(), "2", 100)
	if err != nil || len(rows) != 1 {
		t.Fatalf("accepted event absent from queue: %+v %v", rows, err)
	}
}

func TestQueueOneMiBPayloadAndOutgoingByteBudget(t *testing.T) {
	f := newFixture(t, Options{}, 2)
	alice, bob := f.authenticate(t, "1"), f.authenticate(t, "2")
	payload := base64.StdEncoding.EncodeToString(bytes.Repeat([]byte{0, 255, 9, 5}, queue.MaxPayloadBytes/4))
	write(t, alice, queuedSend(firstEventID, "2", payload))
	expectAccepted(t, read(t, alice), firstEventID)
	expectEvent(t, read(t, bob), firstEventID, "1", payload)
	fake := fakeSession(t, f.server, "3", newControlledSocket())
	if fake.enqueue(outbound{body: make([]byte, maxQueuedBytes+1), ctx: fake.ctx, receipt: make(chan error, 1)}) {
		t.Fatal("oversized outgoing item bypassed byte budget")
	}
	item := outbound{body: make([]byte, 1<<20), ctx: fake.ctx, receipt: make(chan error, 1)}
	if !fake.enqueue(item) || !fake.enqueue(item) || fake.enqueue(item) || fake.queuedBytes.Load() != maxQueuedBytes {
		t.Fatal("outgoing queue exceeded byte budget")
	}
}

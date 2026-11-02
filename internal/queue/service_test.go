package queue

import (
	"bytes"
	"context"
	"errors"
	"fmt"
	"sync"
	"testing"
	"time"
)

func testID(n int) string { return fmt.Sprintf("00000000-0000-4000-8000-%012x", n) }
func testService(t *testing.T) (*Service, *MemoryStore) {
	t.Helper()
	store := NewMemoryStore([]Person{{VoidID: "1", AuthPublicKey: "synthetic-public-1"}, {VoidID: "2", AuthPublicKey: "synthetic-public-2"}, {VoidID: "3", AuthPublicKey: "synthetic-public-3"}})
	service, err := New(store)
	if err != nil {
		t.Fatal(err)
	}
	return service, store
}

func TestServerTimestampsOfflineQueueAndAuthenticatedDeletion(t *testing.T) {
	ctx := context.Background()
	s, store := testService(t)
	now := time.Date(2026, 11, 2, 16, 30, 0, 123456789, time.FixedZone("synthetic", 8*3600))
	s.now = func() time.Time { return now }
	payload := []byte{0, 255, 1, 254}
	event, err := s.Send(ctx, "1", testID(1), "2", payload)
	if err != nil {
		t.Fatal(err)
	}
	want := now.UTC().Truncate(time.Microsecond)
	if !event.CreatedAt.Equal(want) || event.CreatedAt.Location() != time.UTC || !event.ExpiresAt.Equal(want.Add(30*24*time.Hour)) {
		t.Fatal("timestamps must originate from one server clock reading")
	}
	payload[0] = 99
	event.Payload[1] = 99
	restarted, _ := New(store)
	restarted.now = func() time.Time { return now }
	rows, err := restarted.Pending(ctx, "2", 100)
	if err != nil || len(rows) != 1 || !bytes.Equal(rows[0].Payload, []byte{0, 255, 1, 254}) {
		t.Fatalf("offline bytes not preserved: %+v %v", rows, err)
	}
	if err = restarted.Ack(ctx, "3", "1", testID(1)); err != nil {
		t.Fatal(err)
	}
	rows, _ = restarted.Pending(ctx, "2", 100)
	if len(rows) != 1 {
		t.Fatal("foreign account deleted recipient queue")
	}
	if err = restarted.Ack(ctx, "2", "1", testID(1)); err != nil {
		t.Fatal(err)
	}
	if len(store.events) != 0 {
		t.Fatal("ACK retained ciphertext/history/tombstone")
	}
	if err = restarted.Ack(ctx, "2", "1", testID(1)); err != nil {
		t.Fatal("repeated ACK should be harmless")
	}
}

func TestExpiryBoundaryAndBoundedCleanup(t *testing.T) {
	ctx := context.Background()
	s, store := testService(t)
	now := time.Date(2026, 11, 2, 0, 0, 0, 0, time.UTC)
	s.now = func() time.Time { return now }
	for i := 1; i <= 3; i++ {
		if _, err := s.Send(ctx, "1", testID(i), "2", []byte("opaque")); err != nil {
			t.Fatal(err)
		}
	}
	now = now.Add(Retention - time.Microsecond)
	if rows, _ := s.Pending(ctx, "2", 100); len(rows) != 3 {
		t.Fatal("premature expiry")
	}
	now = now.Add(time.Microsecond)
	if rows, _ := s.Pending(ctx, "2", 100); len(rows) != 0 {
		t.Fatal("expired ciphertext delivered")
	}
	n, err := s.Cleanup(ctx, 2)
	if err != nil || n != 2 || len(store.events) != 1 {
		t.Fatalf("unbounded cleanup: %d %v", n, err)
	}
	n, err = s.Cleanup(ctx, 2)
	if err != nil || n != 1 || len(store.events) != 0 {
		t.Fatalf("expiry retained records: %d %v", n, err)
	}
}

func TestDuplicatePendingIDsDoNotExtendExpiryAndNoPostDeletionGuarantee(t *testing.T) {
	ctx := context.Background()
	s, store := testService(t)
	now := time.Date(2026, 11, 2, 0, 0, 0, 0, time.UTC)
	s.now = func() time.Time { return now }
	first, err := s.Send(ctx, "1", testID(1), "2", []byte("opaque"))
	if err != nil {
		t.Fatal(err)
	}
	now = now.Add(time.Hour)
	if _, err = s.Send(ctx, "1", testID(1), "3", []byte("changed")); !errors.Is(err, ErrDuplicate) {
		t.Fatal(err)
	}
	rows, _ := s.Pending(ctx, "2", 100)
	if len(rows) != 1 || !rows[0].ExpiresAt.Equal(first.ExpiresAt) {
		t.Fatal("duplicate extended retention or changed destination")
	}
	// IDs are sender-scoped. Another sender cannot overwrite the original event.
	if _, err = s.Send(ctx, "3", testID(1), "2", []byte("independent")); err != nil {
		t.Fatal(err)
	}
	if err = s.Ack(ctx, "2", "1", testID(1)); err != nil {
		t.Fatal(err)
	}
	// This explicitly documents the deferred reliability limit: no tombstone is
	// retained, so a reused ID after deletion can be accepted as a new event.
	again, err := s.Send(ctx, "1", testID(1), "2", []byte("opaque"))
	if err != nil || !again.CreatedAt.After(first.CreatedAt) {
		t.Fatalf("unexpected retained idempotency state: %v", err)
	}
	if len(store.events) != 2 {
		t.Fatal("unexpected queue contents")
	}
}

func TestQueueByteQuotaAndACKReleaseCapacity(t *testing.T) {
	ctx := context.Background()
	s, _ := testService(t)
	payload := bytes.Repeat([]byte{1}, MaxPayloadBytes)
	for i := 1; i <= MaxPendingBytesPerAccount/MaxPayloadBytes; i++ {
		if _, err := s.Send(ctx, "1", testID(i), "2", payload); err != nil {
			t.Fatal(err)
		}
	}
	if _, err := s.Send(ctx, "1", testID(100), "3", []byte{1}); !errors.Is(err, ErrCapacity) {
		t.Fatalf("sender quota bypass: %v", err)
	}
	if _, err := s.Send(ctx, "3", testID(101), "2", []byte{1}); !errors.Is(err, ErrCapacity) {
		t.Fatalf("recipient quota bypass: %v", err)
	}
	if err := s.Ack(ctx, "2", "1", testID(1)); err != nil {
		t.Fatal(err)
	}
	if _, err := s.Send(ctx, "3", testID(101), "2", payload); err != nil {
		t.Fatal("ACK did not release queue capacity", err)
	}
}

func TestConcurrentSameEventIsInsertedOnce(t *testing.T) {
	ctx := context.Background()
	s, store := testService(t)
	var wg sync.WaitGroup
	results := make(chan error, 20)
	for i := 0; i < 20; i++ {
		wg.Add(1)
		go func() { defer wg.Done(); _, err := s.Send(ctx, "1", testID(1), "2", []byte("opaque")); results <- err }()
	}
	wg.Wait()
	close(results)
	success := 0
	for err := range results {
		if err == nil {
			success++
		} else if !errors.Is(err, ErrDuplicate) {
			t.Fatal(err)
		}
	}
	if success != 1 || len(store.events) != 1 {
		t.Fatal("concurrent queue duplicate", success)
	}
}

type brokenStore struct{ Store }

func (s brokenStore) Enqueue(context.Context, Event) error {
	return errors.New("private database connection diagnostic")
}
func TestInputOwnershipCancellationAndSafeErrors(t *testing.T) {
	ctx := context.Background()
	s, store := testService(t)
	for _, pair := range [][2]string{{"01", "2"}, {"1", "02"}} {
		if _, err := s.Send(ctx, pair[0], testID(1), pair[1], []byte("opaque")); !errors.Is(err, ErrInvalid) {
			t.Fatal(err)
		}
	}
	for _, id := range []string{"", "00000000-0000-0000-0000-000000000000", "BAD", "00000000-0000-4000-8000-00000000000A"} {
		if _, err := s.Send(ctx, "1", id, "2", []byte("opaque")); !errors.Is(err, ErrInvalid) {
			t.Fatal(err)
		}
	}
	for _, actor := range []string{"1", "404"} {
		target := "404"
		if actor == "404" {
			target = "2"
		}
		if _, err := s.Send(ctx, actor, testID(1), target, []byte("opaque")); !errors.Is(err, ErrNotFound) {
			t.Fatal(err)
		}
	}
	for _, payload := range [][]byte{nil, make([]byte, MaxPayloadBytes+1)} {
		if _, err := s.Send(ctx, "1", testID(1), "2", payload); !errors.Is(err, ErrInvalid) {
			t.Fatal(err)
		}
	}
	cancelled, cancel := context.WithCancel(ctx)
	cancel()
	if _, err := s.Send(cancelled, "1", testID(1), "2", []byte("opaque")); !errors.Is(err, context.Canceled) {
		t.Fatal(err)
	}
	if len(store.events) != 0 {
		t.Fatal("failed operations mutated the queue")
	}
	broken, _ := New(brokenStore{Store: store})
	if _, err := broken.Send(ctx, "1", testID(1), "2", []byte("opaque")); err != ErrUnavailable {
		t.Fatal("storage diagnostics leaked", err)
	}
}

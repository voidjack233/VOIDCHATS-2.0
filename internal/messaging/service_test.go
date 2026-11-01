package messaging

import (
	"context"
	"encoding/base64"
	"encoding/json"
	"errors"
	"fmt"
	"strconv"
	"sync"
	"testing"
	"time"
)

var testContext = context.Background()

func testID(n int) string  { return fmt.Sprintf("00000000-0000-4000-8000-%012x", n) }
func wire(s string) string { return base64.StdEncoding.EncodeToString([]byte(s)) }
func setup(t *testing.T) (*Service, *MemoryStore) {
	t.Helper()
	store := NewMemoryStore([]Person{{VoidID: "101", Username: "alice", AuthPublicKey: wire("alice-public")}, {VoidID: "202", Username: "bob", AuthPublicKey: wire("bob-public")}, {VoidID: "303", Username: "carol", AuthPublicKey: wire("carol-public")}})
	s, e := NewService(store)
	if e != nil {
		t.Fatal(e)
	}
	return s, store
}
func requireCode(t *testing.T, e error, code string) {
	t.Helper()
	var app *Error
	if !errors.As(e, &app) || app.Code != code {
		t.Fatalf("expected %s, got %v", code, e)
	}
}
func requestInput(t *testing.T, s *Service, n int) CreateRequestInput {
	t.Helper()
	if _, e := s.Publish(testContext, "202", [][]byte{[]byte(fmt.Sprintf("public-package-%d", n))}); e != nil {
		t.Fatal(e)
	}
	claim, e := s.Claim(testContext, "101", "202", testID(n))
	if e != nil {
		t.Fatal(e)
	}
	return CreateRequestInput{ClientRequestID: testID(n + 100), ToVoidID: "202", KeyPackageID: claim.KeyPackageID, GroupIDBase64: wire("group"), WelcomeBase64: wire("opaque-welcome")}
}
func conversation(t *testing.T, s *Service, n int) Request {
	t.Helper()
	r, e := s.CreateRequest(testContext, "101", requestInput(t, s, n))
	if e != nil {
		t.Fatal(e)
	}
	r, e = s.Accept(testContext, "202", r.RequestID)
	if e != nil {
		t.Fatal(e)
	}
	return r
}
func ackAll(t *testing.T, s *Service, actor string) {
	t.Helper()
	for {
		page, e := s.Sync(testContext, actor, "0", 100)
		if e != nil {
			t.Fatal(e)
		}
		if len(page.Events) == 0 {
			return
		}
		acks := []Acknowledgment{}
		for _, event := range page.Events {
			acks = append(acks, Acknowledgment{event.EventID, "applied"})
		}
		if e = s.Ack(testContext, actor, acks); e != nil {
			t.Fatal(e)
		}
	}
}

func TestLookupOwnershipAndExactIDs(t *testing.T) {
	s, _ := setup(t)
	p, e := s.Lookup(testContext, "101", "202")
	if e != nil || p.Username != "bob" || p.AuthPublicKey != wire("bob-public") {
		t.Fatalf("lookup: %+v %v", p, e)
	}
	_, e = s.Lookup(testContext, "101", "0202")
	requireCode(t, e, "invalid_input")
	_, e = s.Lookup(testContext, "101", "404")
	requireCode(t, e, "not_found")
	_, e = s.Lookup(testContext, "404", "202")
	requireCode(t, e, "invalid_session")
}

func TestPackagesAtomicClaimsFingerprintAndExpiry(t *testing.T) {
	s, store := setup(t)
	published, e := s.Publish(testContext, "202", [][]byte{[]byte("one")})
	if e != nil {
		t.Fatal(e)
	}
	var wg sync.WaitGroup
	errs := make(chan error, 2)
	claims := make(chan Claim, 2)
	for i := 1; i <= 2; i++ {
		wg.Add(1)
		go func(n int) {
			defer wg.Done()
			c, e := s.Claim(testContext, "101", "202", testID(n))
			errs <- e
			if e == nil {
				claims <- c
			}
		}(i)
	}
	wg.Wait()
	close(errs)
	close(claims)
	success := 0
	for e := range errs {
		if e == nil {
			success++
		} else {
			requireCode(t, e, "key_package_unavailable")
		}
	}
	if success != 1 {
		t.Fatalf("successes=%d", success)
	}
	c := <-claims
	again, e := s.Claim(testContext, "101", "202", c.ClientClaimID)
	if e != nil || again.KeyPackageID != c.KeyPackageID {
		t.Fatalf("claim retry: %+v %v", again, e)
	}
	_, e = s.Claim(testContext, "101", "303", c.ClientClaimID)
	requireCode(t, e, "idempotency_conflict")
	duplicate, e := s.Publish(testContext, "202", [][]byte{[]byte("one")})
	if e != nil || duplicate[0].KeyPackageID != published[0].KeyPackageID || duplicate[0].State != "claimed" {
		t.Fatalf("dedup: %+v %v", duplicate, e)
	}
	now := time.Now().UTC()
	s.now = func() time.Time { return now }
	expired, e := s.Publish(testContext, "202", [][]byte{[]byte("expires")})
	if e != nil {
		t.Fatal(e)
	}
	s.now = func() time.Time { return now.Add(PackageTTL) }
	dup, e := s.Publish(testContext, "202", [][]byte{[]byte("expires")})
	if e != nil || dup[0].State != "expired" || dup[0].KeyPackageID != expired[0].KeyPackageID {
		t.Fatalf("expiry: %+v %v", dup, e)
	}
	_, e = s.Claim(testContext, "101", "202", testID(3))
	requireCode(t, e, "key_package_unavailable")
	restarted, _ := NewService(store)
	retry, e := restarted.Claim(testContext, "101", "202", c.ClientClaimID)
	if e != nil || retry.KeyPackageID != c.KeyPackageID {
		t.Fatal("claim retry lost after service restart", e)
	}
}

func TestRequestsStableRetriesOppositeDirectionAndOneUsePackages(t *testing.T) {
	s, _ := setup(t)
	in := requestInput(t, s, 1)
	r, e := s.CreateRequest(testContext, "101", in)
	if e != nil {
		t.Fatal(e)
	}
	again, e := s.CreateRequest(testContext, "101", in)
	if e != nil || again.RequestID != r.RequestID {
		t.Fatalf("request retry: %+v %v", again, e)
	}
	changed := in
	changed.WelcomeBase64 = wire("different")
	_, e = s.CreateRequest(testContext, "101", changed)
	requireCode(t, e, "idempotency_conflict")
	_, e = s.Accept(testContext, "101", r.RequestID)
	requireCode(t, e, "forbidden")
	_, e = s.Accept(testContext, "303", r.RequestID)
	requireCode(t, e, "not_found")
	if _, e = s.Publish(testContext, "101", [][]byte{[]byte("alice-package")}); e != nil {
		t.Fatal(e)
	}
	c, e := s.Claim(testContext, "202", "101", testID(9))
	if e != nil {
		t.Fatal(e)
	}
	_, e = s.CreateRequest(testContext, "202", CreateRequestInput{ClientRequestID: testID(109), ToVoidID: "101", KeyPackageID: c.KeyPackageID, GroupIDBase64: wire("other-group"), WelcomeBase64: wire("other-welcome")})
	requireCode(t, e, "conversation_exists")
	if _, e = s.Decline(testContext, "202", r.RequestID); e != nil {
		t.Fatal(e)
	}
	if _, e = s.Decline(testContext, "202", r.RequestID); e != nil {
		t.Fatal(e)
	}
	_, e = s.Accept(testContext, "202", r.RequestID)
	requireCode(t, e, "request_not_pending")
	in.ClientRequestID = testID(110)
	_, e = s.CreateRequest(testContext, "101", in)
	requireCode(t, e, "key_package_used")
	fresh := requestInput(t, s, 2)
	if _, e = s.CreateRequest(testContext, "101", fresh); e != nil {
		t.Fatal(e)
	}
}

func TestConcurrentOppositeRequestsCreateOnlyOnePair(t *testing.T) {
	s, _ := setup(t)
	a := requestInput(t, s, 1)
	if _, e := s.Publish(testContext, "101", [][]byte{[]byte("alice-package")}); e != nil {
		t.Fatal(e)
	}
	c, e := s.Claim(testContext, "202", "101", testID(2))
	if e != nil {
		t.Fatal(e)
	}
	b := CreateRequestInput{ClientRequestID: testID(200), ToVoidID: "101", KeyPackageID: c.KeyPackageID, GroupIDBase64: wire("b"), WelcomeBase64: wire("welcome-b")}
	var wg sync.WaitGroup
	errs := make(chan error, 2)
	for _, v := range []struct {
		actor string
		in    CreateRequestInput
	}{{"101", a}, {"202", b}} {
		wg.Add(1)
		go func(actor string, in CreateRequestInput) {
			defer wg.Done()
			_, e := s.CreateRequest(testContext, actor, in)
			errs <- e
		}(v.actor, v.in)
	}
	wg.Wait()
	close(errs)
	success := 0
	for e := range errs {
		if e == nil {
			success++
		} else {
			requireCode(t, e, "conversation_exists")
		}
	}
	if success != 1 {
		t.Fatalf("successes=%d", success)
	}
}

func TestDurableOfflineSyncOrderingAndAcknowledgmentReceipts(t *testing.T) {
	s, store := setup(t)
	r := conversation(t, s, 1)
	ackAll(t, s, "101")
	ackAll(t, s, "202")
	in := SendInput{ClientMessageID: testID(300), Kind: "application", CiphertextBase64: wire("opaque-one")}
	first, e := s.Send(testContext, "101", r.ConversationID, in)
	if e != nil || first.Sequence != "1" {
		t.Fatalf("send: %+v %v", first, e)
	}
	same, e := s.Send(testContext, "101", r.ConversationID, in)
	if e != nil || same.MessageID != first.MessageID {
		t.Fatalf("send retry: %+v %v", same, e)
	}
	changed := in
	changed.CiphertextBase64 = wire("changed")
	_, e = s.Send(testContext, "101", r.ConversationID, changed)
	requireCode(t, e, "idempotency_conflict")
	changed = in
	changed.Kind = "commit"
	_, e = s.Send(testContext, "101", r.ConversationID, changed)
	requireCode(t, e, "idempotency_conflict")
	_, e = s.Send(testContext, "101", testID(900), in)
	requireCode(t, e, "idempotency_conflict")
	_, e = s.Send(testContext, "303", r.ConversationID, SendInput{testID(301), "application", wire("opaque")})
	requireCode(t, e, "not_found")
	second, e := s.Send(testContext, "202", r.ConversationID, SendInput{testID(301), "commit", wire("opaque-commit")})
	if e != nil || second.Sequence != "2" {
		t.Fatal(second, e)
	}
	third, e := s.Send(testContext, "101", r.ConversationID, SendInput{testID(302), "application", wire("opaque-two")})
	if e != nil || third.Sequence != "3" {
		t.Fatal(third, e)
	}
	restarted, _ := NewService(store)
	page, e := restarted.Sync(testContext, "202", "0", 1)
	if e != nil || len(page.Events) != 1 || !page.HasMore {
		t.Fatal(page, e)
	}
	event := page.Events[0]
	var got Message
	if e = json.Unmarshal(event.Payload, &got); e != nil || got.MessageID != first.MessageID {
		t.Fatal(got, e)
	}
	retry, e := restarted.Sync(testContext, "202", "0", 1)
	if e != nil || retry.Events[0].EventID != event.EventID {
		t.Fatal("fetch accidentally ACKed", e)
	}
	next, e := restarted.Sync(testContext, "202", page.NextCursor, 1)
	if e != nil || len(next.Events) != 1 || next.Events[0].Cursor <= event.Cursor {
		t.Fatal(next, e)
	}
	requireCode(t, restarted.Ack(testContext, "303", []Acknowledgment{{event.EventID, "applied"}}), "not_found")
	if e = restarted.Ack(testContext, "202", []Acknowledgment{{event.EventID, "applied"}}); e != nil {
		t.Fatal(e)
	}
	if e = restarted.Ack(testContext, "202", []Acknowledgment{{event.EventID, "applied"}}); e != nil {
		t.Fatal(e)
	}
	requireCode(t, restarted.Ack(testContext, "202", []Acknowledgment{{event.EventID, "rejected"}}), "ack_conflict")
	if e = restarted.Ack(testContext, "202", []Acknowledgment{{next.Events[0].EventID, "rejected"}}); e != nil {
		t.Fatal(e)
	}
	receipts, e := restarted.Sync(testContext, "101", "0", 100)
	if e != nil {
		t.Fatal(e)
	}
	kinds := map[string]int{}
	for _, ev := range receipts.Events {
		kinds[ev.Type]++
	}
	if kinds["message.delivered"] != 1 || kinds["message.rejected"] != 1 {
		t.Fatalf("receipt types: %v", kinds)
	}
	empty, e := restarted.Sync(testContext, "202", "0", 100)
	if e != nil || len(empty.Events) != 0 {
		t.Fatal(empty, e)
	}
}

func TestAckBatchIsAtomic(t *testing.T) {
	s, _ := setup(t)
	conversation(t, s, 1)
	page, e := s.Sync(testContext, "202", "0", 100)
	if e != nil {
		t.Fatal(e)
	}
	first := page.Events[0]
	requireCode(t, s.Ack(testContext, "202", []Acknowledgment{{first.EventID, "applied"}, {testID(999), "applied"}}), "not_found")
	page, e = s.Sync(testContext, "202", "0", 100)
	if e != nil || len(page.Events) != 2 {
		t.Fatal("partial ACK committed", page, e)
	}
}

func TestBlockCancelsQueuedDeliveryAndReleasesReservations(t *testing.T) {
	s, store := setup(t)
	r := conversation(t, s, 1)
	ackAll(t, s, "101")
	ackAll(t, s, "202")
	if _, e := s.Send(testContext, "101", r.ConversationID, SendInput{testID(300), "application", wire("opaque")}); e != nil {
		t.Fatal(e)
	}
	page, e := s.Sync(testContext, "202", "0", 100)
	if e != nil || len(page.Events) != 1 {
		t.Fatal(page, e)
	}
	if e = s.Block(testContext, "202", "101"); e != nil {
		t.Fatal(e)
	}
	after, e := s.Sync(testContext, "202", "0", 100)
	if e != nil || len(after.Events) != 1 || after.Events[0].Type != "block.updated" {
		t.Fatal(after, e)
	}
	peer, e := s.Sync(testContext, "101", "0", 100)
	if e != nil || len(peer.Events) != 0 {
		t.Fatal("block leaked to peer", peer, e)
	}
	requireCode(t, s.Ack(testContext, "202", []Acknowledgment{{page.Events[0].EventID, "applied"}}), "ack_conflict")
	_, e = s.Send(testContext, "101", r.ConversationID, SendInput{testID(301), "application", wire("blocked")})
	requireCode(t, e, "not_found")
	_, e = s.Claim(testContext, "101", "202", testID(302))
	requireCode(t, e, "not_found")
	_, e = s.Lookup(testContext, "101", "202")
	requireCode(t, e, "not_found")
	if e = s.Block(testContext, "101", "202"); e != nil {
		t.Fatal(e)
	}
	own, e := s.Sync(testContext, "101", "0", 100)
	if e != nil || len(own.Events) != 1 || own.Events[0].Type != "block.updated" {
		t.Fatal("reciprocal block event missing", own, e)
	}
	if e = store.Run(testContext, func(tx Tx) error {
		n, e := tx.ReceiptReservations("101")
		if n != 0 {
			t.Fatalf("reservation leaked=%d", n)
		}
		return e
	}); e != nil {
		t.Fatal(e)
	}
}

func TestBlockCancelsPendingRequest(t *testing.T) {
	s, _ := setup(t)
	r, e := s.CreateRequest(testContext, "101", requestInput(t, s, 1))
	if e != nil {
		t.Fatal(e)
	}
	if e = s.Block(testContext, "202", "101"); e != nil {
		t.Fatal(e)
	}
	_, e = s.Accept(testContext, "202", r.RequestID)
	requireCode(t, e, "request_not_pending")
	page, e := s.ListRequests(testContext, "202", "", 50)
	if e != nil || len(page.Requests) != 0 || page.NextCursor != r.RequestID {
		t.Fatal(page, e)
	}
	peerPage, e := s.ListRequests(testContext, "101", "", 50)
	if e != nil || len(peerPage.Requests) != 0 {
		t.Fatal("blocked request Welcome leaked through list", peerPage, e)
	}
}

func TestReservedReceiptAllowsACKWhenSenderQueueAtCapacity(t *testing.T) {
	s, store := setup(t)
	r := conversation(t, s, 1)
	ackAll(t, s, "101")
	ackAll(t, s, "202")
	if _, e := s.Send(testContext, "101", r.ConversationID, SendInput{testID(300), "application", wire("opaque")}); e != nil {
		t.Fatal(e)
	}
	if e := store.Run(testContext, func(tx Tx) error {
		for i := 0; i < MaxPendingEvents-1; i++ {
			if e := s.emit(tx, "101", "test.event", map[string]string{"value": "small"}); e != nil {
				return e
			}
		}
		requireCode(t, s.emit(tx, "101", "test.event", map[string]string{}), "messaging_capacity")
		return nil
	}); e != nil {
		t.Fatal(e)
	}
	page, e := s.Sync(testContext, "202", "0", 10)
	if e != nil {
		t.Fatal(e)
	}
	if e = s.Ack(testContext, "202", []Acknowledgment{{page.Events[0].EventID, "applied"}}); e != nil {
		t.Fatal("reserved receipt could not commit", e)
	}
	if e = store.Run(testContext, func(tx Tx) error {
		_, pending, _, _, e := tx.EventCounts("101")
		if pending != MaxPendingEvents {
			t.Fatalf("pending=%d", pending)
		}
		n, _ := tx.ReceiptReservations("101")
		if n != 0 {
			t.Fatalf("reservation=%d", n)
		}
		return e
	}); e != nil {
		t.Fatal(e)
	}
}

func TestBackpressureRollsBackMessageAndSequence(t *testing.T) {
	s, store := setup(t)
	r := conversation(t, s, 1)
	ackAll(t, s, "101")
	ackAll(t, s, "202")
	if e := store.Run(testContext, func(tx Tx) error {
		for i := 0; i < MaxPendingEvents; i++ {
			if e := s.emit(tx, "202", "test.event", map[string]string{}); e != nil {
				return e
			}
		}
		return nil
	}); e != nil {
		t.Fatal(e)
	}
	in := SendInput{testID(300), "application", wire("opaque")}
	_, e := s.Send(testContext, "101", r.ConversationID, in)
	requireCode(t, e, "messaging_capacity")
	ackAll(t, s, "202")
	m, e := s.Send(testContext, "101", r.ConversationID, in)
	if e != nil || m.Sequence != "1" {
		t.Fatal("failed send consumed sequence or ID", m, e)
	}
}

func TestBlockSucceedsWhenPrivateEventQuotaExhausted(t *testing.T) {
	s, store := setup(t)
	if e := store.Run(testContext, func(tx Tx) error {
		for i := 0; i < MaxPendingEvents; i++ {
			if e := s.emit(tx, "101", "test.event", map[string]string{}); e != nil {
				return e
			}
		}
		return nil
	}); e != nil {
		t.Fatal(e)
	}
	if e := s.Block(testContext, "101", "202"); e != nil {
		t.Fatal("capacity prevented block", e)
	}
	_, e := s.Lookup(testContext, "101", "202")
	requireCode(t, e, "not_found")
}

func TestBoundedInputsAndBytes(t *testing.T) {
	s, _ := setup(t)
	_, e := s.Publish(testContext, "101", make([][]byte, MaxPackageBatch+1))
	requireCode(t, e, "invalid_input")
	_, e = s.Publish(testContext, "101", [][]byte{make([]byte, MaxOpaqueBytes+1)})
	requireCode(t, e, "payload_too_large")
	_, e = s.Send(testContext, "101", testID(1), SendInput{testID(2), "application", "YQ"})
	requireCode(t, e, "invalid_input")
	_, e = s.Send(testContext, "101", testID(1), SendInput{testID(2), "application", "YQ==\n"})
	requireCode(t, e, "invalid_input")
	_, e = s.Sync(testContext, "101", "01", 10)
	requireCode(t, e, "invalid_input")
	_, e = s.Sync(testContext, "101", "-1", 10)
	requireCode(t, e, "invalid_input")
	_, e = s.Sync(testContext, "101", "0", 101)
	requireCode(t, e, "invalid_input")
	requireCode(t, s.Ack(testContext, "101", []Acknowledgment{{testID(1), "read"}}), "invalid_input")
	for i := 0; i < 11; i++ {
		p := make([]byte, MaxOpaqueBytes)
		p[0] = byte(i)
		if _, e = s.Publish(testContext, "101", [][]byte{p}); e != nil {
			t.Fatal("unexpected package quota", i, e)
		}
	}
	p := make([]byte, MaxOpaqueBytes)
	p[0] = 99
	_, e = s.Publish(testContext, "101", [][]byte{p})
	requireCode(t, e, "messaging_capacity")
}

func TestSyncPageRespectsFourMiBEncodedLimit(t *testing.T) {
	s, store := setup(t)
	big := wire(string(make([]byte, MaxOpaqueBytes)))
	if e := store.Run(testContext, func(tx Tx) error {
		for i := 0; i < 3; i++ {
			if e := s.emit(tx, "202", "test.event", map[string]string{"ciphertext_base64": big}); e != nil {
				return e
			}
		}
		return nil
	}); e != nil {
		t.Fatal(e)
	}
	page, e := s.Sync(testContext, "202", "0", 100)
	if e != nil || len(page.Events) != 2 || !page.HasMore {
		t.Fatal(len(page.Events), page.HasMore, e)
	}
	encoded, _ := json.Marshal(page)
	if len(encoded) > MaxSyncBytes {
		t.Fatalf("page bytes=%d", len(encoded))
	}
}

func TestConcurrentMessagesHaveUniqueOrderedSequences(t *testing.T) {
	s, _ := setup(t)
	r := conversation(t, s, 1)
	var wg sync.WaitGroup
	results := make(chan Message, 20)
	errs := make(chan error, 20)
	for i := 0; i < 20; i++ {
		wg.Add(1)
		go func(n int) {
			defer wg.Done()
			m, e := s.Send(testContext, "101", r.ConversationID, SendInput{testID(300 + n), "application", wire("opaque")})
			results <- m
			errs <- e
		}(i)
	}
	wg.Wait()
	close(results)
	close(errs)
	for e := range errs {
		if e != nil {
			t.Fatal(e)
		}
	}
	sequences := map[string]bool{}
	for m := range results {
		sequences[m.Sequence] = true
	}
	for i := 1; i <= 20; i++ {
		if !sequences[strconv.Itoa(i)] {
			t.Fatalf("missing sequence %d", i)
		}
	}
}

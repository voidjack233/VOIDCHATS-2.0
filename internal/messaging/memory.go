package messaging

import (
	"context"
	"encoding/json"
	"errors"
	"maps"
	"sort"
	"strconv"
	"sync"
	"time"
)

// MemoryStore is a synthetic-account, transactional store for local tests. The
// production server uses PostgreSQL. Recreating a Service over the same store
// preserves all pending delivery, client-ID bindings and acknowledgment state.
type MemoryStore struct {
	mu    sync.Mutex
	state memoryState
}
type memoryState struct {
	people        map[string]Person
	packages      map[string]PackageRow
	requests      map[string]Request
	conversations map[string]ConversationRow
	blocks        map[string]time.Time
	messages      map[string]Message
	events        map[string]EventRow
	cursors       map[string]int64
	reservations  map[string]int
}
type memoryTx struct{ memoryState }

func NewMemoryStore(people []Person) *MemoryStore {
	s := &MemoryStore{state: memoryState{people: map[string]Person{}, packages: map[string]PackageRow{}, requests: map[string]Request{}, conversations: map[string]ConversationRow{}, blocks: map[string]time.Time{}, messages: map[string]Message{}, events: map[string]EventRow{}, cursors: map[string]int64{}, reservations: map[string]int{}}}
	for _, p := range people {
		s.state.people[p.VoidID] = p
	}
	return s
}
func (s *MemoryStore) Validate(ctx context.Context) error { return ctx.Err() }
func (s *MemoryStore) Run(ctx context.Context, fn func(Tx) error) error {
	if e := ctx.Err(); e != nil {
		return e
	}
	s.mu.Lock()
	defer s.mu.Unlock()
	v := s.state
	tx := &memoryTx{memoryState{people: maps.Clone(v.people), packages: maps.Clone(v.packages), requests: maps.Clone(v.requests), conversations: maps.Clone(v.conversations), blocks: maps.Clone(v.blocks), messages: maps.Clone(v.messages), events: maps.Clone(v.events), cursors: maps.Clone(v.cursors), reservations: maps.Clone(v.reservations)}}
	if e := fn(tx); e != nil {
		return e
	}
	if e := ctx.Err(); e != nil {
		return e
	}
	s.state = tx.memoryState
	return nil
}
func (t *memoryTx) Person(id string) (Person, bool, error) { v, ok := t.people[id]; return v, ok, nil }
func (t *memoryTx) Package(id string) (PackageRow, bool, error) {
	v, ok := t.packages[id]
	return v, ok, nil
}
func (t *memoryTx) PackageByFingerprint(owner, fp string) (PackageRow, bool, error) {
	for _, p := range t.packages {
		if p.Owner == owner && p.Fingerprint == fp {
			return p, true, nil
		}
	}
	return PackageRow{}, false, nil
}
func (t *memoryTx) AvailablePackage(owner string, now time.Time) (PackageRow, bool, error) {
	var selected PackageRow
	for _, p := range t.packages {
		if p.Owner == owner && p.ClaimedBy == "" && now.Before(p.ExpiresAt) && (selected.ID == "" || p.ID < selected.ID) {
			selected = p
		}
	}
	return selected, selected.ID != "", nil
}
func (t *memoryTx) ClaimedPackage(actor, client string) (PackageRow, bool, error) {
	for _, p := range t.packages {
		if p.ClaimedBy == actor && p.ClaimID == client {
			return p, true, nil
		}
	}
	return PackageRow{}, false, nil
}
func (t *memoryTx) PackageCount(owner string) (int, int, error) {
	n, size := 0, 0
	for _, p := range t.packages {
		if p.Owner == owner {
			n++
			size += len(p.Data)
		}
	}
	return n, size, nil
}
func (t *memoryTx) SavePackage(p PackageRow) error { t.packages[p.ID] = p; return nil }
func (t *memoryTx) Request(id string) (Request, bool, error) {
	v, ok := t.requests[id]
	return v, ok, nil
}
func (t *memoryTx) RequestByClient(actor, client string) (Request, bool, error) {
	for _, r := range t.requests {
		if r.Sender.VoidID == actor && r.ClientRequestID == client {
			return r, true, nil
		}
	}
	return Request{}, false, nil
}
func samePair(r Request, a, b string) bool {
	return r.Sender.VoidID == a && r.Recipient.VoidID == b || r.Sender.VoidID == b && r.Recipient.VoidID == a
}
func (t *memoryTx) ActivePair(a, b string) (Request, bool, error) {
	for _, r := range t.requests {
		if (r.Status == "pending" || r.Status == "accepted") && samePair(r, a, b) {
			return r, true, nil
		}
	}
	return Request{}, false, nil
}
func (t *memoryTx) RequestCount(owner string) (int, error) {
	n := 0
	for _, r := range t.requests {
		if r.Sender.VoidID == owner || r.Recipient.VoidID == owner {
			n++
		}
	}
	return n, nil
}
func (t *memoryTx) Requests(owner, after string, limit int) ([]Request, error) {
	out := []Request{}
	for _, r := range t.requests {
		if (r.Sender.VoidID == owner || r.Recipient.VoidID == owner) && r.RequestID > after {
			out = append(out, r)
		}
	}
	sort.Slice(out, func(i, j int) bool { return out[i].RequestID < out[j].RequestID })
	if len(out) > limit {
		out = out[:limit]
	}
	return out, nil
}
func (t *memoryTx) SaveRequest(r Request) error { t.requests[r.RequestID] = r; return nil }
func (t *memoryTx) Conversation(id string) (ConversationRow, bool, error) {
	v, ok := t.conversations[id]
	return v, ok, nil
}
func (t *memoryTx) Conversations(owner, after string, limit int) ([]Conversation, error) {
	out := []Conversation{}
	for _, r := range t.conversations {
		if _, ok := peer(r.Conversation, owner); ok && r.Conversation.ConversationID > after {
			out = append(out, r.Conversation)
		}
	}
	sort.Slice(out, func(i, j int) bool { return out[i].ConversationID < out[j].ConversationID })
	if len(out) > limit {
		out = out[:limit]
	}
	return out, nil
}
func (t *memoryTx) SaveConversation(c ConversationRow) error {
	t.conversations[c.Conversation.ConversationID] = c
	return nil
}
func blockKey(a, b string) string { return a + ":" + b }
func (t *memoryTx) Blocked(a, b string) (bool, error) {
	_, x := t.blocks[blockKey(a, b)]
	_, y := t.blocks[blockKey(b, a)]
	return x || y, nil
}
func (t *memoryTx) SaveBlock(a, b string, at time.Time) (bool, error) {
	_, exists := t.blocks[blockKey(a, b)]
	t.blocks[blockKey(a, b)] = at
	return !exists, nil
}
func (t *memoryTx) MessageByClient(actor, client string) (Message, bool, error) {
	for _, m := range t.messages {
		if m.SenderVoidID == actor && m.ClientMessageID == client {
			return m, true, nil
		}
	}
	return Message{}, false, nil
}
func (t *memoryTx) MessageCount(conv string) (int, int, error) {
	n, size := 0, 0
	for _, m := range t.messages {
		if m.ConversationID == conv {
			n++
			size += len(m.CiphertextBase64)
		}
	}
	return n, size, nil
}
func (t *memoryTx) SaveMessage(m Message) error { t.messages[m.MessageID] = m; return nil }
func (t *memoryTx) Event(owner, id string) (EventRow, bool, error) {
	e, ok := t.events[id]
	return e, ok && e.Owner == owner, nil
}
func (t *memoryTx) Events(owner string, after int64, limit int) ([]EventRow, error) {
	out := []EventRow{}
	for _, e := range t.events {
		v, _ := strconv.ParseInt(e.Event.Cursor, 10, 64)
		if e.Owner == owner && e.Outcome == "" && v > after {
			out = append(out, e)
		}
	}
	sort.Slice(out, func(i, j int) bool {
		a, _ := strconv.ParseInt(out[i].Event.Cursor, 10, 64)
		b, _ := strconv.ParseInt(out[j].Event.Cursor, 10, 64)
		return a < b
	})
	if len(out) > limit {
		out = out[:limit]
	}
	return out, nil
}
func (t *memoryTx) EventCounts(owner string) (int, int, int, int, error) {
	total, pending, totalBytes, pendingBytes := 0, 0, 0, 0
	for _, e := range t.events {
		if e.Owner == owner {
			total++
			b, _ := json.Marshal(e.Event)
			totalBytes += len(b)
			if e.Outcome == "" {
				pending++
				pendingBytes += len(b)
			}
		}
	}
	return total, pending, totalBytes, pendingBytes, nil
}
func (t *memoryTx) ReceiptReservations(owner string) (int, error) { return t.reservations[owner], nil }
func (t *memoryTx) ReserveReceipt(owner string, delta int) error {
	n := t.reservations[owner] + delta
	if n < 0 {
		return errors.New("invalid receipt reservation")
	}
	t.reservations[owner] = n
	return nil
}
func (t *memoryTx) NextCursor(owner string) (int64, error) {
	t.cursors[owner]++
	return t.cursors[owner], nil
}
func (t *memoryTx) SaveEvent(e EventRow) error { t.events[e.Event.EventID] = e; return nil }

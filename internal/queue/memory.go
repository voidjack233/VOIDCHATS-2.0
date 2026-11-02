package queue

import (
	"context"
	"sort"
	"sync"
	"time"
)

// MemoryStore is for synthetic local tests and demos. Production uses the
// PostgreSQL queue. Reusing this store across Service instances models pending
// records surviving a server restart; it is not disk persistence.
type MemoryStore struct {
	mu     sync.Mutex
	people map[string]Person
	events map[string]Event
}

func NewMemoryStore(people []Person) *MemoryStore {
	s := &MemoryStore{people: map[string]Person{}, events: map[string]Event{}}
	for _, p := range people {
		s.people[p.VoidID] = p
	}
	return s
}

func eventKey(sender, id string) string { return sender + ":" + id }

func (s *MemoryStore) Validate(ctx context.Context) error { return ctx.Err() }

func (s *MemoryStore) Lookup(ctx context.Context, id string) (Person, error) {
	s.mu.Lock()
	defer s.mu.Unlock()
	if err := ctx.Err(); err != nil {
		return Person{}, err
	}
	p, ok := s.people[id]
	if !ok {
		return Person{}, ErrNotFound
	}
	return p, nil
}

func (s *MemoryStore) Enqueue(ctx context.Context, event Event) error {
	s.mu.Lock()
	defer s.mu.Unlock()
	if err := ctx.Err(); err != nil {
		return err
	}
	if _, ok := s.people[event.SenderVoidID]; !ok {
		return ErrNotFound
	}
	if _, ok := s.people[event.RecipientVoidID]; !ok {
		return ErrNotFound
	}
	key := eventKey(event.SenderVoidID, event.EventID)
	if _, ok := s.events[key]; ok {
		return ErrDuplicate
	}
	for _, account := range []string{event.SenderVoidID, event.RecipientVoidID} {
		incoming, outgoing, inBytes, outBytes := 0, 0, 0, 0
		for _, old := range s.events {
			if !old.ExpiresAt.After(event.CreatedAt) {
				continue
			}
			if old.SenderVoidID == account {
				outgoing++
				outBytes += len(old.Payload)
			}
			if old.RecipientVoidID == account {
				incoming++
				inBytes += len(old.Payload)
			}
		}
		if event.SenderVoidID == account && (outgoing >= MaxPendingPerAccount || outBytes+len(event.Payload) > MaxPendingBytesPerAccount) {
			return ErrCapacity
		}
		if event.RecipientVoidID == account && (incoming >= MaxPendingPerAccount || inBytes+len(event.Payload) > MaxPendingBytesPerAccount) {
			return ErrCapacity
		}
	}
	event.Payload = append([]byte(nil), event.Payload...)
	s.events[key] = event
	return nil
}

func (s *MemoryStore) Pending(ctx context.Context, recipient string, now time.Time, limit int) ([]Event, error) {
	s.mu.Lock()
	defer s.mu.Unlock()
	if err := ctx.Err(); err != nil {
		return nil, err
	}
	if _, ok := s.people[recipient]; !ok {
		return nil, ErrNotFound
	}
	rows := []Event{}
	for _, event := range s.events {
		if event.RecipientVoidID == recipient && event.ExpiresAt.After(now) {
			event.Payload = append([]byte(nil), event.Payload...)
			rows = append(rows, event)
		}
	}
	sort.Slice(rows, func(i, j int) bool {
		if !rows[i].CreatedAt.Equal(rows[j].CreatedAt) {
			return rows[i].CreatedAt.Before(rows[j].CreatedAt)
		}
		if rows[i].SenderVoidID != rows[j].SenderVoidID {
			return numericID(rows[i].SenderVoidID) < numericID(rows[j].SenderVoidID)
		}
		return rows[i].EventID < rows[j].EventID
	})
	if len(rows) > limit {
		rows = rows[:limit]
	}
	return rows, nil
}

func (s *MemoryStore) Ack(ctx context.Context, recipient, sender, id string) error {
	s.mu.Lock()
	defer s.mu.Unlock()
	if err := ctx.Err(); err != nil {
		return err
	}
	key := eventKey(sender, id)
	if event, ok := s.events[key]; ok && event.RecipientVoidID == recipient {
		delete(s.events, key)
	}
	return nil
}

func (s *MemoryStore) Expire(ctx context.Context, now time.Time, limit int) (int64, error) {
	s.mu.Lock()
	defer s.mu.Unlock()
	if err := ctx.Err(); err != nil {
		return 0, err
	}
	var removed int64
	for key, event := range s.events {
		if removed >= int64(limit) {
			break
		}
		if !event.ExpiresAt.After(now) {
			delete(s.events, key)
			removed++
		}
	}
	return removed, nil
}

var _ Store = (*MemoryStore)(nil)

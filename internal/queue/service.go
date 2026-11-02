// Package queue implements temporary delivery of opaque encrypted events.
// VOID-IDs are public routing addresses. Callers obtain the sender from an
// authenticated connection; the service never accepts authentication secrets.
package queue

import (
	"context"
	"errors"
	"strconv"
	"time"

	"void-nothingness/internal/auth"
)

const (
	Retention                 = 30 * 24 * time.Hour
	MaxPayloadBytes           = 1 << 20
	MaxPendingPerAccount      = 1000
	MaxPendingBytesPerAccount = 16 << 20
	MaxBatch                  = 100
)

var (
	ErrInvalid     = errors.New("invalid relay input")
	ErrNotFound    = errors.New("account unavailable")
	ErrDuplicate   = errors.New("event identifier is already queued")
	ErrCapacity    = errors.New("temporary queue capacity reached")
	ErrUnavailable = errors.New("temporary queue unavailable")
)

type Person struct {
	VoidID        string `json:"void_id"`
	AuthPublicKey string `json:"auth_public_key"`
}

// Event is exactly one queue record. Payload is never inspected as MLS or
// application content, and no delivered/history/tombstone state is stored.
type Event struct {
	EventID         string    `json:"event_id"`
	SenderVoidID    string    `json:"sender_void_id"`
	RecipientVoidID string    `json:"recipient_void_id"`
	Payload         []byte    `json:"-"`
	CreatedAt       time.Time `json:"created_at"`
	ExpiresAt       time.Time `json:"expires_at"`
}

// Store.Enqueue returns only after persistence commits. Pending excludes
// expired rows. Ack deletes only rows addressed to its authenticated recipient.
// Implementations bound pending count/bytes per sender and recipient atomically.
type Store interface {
	Lookup(context.Context, string) (Person, error)
	Enqueue(context.Context, Event) error
	Pending(context.Context, string, time.Time, int) ([]Event, error)
	Ack(context.Context, string, string, string) error
	Expire(context.Context, time.Time, int) (int64, error)
	Validate(context.Context) error
}

type Service struct {
	store Store
	now   func() time.Time
}

func New(store Store) (*Service, error) {
	if store == nil {
		return nil, ErrUnavailable
	}
	return &Service{store: store, now: time.Now}, nil
}

func ValidEventID(id string) bool {
	if len(id) != 36 || id == "00000000-0000-0000-0000-000000000000" {
		return false
	}
	for i, c := range id {
		if i == 8 || i == 13 || i == 18 || i == 23 {
			if c != '-' {
				return false
			}
			continue
		}
		if !(c >= '0' && c <= '9' || c >= 'a' && c <= 'f') {
			return false
		}
	}
	return true
}

func (s *Service) Lookup(ctx context.Context, actor, target string) (Person, error) {
	if !auth.ValidVoidID(actor) || !auth.ValidVoidID(target) {
		return Person{}, ErrInvalid
	}
	if _, err := s.store.Lookup(ctx, actor); err != nil {
		return Person{}, safeError(ctx, err)
	}
	p, err := s.store.Lookup(ctx, target)
	return p, safeError(ctx, err)
}

// Send accepts no timestamps. Go assigns both from one server clock reading.
// Duplicate IDs still present in the queue are rejected; advanced retries and
// idempotency after deletion are deliberately outside this first version.
func (s *Service) Send(ctx context.Context, actor, eventID, target string, payload []byte) (Event, error) {
	if !auth.ValidVoidID(actor) || !auth.ValidVoidID(target) || !ValidEventID(eventID) || len(payload) == 0 || len(payload) > MaxPayloadBytes {
		return Event{}, ErrInvalid
	}
	now := s.now().UTC().Truncate(time.Microsecond)
	event := Event{EventID: eventID, SenderVoidID: actor, RecipientVoidID: target, Payload: append([]byte(nil), payload...), CreatedAt: now, ExpiresAt: now.Add(Retention)}
	if err := s.store.Enqueue(ctx, event); err != nil {
		return Event{}, safeError(ctx, err)
	}
	return event, nil
}

func (s *Service) Pending(ctx context.Context, actor string, limit int) ([]Event, error) {
	if !auth.ValidVoidID(actor) || limit < 1 || limit > MaxBatch {
		return nil, ErrInvalid
	}
	rows, err := s.store.Pending(ctx, actor, s.now().UTC(), limit)
	return rows, safeError(ctx, err)
}

func (s *Service) Ack(ctx context.Context, actor, sender, eventID string) error {
	if !auth.ValidVoidID(actor) || !auth.ValidVoidID(sender) || !ValidEventID(eventID) {
		return ErrInvalid
	}
	return safeError(ctx, s.store.Ack(ctx, actor, sender, eventID))
}

func (s *Service) Cleanup(ctx context.Context, limit int) (int64, error) {
	if limit < 1 || limit > 1000 {
		return 0, ErrInvalid
	}
	n, err := s.store.Expire(ctx, s.now().UTC(), limit)
	return n, safeError(ctx, err)
}

func (s *Service) Ready(ctx context.Context) error { return safeError(ctx, s.store.Validate(ctx)) }

func safeError(ctx context.Context, err error) error {
	if err == nil {
		return nil
	}
	if ctx.Err() != nil {
		return ctx.Err()
	}
	for _, known := range []error{ErrInvalid, ErrNotFound, ErrDuplicate, ErrCapacity} {
		if errors.Is(err, known) {
			return known
		}
	}
	return ErrUnavailable
}

func numericID(id string) int64 { n, _ := strconv.ParseInt(id, 10, 64); return n }

package account

import (
	"context"
	"crypto/ed25519"
	"crypto/rand"
	"encoding/base64"
	"errors"
	"net/url"
	"strconv"
	"strings"
	"sync"
	"time"
)

var ErrDeleted = errors.New("account permanently deleted")

// A nil deletion timestamp means Active; an absent row means Unknown.
type Status struct {
	VoidID    string     `json:"void_id"`
	State     string     `json:"state"`
	DeletedAt *time.Time `json:"deleted_at,omitempty"`
}

type LifecycleStore interface {
	DeletionKey(context.Context, string) ([]byte, error)
	Delete(context.Context, string) (Status, error)
	Status(context.Context, string, string) (Status, error)
	WithActive(context.Context, []string, func() error) error
}

type DeletionService struct {
	store    LifecycleStore
	audience string
	now      func() time.Time
}

// DeletionChallenge belongs to one connection. Copies share consumption state.
type DeletionChallenge struct {
	Challenge
	state *deletionState
}

type deletionState struct {
	mu      sync.Mutex
	used    bool
	voidID  string
	key     ed25519.PublicKey
	message []byte
	id      string
	expires time.Time
}

func NewDeletionService(store LifecycleStore, audience string) (*DeletionService, error) {
	u, err := url.Parse(audience)
	if store == nil || err != nil || len(audience) > 2048 || u.Scheme != "https" || u.Hostname() == "" ||
		u.User != nil || u.ForceQuery || u.RawQuery != "" || u.Fragment != "" ||
		u.String() != audience || strings.HasSuffix(audience, "/") || strings.ContainsAny(audience, "\r\n\t ") {
		return nil, ErrInvalidInput
	}
	return &DeletionService{store: store, audience: audience, now: time.Now}, nil
}

func validID(id string) bool {
	n, err := strconv.ParseInt(id, 10, 64)
	return err == nil && n > 0 && strconv.FormatInt(n, 10) == id
}

func (s *DeletionService) Begin(ctx context.Context, id string) (*DeletionChallenge, error) {
	if !validID(id) {
		return nil, ErrInvalidInput
	}
	key, err := s.store.DeletionKey(ctx, id)
	if err != nil {
		return nil, err
	}
	if !ValidPublicKey(key) {
		return nil, ErrUnavailable
	}
	var entropy [64]byte
	if _, err := rand.Read(entropy[:]); err != nil {
		return nil, ErrUnavailable
	}
	token := base64.RawURLEncoding.EncodeToString(entropy[:32])
	expires := s.now().Add(30 * time.Second).UTC()
	message := []byte("VOID:delete-account:v1\n" + token + "\n" + id + "\n" +
		base64.StdEncoding.EncodeToString(key) + "\n" + expires.Format(time.RFC3339Nano) + "\n" +
		base64.StdEncoding.EncodeToString([]byte(s.audience)) + "\nDELETE\n" +
		base64.RawURLEncoding.EncodeToString(entropy[32:]) + "\n")
	return &DeletionChallenge{Challenge: Challenge{ChallengeID: token, Message: base64.StdEncoding.EncodeToString(message), ExpiresAt: expires},
		state: &deletionState{voidID: id, key: append(ed25519.PublicKey(nil), key...), message: message, id: token, expires: expires}}, nil
}

// Confirm consumes every attempt before verification. A fresh signed challenge
// can recover the same durable result after a lost acknowledgment or restart.
func (s *DeletionService) Confirm(ctx context.Context, c *DeletionChallenge, id, confirmation string, signature []byte) (Status, error) {
	if c == nil || c.state == nil {
		return Status{}, ErrInvalidChallenge
	}
	state := c.state
	state.mu.Lock()
	used := state.used
	state.used = true
	state.mu.Unlock()
	if used || id != state.id || !s.now().Before(state.expires) {
		return Status{}, ErrInvalidChallenge
	}
	if confirmation != "DELETE" || len(signature) != ed25519.SignatureSize || !ed25519.Verify(state.key, state.message, signature) {
		return Status{}, ErrInvalidSignature
	}
	return s.store.Delete(ctx, state.voidID)
}

func (s *DeletionService) Status(ctx context.Context, actor, target string) (Status, error) {
	if !validID(actor) || !validID(target) {
		return Status{}, ErrInvalidInput
	}
	return s.store.Status(ctx, actor, target)
}

// WithActive serializes bounded online operations with durable deletion.
func (s *DeletionService) WithActive(ctx context.Context, ids []string, fn func() error) error {
	return s.store.WithActive(ctx, ids, fn)
}

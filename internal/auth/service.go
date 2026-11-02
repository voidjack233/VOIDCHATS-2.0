// Package auth authenticates existing accounts with connection-bound Ed25519
// challenges. Each connection owns its challenge; there is no global token map.
package auth

import (
	"context"
	"crypto/ed25519"
	"crypto/rand"
	"encoding/base64"
	"errors"
	"io"
	"net/url"
	"strconv"
	"strings"
	"sync"
	"time"

	"void-nothingness/internal/account"
)

var (
	ErrNotFound       = errors.New("authentication account not found")
	ErrInvalidInput   = errors.New("invalid authentication input")
	ErrAuthentication = errors.New("authentication failed")
	ErrUnavailable    = errors.New("authentication temporarily unavailable")
)

// KeyLookup fetches an existing account's public authentication key. It returns
// ErrNotFound for unknown accounts. No operation accepts private key material.
type KeyLookup interface {
	AuthenticationKey(context.Context, int64) ([]byte, error)
}

type Service struct {
	lookup   KeyLookup
	ttl      time.Duration
	audience string
	now      func() time.Time
	rand     io.Reader
}

// Challenge is the public challenge envelope plus its private, single-use state.
// Copies share consumption state. Only its owning connection may verify it.
// Message uses standard padded base64; clients sign the decoded bytes verbatim.
type Challenge struct {
	ChallengeID string    `json:"challenge_id"`
	Message     string    `json:"message"`
	ExpiresAt   time.Time `json:"expires_at"`

	state *challengeState
}

type challengeState struct {
	mu        sync.Mutex
	used      bool
	id        string
	voidID    string
	publicKey ed25519.PublicKey
	message   []byte
	expiresAt time.Time
	now       func() time.Time
}

// NewService requires a positive challenge lifetime of at most 30 seconds.
func NewService(lookup KeyLookup, ttl time.Duration) (*Service, error) {
	if lookup == nil || ttl <= 0 || ttl > 30*time.Second {
		return nil, errors.New("invalid authentication service configuration")
	}
	return &Service{lookup: lookup, ttl: ttl, now: time.Now, rand: rand.Reader}, nil
}

// NewServiceForAudience binds every connection proof to this deployment's
// canonical HTTPS API audience, existing identity key, and a fresh nonce. The
// legacy constructor retains v1 framing for compatibility; production uses v2.
func NewServiceForAudience(lookup KeyLookup, ttl time.Duration, audience string) (*Service, error) {
	u, err := url.Parse(audience)
	if err != nil || len(audience) > 2048 || u.Scheme != "https" || u.Hostname() == "" ||
		u.User != nil || u.ForceQuery || u.RawQuery != "" || u.Fragment != "" || strings.HasSuffix(audience, "/") ||
		u.String() != audience || strings.ContainsAny(audience, "\r\n\t ") {
		return nil, errors.New("invalid authentication audience")
	}
	s, err := NewService(lookup, ttl)
	if err != nil {
		return nil, err
	}
	s.audience = audience
	return s, nil
}

// ValidVoidID accepts the canonical decimal representation of a positive int64.
func ValidVoidID(voidID string) bool {
	if len(voidID) == 0 || len(voidID) > 19 || voidID[0] < '1' || voidID[0] > '9' {
		return false
	}
	for i := 1; i < len(voidID); i++ {
		if voidID[i] < '0' || voidID[i] > '9' {
			return false
		}
	}
	id, err := strconv.ParseInt(voidID, 10, 64)
	return err == nil && id > 0
}

// Begin loads the stored public key and issues a challenge bound to that account.
// Storage errors and cancellation never expose repository or connection details.
func (s *Service) Begin(ctx context.Context, voidID string) (*Challenge, error) {
	if !ValidVoidID(voidID) {
		return nil, ErrInvalidInput
	}
	if ctx.Err() != nil {
		return nil, ErrUnavailable
	}
	id, _ := strconv.ParseInt(voidID, 10, 64)
	key, err := s.lookup.AuthenticationKey(ctx, id)
	if ctx.Err() != nil {
		return nil, ErrUnavailable
	}
	if errors.Is(err, ErrNotFound) {
		return nil, ErrAuthentication
	}
	if err != nil || !account.ValidPublicKey(key) {
		return nil, ErrUnavailable
	}
	// Independent 256-bit values prevent predictable challenge IDs and nonces.
	var entropy [64]byte
	if _, err := io.ReadFull(s.rand, entropy[:]); err != nil {
		return nil, ErrUnavailable
	}
	if ctx.Err() != nil {
		return nil, ErrUnavailable
	}
	challengeID := base64.RawURLEncoding.EncodeToString(entropy[:32])
	nonce := base64.RawURLEncoding.EncodeToString(entropy[32:])
	expiresAt := s.now().Add(s.ttl).UTC()
	message := []byte("VOID-NOTHINGNESS:authenticate:v1\n" + challengeID + "\n" + voidID + "\n" +
		expiresAt.Format(time.RFC3339Nano) + "\n" + nonce + "\n")
	if s.audience != "" {
		message = []byte("VOID-NOTHINGNESS:authenticate:v2\n" + challengeID + "\n" + voidID + "\n" +
			base64.StdEncoding.EncodeToString(key) + "\n" + expiresAt.Format(time.RFC3339Nano) + "\n" +
			base64.StdEncoding.EncodeToString([]byte(s.audience)) + "\n" + nonce + "\n")
	}
	return &Challenge{
		ChallengeID: challengeID,
		Message:     base64.StdEncoding.EncodeToString(message),
		ExpiresAt:   expiresAt,
		state: &challengeState{
			id:        challengeID,
			voidID:    voidID,
			publicKey: append(ed25519.PublicKey(nil), key...),
			message:   message,
			expiresAt: expiresAt,
			now:       s.now,
		},
	}, nil
}

// Verify consumes the challenge before checking any proof, including a malformed
// ID or signature. Success returns only the identity bound during Begin; callers
// must never obtain an authenticated identity from a client-supplied sender field.
func (c *Challenge) Verify(challengeID string, signature []byte) (string, error) {
	if c == nil || c.state == nil {
		return "", ErrAuthentication
	}
	state := c.state
	state.mu.Lock()
	if state.used {
		state.mu.Unlock()
		return "", ErrAuthentication
	}
	state.used = true
	state.mu.Unlock()
	if challengeID != state.id || len(signature) != ed25519.SignatureSize || !state.now().Before(state.expiresAt) {
		return "", ErrAuthentication
	}
	if !ed25519.Verify(state.publicKey, state.message, signature) {
		return "", ErrAuthentication
	}
	return state.voidID, nil
}

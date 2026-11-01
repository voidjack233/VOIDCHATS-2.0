// Package session authenticates existing Ed25519 identities for the HTTP API.
// The server accepts signatures and public keys only, never private keys.
package session

import (
	"context"
	"crypto/ed25519"
	"crypto/rand"
	"crypto/sha256"
	"encoding/base64"
	"errors"
	"io"
	"net/url"
	"strconv"
	"strings"
	"sync"
	"time"

	"void-nothingness/internal/account"
	"void-nothingness/internal/auth"
)

const (
	ChallengeTTL          = 2 * time.Minute
	TokenTTL              = 30 * time.Minute
	MaxPendingChallenges  = 4096
	MaxSessionsPerAccount = 8
)

var (
	ErrInvalidInput   = errors.New("invalid session input")
	ErrAuthentication = errors.New("authentication failed")
	ErrCapacity       = errors.New("session capacity reached")
	ErrUnavailable    = errors.New("session temporarily unavailable")
)

// Record contains only a SHA-256 token digest, never a reusable bearer token.
type Record struct {
	TokenHash [sha256.Size]byte
	VoidID    string
	CreatedAt time.Time
	ExpiresAt time.Time
}

// Store implementations persist sessions across server restarts. Persist must
// atomically enforce maxPerAccount active sessions, reject duplicate hashes,
// and remove expired sessions. Find returns ErrAuthentication for absent tokens.
// Revoke is idempotent. Store errors must not expose database diagnostics.
type Store interface {
	Persist(context.Context, Record, int) error
	Find(context.Context, [sha256.Size]byte) (Record, error)
	Revoke(context.Context, [sha256.Size]byte) error
}

type Challenge struct {
	ChallengeID string    `json:"challenge_id"`
	Message     string    `json:"message"`
	ExpiresAt   time.Time `json:"expires_at"`
}

type Token struct {
	AccessToken string    `json:"access_token"`
	TokenType   string    `json:"token_type"`
	ExpiresAt   time.Time `json:"expires_at"`
}

type pendingChallenge struct {
	voidID    string
	key       ed25519.PublicKey
	message   []byte
	expiresAt time.Time
}

type Service struct {
	lookup        auth.KeyLookup
	store         Store
	audience      string
	now           func() time.Time
	rand          io.Reader
	mu            sync.Mutex
	challenges    map[string]pendingChallenge
	maxChallenges int
	maxSessions   int
}

// New requires a canonical HTTPS audience without a trailing slash, query or
// fragment. It binds proofs to one API deployment. Challenges are intentionally
// short-lived process memory; durable bearer sessions survive process restarts.
func New(lookup auth.KeyLookup, store Store, audience string) (*Service, error) {
	u, err := url.Parse(audience)
	if lookup == nil || store == nil || err != nil || u.Scheme != "https" || u.Hostname() == "" ||
		u.User != nil || u.ForceQuery || u.RawQuery != "" || u.Fragment != "" || strings.HasSuffix(audience, "/") ||
		u.String() != audience || strings.ContainsAny(audience, "\r\n\t ") {
		return nil, ErrInvalidInput
	}
	return &Service{lookup: lookup, store: store, audience: audience, now: time.Now,
		rand: rand.Reader, challenges: make(map[string]pendingChallenge),
		maxChallenges: MaxPendingChallenges, maxSessions: MaxSessionsPerAccount}, nil
}

func (s *Service) pruneLocked(now time.Time) {
	for id, c := range s.challenges {
		if !now.Before(c.expiresAt) {
			delete(s.challenges, id)
		}
	}
}

// Challenge binds the exact registered identity key, API audience and expiry.
// Message is padded standard base64; clients sign its decoded bytes verbatim.
func (s *Service) Challenge(ctx context.Context, voidID string) (Challenge, error) {
	if !auth.ValidVoidID(voidID) {
		return Challenge{}, ErrInvalidInput
	}
	if ctx.Err() != nil {
		return Challenge{}, ErrUnavailable
	}
	s.mu.Lock()
	s.pruneLocked(s.now())
	full := len(s.challenges) >= s.maxChallenges
	s.mu.Unlock()
	if full {
		return Challenge{}, ErrCapacity
	}
	id, _ := strconv.ParseInt(voidID, 10, 64)
	key, err := s.lookup.AuthenticationKey(ctx, id)
	if ctx.Err() != nil {
		return Challenge{}, ErrUnavailable
	}
	if errors.Is(err, auth.ErrNotFound) {
		return Challenge{}, ErrAuthentication
	}
	if err != nil || !account.ValidPublicKey(key) {
		return Challenge{}, ErrUnavailable
	}
	var entropy [64]byte
	if _, err := io.ReadFull(s.rand, entropy[:]); err != nil {
		return Challenge{}, ErrUnavailable
	}
	challengeID := base64.RawURLEncoding.EncodeToString(entropy[:32])
	nonce := base64.RawURLEncoding.EncodeToString(entropy[32:])
	expiresAt := s.now().Add(ChallengeTTL).UTC()
	message := []byte("VOID-NOTHINGNESS:auth:v1\n" + challengeID + "\n" +
		expiresAt.Format(time.RFC3339Nano) + "\n" + voidID + "\n" +
		base64.StdEncoding.EncodeToString(key) + "\n" +
		base64.StdEncoding.EncodeToString([]byte(s.audience)) + "\n" + nonce + "\n")
	s.mu.Lock()
	defer s.mu.Unlock()
	s.pruneLocked(s.now())
	if ctx.Err() != nil {
		return Challenge{}, ErrUnavailable
	}
	if len(s.challenges) >= s.maxChallenges {
		return Challenge{}, ErrCapacity
	}
	if _, exists := s.challenges[challengeID]; exists {
		return Challenge{}, ErrUnavailable
	}
	s.challenges[challengeID] = pendingChallenge{voidID: voidID, key: append(ed25519.PublicKey(nil), key...), message: message, expiresAt: expiresAt}
	return Challenge{ChallengeID: challengeID, Message: base64.StdEncoding.EncodeToString(message), ExpiresAt: expiresAt}, nil
}

// Create atomically consumes a challenge before checking expiry or any proof.
// Failed proofs cannot be retried; clients must obtain a fresh challenge.
func (s *Service) Create(ctx context.Context, challengeID string, signature []byte) (Token, error) {
	s.mu.Lock()
	c, found := s.challenges[challengeID]
	delete(s.challenges, challengeID)
	s.pruneLocked(s.now())
	s.mu.Unlock()
	if !found || !s.now().Before(c.expiresAt) || len(signature) != ed25519.SignatureSize || !ed25519.Verify(c.key, c.message, signature) {
		return Token{}, ErrAuthentication
	}
	if ctx.Err() != nil {
		return Token{}, ErrUnavailable
	}
	var entropy [32]byte
	if _, err := io.ReadFull(s.rand, entropy[:]); err != nil {
		return Token{}, ErrUnavailable
	}
	raw := base64.RawURLEncoding.EncodeToString(entropy[:])
	now := s.now().UTC()
	record := Record{TokenHash: sha256.Sum256([]byte(raw)), VoidID: c.voidID, CreatedAt: now, ExpiresAt: now.Add(TokenTTL)}
	if err := s.store.Persist(ctx, record, s.maxSessions); err != nil {
		if errors.Is(err, ErrCapacity) {
			return Token{}, ErrCapacity
		}
		return Token{}, ErrUnavailable
	}
	if ctx.Err() != nil {
		return Token{}, ErrUnavailable
	}
	return Token{AccessToken: raw, TokenType: "Bearer", ExpiresAt: record.ExpiresAt}, nil
}

func tokenHash(raw string) ([sha256.Size]byte, error) {
	if len(raw) != 43 {
		return [sha256.Size]byte{}, ErrAuthentication
	}
	b, err := base64.RawURLEncoding.Strict().DecodeString(raw)
	if err != nil || len(b) != 32 || base64.RawURLEncoding.EncodeToString(b) != raw {
		return [sha256.Size]byte{}, ErrAuthentication
	}
	return sha256.Sum256([]byte(raw)), nil
}

// Authenticate returns only the account bound to a current persisted session.
func (s *Service) Authenticate(ctx context.Context, rawToken string) (string, error) {
	hash, err := tokenHash(rawToken)
	if err != nil {
		return "", err
	}
	if ctx.Err() != nil {
		return "", ErrUnavailable
	}
	r, err := s.store.Find(ctx, hash)
	if errors.Is(err, ErrAuthentication) {
		return "", ErrAuthentication
	}
	if err != nil || ctx.Err() != nil {
		return "", ErrUnavailable
	}
	if r.TokenHash != hash || !auth.ValidVoidID(r.VoidID) || !s.now().Before(r.ExpiresAt) {
		return "", ErrAuthentication
	}
	return r.VoidID, nil
}

// Revoke deletes the caller's hashed token; repeating revocation succeeds.
func (s *Service) Revoke(ctx context.Context, rawToken string) error {
	hash, err := tokenHash(rawToken)
	if err != nil {
		return err
	}
	if ctx.Err() != nil {
		return ErrUnavailable
	}
	if err := s.store.Revoke(ctx, hash); err != nil || ctx.Err() != nil {
		return ErrUnavailable
	}
	return nil
}

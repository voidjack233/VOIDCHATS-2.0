package session

import (
	"bytes"
	"context"
	"crypto/ed25519"
	"crypto/rand"
	"crypto/sha256"
	"encoding/base64"
	"encoding/hex"
	"encoding/json"
	"errors"
	"os"
	"strings"
	"sync"
	"sync/atomic"
	"testing"
	"time"

	"void-nothingness/internal/auth"
)

const testAudience = "https://api.voidchats.xyz/dm9pZG5vdGhpbmduZXNz"

type testLookup struct {
	key []byte
	err error
}

func (l testLookup) AuthenticationKey(context.Context, int64) ([]byte, error) { return l.key, l.err }

type memoryStore struct {
	mu   sync.Mutex
	rows map[[32]byte]Record
	err  error
}

func newMemoryStore() *memoryStore { return &memoryStore{rows: make(map[[32]byte]Record)} }
func (m *memoryStore) Persist(_ context.Context, r Record, max int) error {
	m.mu.Lock()
	defer m.mu.Unlock()
	if m.err != nil {
		return m.err
	}
	count := 0
	for h, row := range m.rows {
		if row.VoidID != r.VoidID {
			continue
		}
		if !r.CreatedAt.Before(row.ExpiresAt) {
			delete(m.rows, h)
		} else {
			count++
		}
	}
	if count >= max {
		return ErrCapacity
	}
	if _, found := m.rows[r.TokenHash]; found {
		return ErrUnavailable
	}
	m.rows[r.TokenHash] = r
	return nil
}
func (m *memoryStore) Find(_ context.Context, h [32]byte) (Record, error) {
	m.mu.Lock()
	defer m.mu.Unlock()
	if m.err != nil {
		return Record{}, m.err
	}
	r, ok := m.rows[h]
	if !ok {
		return Record{}, ErrAuthentication
	}
	return r, nil
}
func (m *memoryStore) Revoke(_ context.Context, h [32]byte) error {
	m.mu.Lock()
	defer m.mu.Unlock()
	if m.err != nil {
		return m.err
	}
	delete(m.rows, h)
	return nil
}

func testService(t *testing.T) (*Service, ed25519.PrivateKey, *memoryStore) {
	t.Helper()
	pub, priv, err := ed25519.GenerateKey(rand.Reader)
	if err != nil {
		t.Fatal(err)
	}
	store := newMemoryStore()
	s, err := New(testLookup{key: pub}, store, testAudience)
	if err != nil {
		t.Fatal(err)
	}
	return s, priv, store
}
func proof(t *testing.T, s *Service, key ed25519.PrivateKey) (Challenge, []byte) {
	t.Helper()
	c, err := s.Challenge(context.Background(), "110559210000000000")
	if err != nil {
		t.Fatal(err)
	}
	msg, err := base64.StdEncoding.DecodeString(c.Message)
	if err != nil {
		t.Fatal(err)
	}
	return c, ed25519.Sign(key, msg)
}
func issue(t *testing.T, s *Service, key ed25519.PrivateKey) Token {
	t.Helper()
	c, sig := proof(t, s, key)
	tok, err := s.Create(context.Background(), c.ChallengeID, sig)
	if err != nil {
		t.Fatal(err)
	}
	return tok
}

func TestCanonicalChallengeBindingsAndTokenPersistence(t *testing.T) {
	s, key, store := testService(t)
	now := time.Date(2026, 11, 2, 0, 0, 0, 123456789, time.UTC)
	s.now = func() time.Time { return now }
	c, sig := proof(t, s, key)
	msg, _ := base64.StdEncoding.DecodeString(c.Message)
	fields := strings.Split(string(msg), "\n")
	if len(fields) != 8 || fields[0] != "VOID-NOTHINGNESS:auth:v1" || fields[1] != c.ChallengeID || fields[2] != c.ExpiresAt.Format(time.RFC3339Nano) || fields[3] != "110559210000000000" || fields[4] != base64.StdEncoding.EncodeToString(key.Public().(ed25519.PublicKey)) || fields[5] != base64.StdEncoding.EncodeToString([]byte(testAudience)) || fields[7] != "" {
		t.Fatalf("invalid challenge bindings: %q", msg)
	}
	nonce, err := base64.RawURLEncoding.DecodeString(fields[6])
	if err != nil || len(nonce) != 32 || fields[6] == c.ChallengeID {
		t.Fatal("nonce must be independently random32bytes")
	}
	if !c.ExpiresAt.Equal(now.Add(ChallengeTTL)) {
		t.Fatal("challenge lifetime mismatch")
	}
	tok, err := s.Create(context.Background(), c.ChallengeID, sig)
	if err != nil {
		t.Fatal(err)
	}
	if tok.TokenType != "Bearer" || !tok.ExpiresAt.Equal(now.Add(TokenTTL)) {
		t.Fatal("token envelope mismatch")
	}
	raw, err := base64.RawURLEncoding.Strict().DecodeString(tok.AccessToken)
	if err != nil || len(raw) != 32 {
		t.Fatal("token entropy encoding invalid")
	}
	h := sha256.Sum256([]byte(tok.AccessToken))
	r, ok := store.rows[h]
	if !ok || r.TokenHash != h || len(store.rows) != 1 {
		t.Fatal("only hashed token was not persisted")
	}
	id, err := s.Authenticate(context.Background(), tok.AccessToken)
	if err != nil || id != "110559210000000000" {
		t.Fatal("issued token did not authenticate")
	}
	// A restarted service retains durable sessions without retaining challenges.
	restarted, err := New(s.lookup, store, testAudience)
	if err != nil {
		t.Fatal(err)
	}
	restarted.now = s.now
	if _, err := restarted.Authenticate(context.Background(), tok.AccessToken); err != nil {
		t.Fatal("restart lost persisted session")
	}
	if _, err := s.Create(context.Background(), c.ChallengeID, sig); err != ErrAuthentication {
		t.Fatal("proof replay succeeded")
	}
	if err := s.Revoke(context.Background(), tok.AccessToken); err != nil {
		t.Fatal(err)
	}
	if err := s.Revoke(context.Background(), tok.AccessToken); err != nil {
		t.Fatal("revocation was not idempotent")
	}
	if _, err := restarted.Authenticate(context.Background(), tok.AccessToken); err != ErrAuthentication {
		t.Fatal("revoked token survived in another service")
	}
}

func TestFailedAndExpiredProofsAreConsumed(t *testing.T) {
	for _, kind := range []string{"malformed", "wrong key", "registration domain", "wrong audience", "expired"} {
		t.Run(kind, func(t *testing.T) {
			s, key, _ := testService(t)
			now := time.Now().UTC()
			s.now = func() time.Time { return now }
			c, sig := proof(t, s, key)
			switch kind {
			case "malformed":
				sig = []byte{1}
			case "wrong key":
				_, other, _ := ed25519.GenerateKey(rand.Reader)
				msg, _ := base64.StdEncoding.DecodeString(c.Message)
				sig = ed25519.Sign(other, msg)
			case "registration domain":
				msg, _ := base64.StdEncoding.DecodeString(c.Message)
				sig = ed25519.Sign(key, bytes.Replace(msg, []byte("VOID-NOTHINGNESS:auth:v1"), []byte("VOID-NOTHINGNESS:register:v1"), 1))
			case "wrong audience":
				msg, _ := base64.StdEncoding.DecodeString(c.Message)
				sig = ed25519.Sign(key, bytes.Replace(msg, []byte(base64.StdEncoding.EncodeToString([]byte(testAudience))), []byte(base64.StdEncoding.EncodeToString([]byte("https://other.example/api"))), 1))
			case "expired":
				now = c.ExpiresAt
			}
			if _, err := s.Create(context.Background(), c.ChallengeID, sig); err != ErrAuthentication {
				t.Fatal("bad proof accepted")
			}
			msg, _ := base64.StdEncoding.DecodeString(c.Message)
			if _, err := s.Create(context.Background(), c.ChallengeID, ed25519.Sign(key, msg)); err != ErrAuthentication {
				t.Fatal("consumed challenge reused")
			}
		})
	}
}

func TestConcurrentProofOnlyCreatesOneSession(t *testing.T) {
	s, key, store := testService(t)
	c, sig := proof(t, s, key)
	var successes atomic.Int32
	var wg sync.WaitGroup
	for i := 0; i < 64; i++ {
		wg.Add(1)
		go func() {
			defer wg.Done()
			_, err := s.Create(context.Background(), c.ChallengeID, sig)
			if err == nil {
				successes.Add(1)
			} else if err != ErrAuthentication {
				t.Errorf("unexpected error: %v", err)
			}
		}()
	}
	wg.Wait()
	if successes.Load() != 1 || len(store.rows) != 1 {
		t.Fatal("concurrent challenge consumption was not atomic")
	}
}

func TestChallengeCapacityIsBoundedAndExpires(t *testing.T) {
	s, _, _ := testService(t)
	s.maxChallenges = 5
	now := time.Now()
	s.now = func() time.Time { return now }
	var successes atomic.Int32
	var wg sync.WaitGroup
	for i := 0; i < 64; i++ {
		wg.Add(1)
		go func() {
			defer wg.Done()
			_, err := s.Challenge(context.Background(), "123")
			if err == nil {
				successes.Add(1)
			} else if err != ErrCapacity {
				t.Errorf("unexpected error: %v", err)
			}
		}()
	}
	wg.Wait()
	if successes.Load() != 5 || len(s.challenges) != 5 {
		t.Fatal("challenge capacity exceeded")
	}
	now = now.Add(ChallengeTTL)
	if _, err := s.Challenge(context.Background(), "123"); err != nil {
		t.Fatal("expired challenges did not free capacity")
	}
	if len(s.challenges) != 1 {
		t.Fatal("expired challenge state retained")
	}
}

func TestSessionCapacityRevocationAndExpiry(t *testing.T) {
	s, key, store := testService(t)
	s.maxSessions = 2
	now := time.Now()
	s.now = func() time.Time { return now }
	a := issue(t, s, key)
	b := issue(t, s, key)
	c, sig := proof(t, s, key)
	if _, err := s.Create(context.Background(), c.ChallengeID, sig); err != ErrCapacity {
		t.Fatal("session cap not enforced")
	}
	if err := s.Revoke(context.Background(), a.AccessToken); err != nil {
		t.Fatal(err)
	}
	issue(t, s, key)
	now = now.Add(TokenTTL)
	for _, r := range store.rows {
		if now.Before(r.ExpiresAt) {
			t.Fatal("test sessions should be expired")
		}
	}
	if _, err := s.Authenticate(context.Background(), a.AccessToken); err != ErrAuthentication {
		t.Fatal("expired/revoked token accepted")
	}
	if _, err := s.Authenticate(context.Background(), b.AccessToken); err != ErrAuthentication {
		t.Fatal("expired persisted token accepted")
	}
	issue(t, s, key)
	if len(store.rows) != 1 {
		t.Fatal("expired sessions were not pruned on issuance")
	}
}

func TestMalformedTokensAndFailuresAreSanitized(t *testing.T) {
	s, key, store := testService(t)
	token := issue(t, s, key)
	for _, bad := range []string{"", token.AccessToken + "=", "Bearer " + token.AccessToken, strings.Repeat("!", 43), token.AccessToken + "\n"} {
		if _, err := s.Authenticate(context.Background(), bad); err != ErrAuthentication {
			t.Fatalf("malformed token accepted: %q", bad)
		}
		if err := s.Revoke(context.Background(), bad); err != ErrAuthentication {
			t.Fatal("malformed revocation token accepted")
		}
	}
	store.err = errors.New("secret connection SQL password")
	if _, err := s.Authenticate(context.Background(), token.AccessToken); err != ErrUnavailable || errors.Unwrap(err) != nil {
		t.Fatal("storage error leaked")
	}
	if err := s.Revoke(context.Background(), token.AccessToken); err != ErrUnavailable {
		t.Fatal("revocation error leaked")
	}
	c, sig := proof(t, s, key)
	if _, err := s.Create(context.Background(), c.ChallengeID, sig); err != ErrUnavailable {
		t.Fatal("issuance error leaked")
	}
	ctx, cancel := context.WithCancel(context.Background())
	cancel()
	if _, err := s.Challenge(ctx, "123"); err != ErrUnavailable {
		t.Fatal("challenge ignored cancellation")
	}
}

func TestInvalidAccountsAudienceAndEntropy(t *testing.T) {
	s, _, store := testService(t)
	for _, aud := range []string{"", "http://api.example/api", "https://api.example/api/", "https://api.example/api?x=1", "https://api.example/api?", "https://api.example/api#x", "https://user@api.example/api", "https://:8080/api"} {
		if _, err := New(s.lookup, store, aud); err != ErrInvalidInput {
			t.Fatalf("invalid audience accepted: %s", aud)
		}
	}
	for _, id := range []string{"", "0", "-1", "01", "9223372036854775808", "1\n"} {
		if _, err := s.Challenge(context.Background(), id); err != ErrInvalidInput {
			t.Fatal("invalid VOID-ID accepted")
		}
	}
	s.lookup = testLookup{err: auth.ErrNotFound}
	if _, err := s.Challenge(context.Background(), "123"); err != ErrAuthentication {
		t.Fatal("unknown account accepted")
	}
	s.lookup = testLookup{key: make([]byte, 32)}
	if _, err := s.Challenge(context.Background(), "123"); err != ErrUnavailable {
		t.Fatal("invalid account key accepted")
	}
	pub, _, _ := ed25519.GenerateKey(rand.Reader)
	s.lookup = testLookup{key: pub}
	s.rand = bytes.NewReader(nil)
	if _, err := s.Challenge(context.Background(), "123"); err != ErrUnavailable {
		t.Fatal("entropy failure accepted")
	}
}

func TestPublishedAuthenticationVector(t *testing.T) {
	data, err := os.ReadFile("testdata/auth-vector.json")
	if err != nil {
		t.Fatal(err)
	}
	var v struct {
		SeedHex     string    `json:"test_seed_hex"`
		PublicKey   string    `json:"public_key_base64"`
		VoidID      string    `json:"void_id"`
		Audience    string    `json:"audience"`
		ChallengeID string    `json:"challenge_id"`
		ExpiresAt   time.Time `json:"expires_at"`
		Message     string    `json:"message_base64"`
		Signature   string    `json:"signature_base64"`
		MessageUTF8 string    `json:"message_utf8"`
	}
	if err := json.Unmarshal(data, &v); err != nil {
		t.Fatal(err)
	}
	seed, err := hex.DecodeString(v.SeedHex)
	if err != nil {
		t.Fatal(err)
	}
	key := ed25519.NewKeyFromSeed(seed)
	if base64.StdEncoding.EncodeToString(key.Public().(ed25519.PublicKey)) != v.PublicKey {
		t.Fatal("vector key mismatch")
	}
	s, err := New(testLookup{key: key.Public().(ed25519.PublicKey)}, newMemoryStore(), v.Audience)
	if err != nil {
		t.Fatal(err)
	}
	now := v.ExpiresAt.Add(-ChallengeTTL)
	s.now = func() time.Time { return now }
	entropy := make([]byte, 96)
	for i := range entropy {
		entropy[i] = byte(i)
	}
	s.rand = bytes.NewReader(entropy)
	c, err := s.Challenge(context.Background(), v.VoidID)
	if err != nil {
		t.Fatal(err)
	}
	msg, _ := base64.StdEncoding.DecodeString(c.Message)
	sig, _ := base64.StdEncoding.DecodeString(v.Signature)
	if c.ChallengeID != v.ChallengeID || c.Message != v.Message || string(msg) != v.MessageUTF8 || !bytes.Equal(ed25519.Sign(key, msg), sig) {
		t.Fatal("published signing vector differs from service")
	}
	if _, err := s.Create(context.Background(), c.ChallengeID, sig); err != nil {
		t.Fatal(err)
	}
}

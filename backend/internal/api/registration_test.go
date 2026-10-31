package api

import (
	"bytes"
	"context"
	"crypto/ed25519"
	"crypto/rand"
	"encoding/base64"
	"encoding/json"
	"net/http"
	"net/http/httptest"
	"strings"
	"sync"
	"testing"
	"time"
)

type memoryStore struct {
	mu       sync.Mutex
	accounts map[string]Account
}
func (m *memoryStore) Ping(context.Context) error { return nil }
func (m *memoryStore) CreateAccount(_ context.Context, input Account) (time.Time, error) {
	m.mu.Lock()
	defer m.mu.Unlock()
	if m.accounts == nil { m.accounts = make(map[string]Account) }
	if _, exists := m.accounts[input.VoidID]; exists { return time.Time{}, ErrConflict }
	m.accounts[input.VoidID] = input
	return time.Date(2026, 10, 31, 0, 0, 0, 0, time.UTC), nil
}

func post(t *testing.T, h http.Handler, path string, data any) *httptest.ResponseRecorder {
	t.Helper()
	body, err := json.Marshal(data)
	if err != nil { t.Fatal(err) }
	req := httptest.NewRequest(http.MethodPost, path, bytes.NewReader(body))
	rec := httptest.NewRecorder()
	h.ServeHTTP(rec, req)
	return rec
}

type issued struct {
	ID      string `json:"challenge_id"`
	Message string `json:"challenge_base64"`
}

func issue(t *testing.T, h http.Handler, key ed25519.PublicKey, id string) issued {
	t.Helper()
	rec := post(t, h, "/v1/registration/challenges", issueRequest{
		VoidID: id, Username: "Alice", AuthPublicKeyBase64: base64.StdEncoding.EncodeToString(key),
	})
	if rec.Code != http.StatusCreated { t.Fatalf("challenge: %d %s", rec.Code, rec.Body.String()) }
	var result issued
	if err := json.NewDecoder(rec.Body).Decode(&result); err != nil { t.Fatal(err) }
	return result
}

func TestAccountRegistrationAndReplay(t *testing.T) {
	store := &memoryStore{}
	h := New(store).Handler()
	pub, priv, err := ed25519.GenerateKey(rand.Reader)
	if err != nil { t.Fatal(err) }
	ch := issue(t, h, pub, "13134123413412345678")
	data, err := base64.StdEncoding.DecodeString(ch.Message)
	if err != nil { t.Fatal(err) }
	if !bytes.HasPrefix(data, []byte("VOIDCHATS/account-registration/v1\x00")) {
		t.Fatal("challenge lacks protocol separation")
	}
	body := registerRequest{ChallengeID: ch.ID, SignatureBase64: base64.StdEncoding.EncodeToString(ed25519.Sign(priv, data))}
	rec := post(t, h, "/v1/accounts", body)
	if rec.Code != http.StatusCreated { t.Fatalf("register: %d %s", rec.Code, rec.Body.String()) }
	if !strings.Contains(rec.Body.String(), "13134123413412345678") { t.Fatal("missing registered ID") }
	if post(t, h, "/v1/accounts", body).Code != http.StatusBadRequest { t.Fatal("challenge replay accepted") }
	if store.accounts["13134123413412345678"].Username != "Alice" { t.Fatal("stored incorrect account") }
}

func TestImpersonationAndCollision(t *testing.T) {
	store := &memoryStore{}
	h := New(store).Handler()
	pub, priv, _ := ed25519.GenerateKey(rand.Reader)
	_, attacker, _ := ed25519.GenerateKey(rand.Reader)
	ch := issue(t, h, pub, "12345678901234567890")
	data, _ := base64.StdEncoding.DecodeString(ch.Message)
	bad := post(t, h, "/v1/accounts", registerRequest{ChallengeID: ch.ID, SignatureBase64: base64.StdEncoding.EncodeToString(ed25519.Sign(attacker, data))})
	if bad.Code != http.StatusUnauthorized { t.Fatalf("wrong key accepted: %d", bad.Code) }
	if len(store.accounts) != 0 { t.Fatal("invalid owner registered") }
	ch = issue(t, h, pub, "12345678901234567890")
	data, _ = base64.StdEncoding.DecodeString(ch.Message)
	sig := base64.StdEncoding.EncodeToString(ed25519.Sign(priv, data))
	if rec := post(t, h, "/v1/accounts", registerRequest{ChallengeID: ch.ID, SignatureBase64: sig}); rec.Code != http.StatusCreated {
		t.Fatalf("expected success, got %d", rec.Code)
	}
	ch = issue(t, h, pub, "12345678901234567890")
	data, _ = base64.StdEncoding.DecodeString(ch.Message)
	sig = base64.StdEncoding.EncodeToString(ed25519.Sign(priv, data))
	if rec := post(t, h, "/v1/accounts", registerRequest{ChallengeID: ch.ID, SignatureBase64: sig}); rec.Code != http.StatusConflict {
		t.Fatalf("duplicate ID not rejected: %d", rec.Code)
	}
}

func TestBadInputAndExpiry(t *testing.T) {
	h := New(&memoryStore{})
	pub, priv, _ := ed25519.GenerateKey(rand.Reader)
	if rec := post(t, h.Handler(), "/v1/registration/challenges", issueRequest{
		VoidID: "123", Username: "Alice", AuthPublicKeyBase64: base64.StdEncoding.EncodeToString(pub),
	}); rec.Code != http.StatusBadRequest { t.Fatalf("invalid VOID-ID accepted: %d", rec.Code) }
	ch := issue(t, h.Handler(), pub, "77777777777777777777")
	data, _ := base64.StdEncoding.DecodeString(ch.Message)
	h.now = func() time.Time { return time.Now().Add(6 * time.Minute) }
	rec := post(t, h.Handler(), "/v1/accounts", registerRequest{
		ChallengeID: ch.ID, SignatureBase64: base64.StdEncoding.EncodeToString(ed25519.Sign(priv, data)),
	})
	if rec.Code != http.StatusBadRequest { t.Fatalf("expired challenge accepted: %d", rec.Code) }
}

func TestHealthAndReadiness(t *testing.T) {
	h := New(&memoryStore{}).Handler()
	for _, path := range []string{"/healthz", "/readyz"} {
		w := httptest.NewRecorder()
		h.ServeHTTP(w, httptest.NewRequest(http.MethodGet, path, nil))
		if w.Code != http.StatusOK { t.Fatalf("%s: %d", path, w.Code) }
	}
}

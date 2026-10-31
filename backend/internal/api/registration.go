package api

import (
	"context"
	"crypto/ed25519"
	"crypto/rand"
	"encoding/base64"
	"encoding/json"
	"errors"
	"io"
	"net"
	"net/http"
	"strings"
	"sync"
	"time"
	"unicode"
	"unicode/utf8"
)

var ErrConflict = errors.New("account already registered")

// Account is a routing identity, not an MLS credential. Never persist a private key.
type Account struct {
	VoidID        string
	Username      string
	AuthPublicKey []byte
}

type Store interface {
	CreateAccount(context.Context, Account) (time.Time, error)
	Ping(context.Context) error
}

type challenge struct {
	account Account
	message []byte
	expires time.Time
}

type attemptWindow struct {
	count   int
	expires time.Time
}

// Server is deliberately single-instance in phase one: challenges and rate limits
// live in memory. Use a shared store before adding multiple API instances.
type Server struct {
	store      Store
	mu         sync.Mutex
	pending    map[string]challenge
	attempts   map[string]attemptWindow
	now        func() time.Time
	random     io.Reader
}

func New(store Store) *Server {
	return &Server{
		store: store, pending: make(map[string]challenge),
		attempts: make(map[string]attemptWindow), now: time.Now, random: rand.Reader,
	}
}

func (s *Server) Handler() http.Handler {
	mux := http.NewServeMux()
	mux.HandleFunc("GET /healthz", func(w http.ResponseWriter, _ *http.Request) {
		send(w, http.StatusOK, map[string]string{"status": "ok"})
	})
	mux.HandleFunc("GET /readyz", s.ready)
	mux.HandleFunc("POST /v1/registration/challenges", s.issueChallenge)
	mux.HandleFunc("POST /v1/accounts", s.register)
	return http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		w.Header().Set("Cache-Control", "no-store")
		mux.ServeHTTP(w, r)
	})
}

func (s *Server) ready(w http.ResponseWriter, r *http.Request) {
	ctx, cancel := context.WithTimeout(r.Context(), 2*time.Second)
	defer cancel()
	if err := s.store.Ping(ctx); err != nil {
		fail(w, http.StatusServiceUnavailable, "not_ready")
		return
	}
	send(w, http.StatusOK, map[string]string{"status": "ready"})
}

type issueRequest struct {
	VoidID              string `json:"void_id"`
	Username            string `json:"username"`
	AuthPublicKeyBase64 string `json:"auth_public_key_base64"`
}

func (s *Server) issueChallenge(w http.ResponseWriter, r *http.Request) {
	if !s.allow(r.RemoteAddr) {
		fail(w, http.StatusTooManyRequests, "rate_limited")
		return
	}
	var req issueRequest
	if err := readJSON(w, r, &req); err != nil {
		fail(w, http.StatusBadRequest, "invalid_json")
		return
	}
	name := strings.TrimSpace(req.Username)
	if !validID(req.VoidID) || !validName(name) {
		fail(w, http.StatusBadRequest, "invalid_identity")
		return
	}
	key, err := base64.StdEncoding.Strict().DecodeString(req.AuthPublicKeyBase64)
	if err != nil || len(key) != ed25519.PublicKeySize {
		fail(w, http.StatusBadRequest, "invalid_public_key")
		return
	}
	nonce := make([]byte, 32)
	if _, err := io.ReadFull(s.random, nonce); err != nil {
		fail(w, http.StatusInternalServerError, "internal_error")
		return
	}
	id := base64.RawURLEncoding.EncodeToString(nonce)
	message := append([]byte("VOIDCHATS/account-registration/v1\x00"), nonce...)
	now := s.now()
	expires := now.Add(5 * time.Minute)

	s.mu.Lock()
	for k, entry := range s.pending {
		if !now.Before(entry.expires) { delete(s.pending, k) }
	}
	if len(s.pending) >= 4096 {
		s.mu.Unlock()
		fail(w, http.StatusServiceUnavailable, "temporarily_unavailable")
		return
	}
	s.pending[id] = challenge{account: Account{VoidID: req.VoidID, Username: name, AuthPublicKey: key}, message: message, expires: expires}
	s.mu.Unlock()
	send(w, http.StatusCreated, map[string]any{
		"challenge_id": id,
		"challenge_base64": base64.StdEncoding.EncodeToString(message),
		"expires_at": expires.UTC().Format(time.RFC3339),
	})
}

type registerRequest struct {
	ChallengeID     string `json:"challenge_id"`
	SignatureBase64 string `json:"signature_base64"`
}

func (s *Server) register(w http.ResponseWriter, r *http.Request) {
	var req registerRequest
	if err := readJSON(w, r, &req); err != nil {
		fail(w, http.StatusBadRequest, "invalid_json")
		return
	}
	if len(req.ChallengeID) != 43 {
		fail(w, http.StatusBadRequest, "invalid_challenge")
		return
	}
	signature, err := base64.StdEncoding.Strict().DecodeString(req.SignatureBase64)
	if err != nil || len(signature) != ed25519.SignatureSize {
		fail(w, http.StatusBadRequest, "invalid_signature")
		return
	}

	s.mu.Lock()
	entry, found := s.pending[req.ChallengeID]
	delete(s.pending, req.ChallengeID) // one use even on failed signature or database error
	s.mu.Unlock()
	if !found || !s.now().Before(entry.expires) {
		fail(w, http.StatusBadRequest, "invalid_challenge")
		return
	}
	if !ed25519.Verify(ed25519.PublicKey(entry.account.AuthPublicKey), entry.message, signature) {
		fail(w, http.StatusUnauthorized, "invalid_signature")
		return
	}
	ctx, cancel := context.WithTimeout(r.Context(), 5*time.Second)
	defer cancel()
	created, err := s.store.CreateAccount(ctx, entry.account)
	if errors.Is(err, ErrConflict) {
		fail(w, http.StatusConflict, "identity_unavailable")
		return
	}
	if err != nil {
		fail(w, http.StatusServiceUnavailable, "registration_unavailable")
		return
	}
	send(w, http.StatusCreated, map[string]string{
		"void_id": entry.account.VoidID,
		"username": entry.account.Username,
		"created_at": created.UTC().Format(time.RFC3339),
	})
}

func (s *Server) allow(remote string) bool {
	ip, _, err := net.SplitHostPort(remote)
	if err != nil { ip = remote }
	now := s.now()
	s.mu.Lock()
	defer s.mu.Unlock()
	// Rate limit requests per direct peer address, not untrusted X-Forwarded-For.
	// The reverse proxy must apply its own abuse controls.
	if len(s.attempts) > 4096 {
		for k, a := range s.attempts {
			if !now.Before(a.expires) { delete(s.attempts, k) }
		}
		if len(s.attempts) > 4096 { return false }
	}
	a := s.attempts[ip]
	if !now.Before(a.expires) { a = attemptWindow{expires: now.Add(10*time.Minute)} }
	if a.count >= 20 { return false }
	a.count++
	s.attempts[ip] = a
	return true
}

func validID(id string) bool {
	if len(id) < 16 || len(id) > 20 { return false }
	for i := 0; i < len(id); i++ {
		if id[i] < '0' || id[i] > '9' { return false }
	}
	return true
}

func validName(name string) bool {
	if !utf8.ValidString(name) || len(name) > 128 { return false }
	n := utf8.RuneCountInString(name)
	if n < 1 || n > 32 { return false }
	for _, ch := range name {
		if unicode.IsControl(ch) { return false }
	}
	return true
}

func readJSON(w http.ResponseWriter, r *http.Request, out any) error {
	r.Body = http.MaxBytesReader(w, r.Body, 2048)
	defer r.Body.Close()
	decoder := json.NewDecoder(r.Body)
	decoder.DisallowUnknownFields()
	if err := decoder.Decode(out); err != nil { return err }
	var extra any
	if err := decoder.Decode(&extra); err != io.EOF { return errors.New("trailing JSON") }
	return nil
}

func send(w http.ResponseWriter, code int, response any) {
	w.Header().Set("Content-Type", "application/json")
	w.WriteHeader(code)
	_ = json.NewEncoder(w).Encode(response)
}

func fail(w http.ResponseWriter, code int, label string) {
	send(w, code, map[string]string{"error": label})
}

// These tests compose the real HTTP API, registration service, and Snowflake
// generator. Only persistence is replaced; no PostgreSQL connection is opened.
package registration_test

import (
	"bytes"
	"context"
	"crypto/ed25519"
	"crypto/rand"
	"encoding/base64"
	"encoding/json"
	"errors"
	"io"
	"net/http"
	"net/http/httptest"
	"strconv"
	"strings"
	"sync"
	"sync/atomic"
	"testing"
	"time"

	"void-nothingness/internal/account"
	"void-nothingness/internal/httpapi"
	"void-nothingness/internal/snowflake"
)

type persistedAccount struct {
	account.Account
	id        int64
	publicKey []byte
}

// memoryRepository models atomic key/ID uniqueness and copies values as a
// database would. An optional gate lets tests coordinate simultaneous inserts
// without sleeps or assumptions about HTTP goroutine scheduling.
type memoryRepository struct {
	mu       sync.Mutex
	byKey    map[string]persistedAccount
	ids      map[int64]struct{}
	failNext error
	attempts atomic.Int64
	entered  chan<- struct{}
	release  <-chan struct{}
}

func newRepository() *memoryRepository {
	return &memoryRepository{
		byKey: make(map[string]persistedAccount),
		ids:   make(map[int64]struct{}),
	}
}

func (r *memoryRepository) Insert(ctx context.Context, id int64, username string, key []byte) (account.Account, error) {
	r.attempts.Add(1)
	if r.entered != nil {
		select {
		case r.entered <- struct{}{}:
		case <-ctx.Done():
			return account.Account{}, ctx.Err()
		}
		select {
		case <-r.release:
		case <-ctx.Done():
			return account.Account{}, ctx.Err()
		}
	}
	r.mu.Lock()
	defer r.mu.Unlock()
	if err := ctx.Err(); err != nil {
		return account.Account{}, err
	}
	if r.failNext != nil {
		err := r.failNext
		r.failNext = nil
		return account.Account{}, err
	}
	if _, exists := r.byKey[string(key)]; exists {
		return account.Account{}, account.ErrDuplicateKey
	}
	if _, exists := r.ids[id]; exists {
		return account.Account{}, account.ErrIDCollision
	}
	result := account.Account{
		VoidID:    strconv.FormatInt(id, 10),
		Username:  username,
		CreatedAt: time.Date(2026, time.November, 1, 0, 0, 0, 123456000, time.UTC),
	}
	r.byKey[string(key)] = persistedAccount{Account: result, id: id, publicKey: bytes.Clone(key)}
	r.ids[id] = struct{}{}
	return result, nil
}

func (r *memoryRepository) rows() []persistedAccount {
	r.mu.Lock()
	defer r.mu.Unlock()
	rows := make([]persistedAccount, 0, len(r.byKey))
	for _, row := range r.byKey {
		row.publicKey = bytes.Clone(row.publicKey)
		rows = append(rows, row)
	}
	return rows
}

type registrationClient struct {
	url    string
	client *http.Client
}

func newClient(t *testing.T, repo *memoryRepository) *registrationClient {
	t.Helper()
	// A relative epoch avoids depending on the machine's calendar date while
	// exercising the same generator implementation used by the server.
	ids, err := snowflake.New(17, time.Now().Add(-365*24*time.Hour))
	if err != nil {
		t.Fatal(err)
	}
	service, err := account.NewService(repo, ids, account.Options{
		ChallengeTTL:  2 * time.Minute,
		MaxChallenges: 32,
	})
	if err != nil {
		t.Fatal(err)
	}
	server := httptest.NewServer(httpapi.New(service, nil, httpapi.Options{
		RequestTimeout:        10 * time.Second,
		RatePerMinute:         1000,
		RateBurst:             100,
		MaxConcurrentRequests: 8,
	}))
	t.Cleanup(server.Close)
	client := server.Client()
	client.Timeout = 15 * time.Second
	t.Cleanup(client.CloseIdleConnections)
	return &registrationClient{url: server.URL, client: client}
}

type apiResponse struct {
	status int
	header http.Header
	body   []byte
}

func (c *registrationClient) post(path string, payload any) (apiResponse, error) {
	body, err := json.Marshal(payload)
	if err != nil {
		return apiResponse{}, err
	}
	request, err := http.NewRequest(http.MethodPost, c.url+path, bytes.NewReader(body))
	if err != nil {
		return apiResponse{}, err
	}
	request.Header.Set("Content-Type", "application/json")
	response, err := c.client.Do(request)
	if err != nil {
		return apiResponse{}, err
	}
	defer response.Body.Close()
	data, err := io.ReadAll(io.LimitReader(response.Body, 8192))
	if err != nil {
		return apiResponse{}, err
	}
	return apiResponse{status: response.StatusCode, header: response.Header.Clone(), body: data}, nil
}

func keyPair(t *testing.T) (ed25519.PublicKey, ed25519.PrivateKey) {
	t.Helper()
	publicKey, privateKey, err := ed25519.GenerateKey(rand.Reader)
	if err != nil {
		t.Fatal(err)
	}
	return publicKey, privateKey
}

type registrationProof struct {
	ChallengeID string `json:"challenge_id"`
	Signature   string `json:"signature"`
}

func (c *registrationClient) proof(t *testing.T, username string, publicKey ed25519.PublicKey, privateKey ed25519.PrivateKey) registrationProof {
	t.Helper()
	response, err := c.post("/dm9pZG5vdGhpbmduZXNz/challenges", map[string]string{
		"username":        username,
		"auth_public_key": base64.StdEncoding.EncodeToString(publicKey),
	})
	if err != nil {
		t.Fatal(err)
	}
	if response.status != http.StatusCreated {
		t.Fatalf("challenge status %d, want %d", response.status, http.StatusCreated)
	}
	var challenge account.Challenge
	if err := json.Unmarshal(response.body, &challenge); err != nil {
		t.Fatalf("decode challenge: %v", err)
	}
	message, err := base64.StdEncoding.Strict().DecodeString(challenge.Message)
	if err != nil {
		t.Fatalf("decode signing message: %v", err)
	}
	// A client signs only after checking the domain and the returned binding.
	fields := strings.Split(string(message), "\n")
	if len(fields) != 7 || fields[0] != "VOID-NOTHINGNESS:register:v1" ||
		fields[1] != challenge.ChallengeID || fields[2] != challenge.ExpiresAt.UTC().Format(time.RFC3339Nano) ||
		fields[3] != base64.StdEncoding.EncodeToString(publicKey) ||
		fields[4] != base64.StdEncoding.EncodeToString([]byte(username)) || fields[5] == "" || fields[6] != "" {
		t.Fatal("challenge signing message does not bind the requested identity")
	}
	return registrationProof{
		ChallengeID: challenge.ChallengeID,
		Signature:   base64.StdEncoding.EncodeToString(ed25519.Sign(privateKey, message)),
	}
}

func expectError(t *testing.T, response apiResponse, status int, code string) {
	t.Helper()
	var body struct {
		Error struct {
			Code    string `json:"code"`
			Message string `json:"message"`
		} `json:"error"`
	}
	if err := json.Unmarshal(response.body, &body); err != nil {
		t.Fatalf("decode API error: %v", err)
	}
	if response.status != status || body.Error.Code != code || body.Error.Message == "" {
		t.Fatalf("API error status=%d code=%q, want status=%d code=%q", response.status, body.Error.Code, status, code)
	}
}

func expectPublicAccount(t *testing.T, response apiResponse, persisted persistedAccount) {
	t.Helper()
	if response.status != http.StatusCreated {
		t.Fatalf("registration status %d, want %d", response.status, http.StatusCreated)
	}
	var fields map[string]json.RawMessage
	if err := json.Unmarshal(response.body, &fields); err != nil {
		t.Fatalf("decode registration: %v", err)
	}
	if len(fields) != 3 || fields["void_id"] == nil || fields["username"] == nil || fields["created_at"] == nil {
		t.Fatal("account response must contain only void_id, username, and created_at")
	}
	var result account.Account
	if err := json.Unmarshal(response.body, &result); err != nil {
		t.Fatalf("decode public account fields: %v", err)
	}
	id, err := strconv.ParseInt(result.VoidID, 10, 64)
	if err != nil || id <= 0 || strconv.FormatInt(id, 10) != result.VoidID || id != persisted.id {
		t.Fatal("public VOID-ID is not the persisted positive int64 encoded as a decimal string")
	}
	if result.Username != persisted.Username || !result.CreatedAt.Equal(persisted.CreatedAt) || result.CreatedAt.IsZero() {
		t.Fatal("public account differs from persisted identity or creation timestamp")
	}
	if response.header.Get("Set-Cookie") != "" || response.header.Get("Authorization") != "" {
		t.Fatal("registration must not issue session cookies or authorization credentials")
	}
	if response.header.Get("Cache-Control") != "no-store" {
		t.Fatal("registration response must disable caching")
	}
}

func TestHTTPRegistrationPersistsRequestedIdentityAndRejectsReplay(t *testing.T) {
	repo := newRepository()
	client := newClient(t, repo)
	publicKey, privateKey := keyPair(t)
	expectedKey := bytes.Clone(publicKey)
	const username = "void-界"
	proof := client.proof(t, username, publicKey, privateKey)
	// Discard the caller's mutable input after challenge creation. Persistence
	// must retain the original key to which the proof was bound.
	clear(publicKey)
	response, err := client.post("/dm9pZG5vdGhpbmduZXNz/accounts", proof)
	if err != nil {
		t.Fatal(err)
	}
	rows := repo.rows()
	if len(rows) != 1 || rows[0].Username != username || !bytes.Equal(rows[0].publicKey, expectedKey) {
		t.Fatal("registration did not persist exactly the requested username and key")
	}
	expectPublicAccount(t, response, rows[0])
	response, err = client.post("/dm9pZG5vdGhpbmduZXNz/accounts", proof)
	if err != nil {
		t.Fatal(err)
	}
	expectError(t, response, http.StatusUnauthorized, "invalid_proof")
	if len(repo.rows()) != 1 || repo.attempts.Load() != 1 {
		t.Fatal("replayed proof reached persistence")
	}
}

func TestHTTPConcurrentFreshProofsEnforceAuthenticationKeyUniqueness(t *testing.T) {
	repo := newRepository()
	entered, release := make(chan struct{}, 2), make(chan struct{})
	repo.entered, repo.release = entered, release
	// Always release blocked inserts before server cleanup, including failures.
	var releaseOnce sync.Once
	defer releaseOnce.Do(func() { close(release) })
	client := newClient(t, repo)
	publicKey, privateKey := keyPair(t)
	proofs := []registrationProof{
		client.proof(t, "first-name", publicKey, privateKey),
		client.proof(t, "second-name", publicKey, privateKey),
	}
	if proofs[0].ChallengeID == proofs[1].ChallengeID || proofs[0].Signature == proofs[1].Signature {
		t.Fatal("fresh challenges must create independent proofs")
	}
	type outcome struct {
		response apiResponse
		err      error
	}
	completed := make(chan outcome, len(proofs))
	for _, proof := range proofs {
		go func() {
			response, err := client.post("/dm9pZG5vdGhpbmduZXNz/accounts", proof)
			completed <- outcome{response: response, err: err}
		}()
	}
	ctx, cancel := context.WithTimeout(context.Background(), 5*time.Second)
	defer cancel()
	for range proofs {
		select {
		case <-entered:
		case <-ctx.Done():
			t.Fatal("both valid concurrent proofs did not reach persistence")
		}
	}
	releaseOnce.Do(func() { close(release) })
	var successes, duplicates int
	var success apiResponse
	for range proofs {
		select {
		case result := <-completed:
			if result.err != nil {
				t.Fatal(result.err)
			}
			switch result.response.status {
			case http.StatusCreated:
				successes++
				success = result.response
			case http.StatusConflict:
				duplicates++
				expectError(t, result.response, http.StatusConflict, "key_registered")
			default:
				t.Fatalf("unexpected concurrent registration status %d", result.response.status)
			}
		case <-ctx.Done():
			t.Fatal("concurrent registration did not complete")
		}
	}
	rows := repo.rows()
	if successes != 1 || duplicates != 1 || len(rows) != 1 || repo.attempts.Load() != 2 {
		t.Fatalf("concurrent registration successes=%d duplicates=%d stored=%d attempts=%d", successes, duplicates, len(rows), repo.attempts.Load())
	}
	if !bytes.Equal(rows[0].publicKey, publicKey) || (rows[0].Username != "first-name" && rows[0].Username != "second-name") {
		t.Fatal("concurrent registration persisted an unrelated identity")
	}
	expectPublicAccount(t, success, rows[0])
}

func TestHTTPFailedPersistenceConsumesProofAndFreshProofCanRecover(t *testing.T) {
	repo := newRepository()
	const diagnostic = "in-memory repository rejected first insert"
	repo.failNext = errors.New(diagnostic)
	client := newClient(t, repo)
	publicKey, privateKey := keyPair(t)
	proof := client.proof(t, "retry-user", publicKey, privateKey)
	response, err := client.post("/dm9pZG5vdGhpbmduZXNz/accounts", proof)
	if err != nil {
		t.Fatal(err)
	}
	expectError(t, response, http.StatusServiceUnavailable, "unavailable")
	if strings.Contains(string(response.body), diagnostic) || len(repo.rows()) != 0 || repo.attempts.Load() != 1 {
		t.Fatal("failed insert leaked a repository diagnostic or persisted an account")
	}
	response, err = client.post("/dm9pZG5vdGhpbmduZXNz/accounts", proof)
	if err != nil {
		t.Fatal(err)
	}
	expectError(t, response, http.StatusUnauthorized, "invalid_proof")
	if repo.attempts.Load() != 1 {
		t.Fatal("proof replay after persistence failure reached the repository")
	}
	fresh := client.proof(t, "retry-user", publicKey, privateKey)
	if fresh.ChallengeID == proof.ChallengeID {
		t.Fatal("recovery reused a consumed challenge")
	}
	response, err = client.post("/dm9pZG5vdGhpbmduZXNz/accounts", fresh)
	if err != nil {
		t.Fatal(err)
	}
	rows := repo.rows()
	if len(rows) != 1 || repo.attempts.Load() != 2 || !bytes.Equal(rows[0].publicKey, publicKey) {
		t.Fatal("fresh proof did not recover after persistence failure")
	}
	expectPublicAccount(t, response, rows[0])
}

var _ account.Repository = (*memoryRepository)(nil)

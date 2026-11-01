package relay_test

import (
	"bytes"
	"context"
	"crypto/ed25519"
	"crypto/rand"
	"encoding/base64"
	"encoding/json"
	"io"
	"net/http"
	"net/http/httptest"
	"strconv"
	"strings"
	"testing"
	"time"

	"github.com/coder/websocket"

	"void-nothingness/internal/account"
	"void-nothingness/internal/auth"
	"void-nothingness/internal/httpapi"
	"void-nothingness/internal/relay"
)

type lifetimeKeys map[int64][]byte

func (keys lifetimeKeys) AuthenticationKey(_ context.Context, id int64) ([]byte, error) {
	key, ok := keys[id]
	if !ok {
		return nil, auth.ErrNotFound
	}
	return bytes.Clone(key), nil
}

func (lifetimeKeys) Insert(_ context.Context, id int64, username string, _ []byte) (account.Account, error) {
	return account.Account{VoidID: strconv.FormatInt(id, 10), Username: username, CreatedAt: time.Now().UTC()}, nil
}

type lifetimeIDs struct{}

func (lifetimeIDs) Next(context.Context) (int64, error) { return 3, nil }

// Real net/http deadlines must not become the lifetime of an upgraded socket.
// Registration retains its shorter per-request budget on the same listener.
func TestAuthenticatedWebSocketsOutliveHTTPDeadlines(t *testing.T) {
	const prefix = "/dm9pZG5vdGhpbmduZXNz"
	ctx, cancel := context.WithTimeout(context.Background(), 5*time.Second)
	defer cancel()
	alicePublic, alicePrivate, err := ed25519.GenerateKey(rand.Reader)
	if err != nil {
		t.Fatal(err)
	}
	bobPublic, bobPrivate, err := ed25519.GenerateKey(rand.Reader)
	if err != nil {
		t.Fatal(err)
	}
	keys := lifetimeKeys{1: alicePublic, 2: bobPublic}
	authentication, err := auth.NewService(keys, 3*time.Second)
	if err != nil {
		t.Fatal(err)
	}
	registration, err := account.NewService(keys, lifetimeIDs{}, account.Options{ChallengeTTL: time.Minute, MaxChallenges: 8})
	if err != nil {
		t.Fatal(err)
	}
	connections, err := relay.New(authentication, relay.Options{AuthTimeout: 3 * time.Second})
	if err != nil {
		t.Fatal(err)
	}
	mux := http.NewServeMux()
	mux.Handle(prefix+"/ws", connections)
	mux.Handle("/", httpapi.New(registration, func(context.Context) error { return nil }, httpapi.Options{RequestTimeout: 25 * time.Millisecond}))
	server := httptest.NewUnstartedServer(mux)
	server.Config.ReadHeaderTimeout = time.Second
	server.Config.ReadTimeout = 100 * time.Millisecond
	server.Config.WriteTimeout = 100 * time.Millisecond
	server.Start()
	defer server.Close()
	defer connections.Close()
	client := server.Client()
	client.Timeout = time.Second
	defer client.CloseIdleConnections()
	wsURL := "ws" + strings.TrimPrefix(server.URL, "http") + prefix + "/ws"
	alice := lifetimeAuthenticate(t, ctx, wsURL, "1", alicePrivate)
	defer alice.CloseNow()
	bob := lifetimeAuthenticate(t, ctx, wsURL, "2", bobPrivate)
	defer bob.CloseNow()

	// This is deliberately longer than both inherited net/http deadlines and
	// the registration handler's request timeout, after both handshakes finish.
	timer := time.NewTimer(250 * time.Millisecond)
	defer timer.Stop()
	select {
	case <-timer.C:
	case <-ctx.Done():
		t.Fatal(ctx.Err())
	}
	payload := base64.StdEncoding.EncodeToString([]byte("SIMULATED-CIPHERTEXT:after-http-deadline"))
	lifetimeWrite(t, ctx, alice, map[string]string{"type": "send", "request_id": "after-deadline", "recipient_id": "2", "payload": payload})
	var message struct {
		Type      string `json:"type"`
		RequestID string `json:"request_id"`
		SenderID  string `json:"sender_id"`
		Payload   string `json:"payload"`
	}
	lifetimeRead(t, ctx, bob, &message)
	if message.Type != "message" || message.RequestID != "after-deadline" || message.SenderID != "1" || message.Payload != payload {
		t.Fatalf("unexpected message after HTTP deadlines: %+v", message)
	}
	var result struct {
		Type      string `json:"type"`
		RequestID string `json:"request_id"`
		Status    string `json:"status"`
	}
	lifetimeRead(t, ctx, alice, &result)
	if result.Type != "result" || result.RequestID != "after-deadline" || result.Status != "forwarded" {
		t.Fatalf("unexpected result after HTTP deadlines: %+v", result)
	}

	healthRequest, err := http.NewRequestWithContext(ctx, http.MethodGet, server.URL+"/healthz", nil)
	if err != nil {
		t.Fatal(err)
	}
	health, err := client.Do(healthRequest)
	if err != nil {
		t.Fatalf("shared listener health route: %v", err)
	}
	io.Copy(io.Discard, health.Body)
	health.Body.Close()
	if health.StatusCode != http.StatusOK {
		t.Fatalf("health status = %d", health.StatusCode)
	}
	body, err := json.Marshal(map[string]string{"username": "deadline-registration", "auth_public_key": base64.StdEncoding.EncodeToString(alicePublic)})
	if err != nil {
		t.Fatal(err)
	}
	request, err := http.NewRequestWithContext(ctx, http.MethodPost, server.URL+prefix+"/challenges", bytes.NewReader(body))
	if err != nil {
		t.Fatal(err)
	}
	request.Header.Set("Content-Type", "application/json")
	response, err := client.Do(request)
	if err != nil {
		t.Fatalf("shared listener registration challenge: %v", err)
	}
	defer response.Body.Close()
	var challenge account.Challenge
	if err := json.NewDecoder(io.LimitReader(response.Body, 4096)).Decode(&challenge); err != nil {
		t.Fatal(err)
	}
	if response.StatusCode != http.StatusCreated || challenge.ChallengeID == "" || challenge.Message == "" {
		t.Fatalf("registration challenge route did not remain available: status=%d", response.StatusCode)
	}
}

func lifetimeAuthenticate(t *testing.T, ctx context.Context, url, voidID string, key ed25519.PrivateKey) *websocket.Conn {
	t.Helper()
	conn, _, err := websocket.Dial(ctx, url, nil)
	if err != nil {
		t.Fatal(err)
	}
	t.Cleanup(func() { conn.CloseNow() })
	lifetimeWrite(t, ctx, conn, map[string]string{"type": "auth_begin", "void_id": voidID})
	var challenge struct {
		Type        string `json:"type"`
		ChallengeID string `json:"challenge_id"`
		Message     string `json:"message"`
	}
	lifetimeRead(t, ctx, conn, &challenge)
	message, err := base64.StdEncoding.Strict().DecodeString(challenge.Message)
	if err != nil || challenge.Type != "auth_challenge" {
		t.Fatal("invalid authentication challenge")
	}
	lifetimeWrite(t, ctx, conn, map[string]string{
		"type": "auth_proof", "challenge_id": challenge.ChallengeID,
		"signature": base64.StdEncoding.EncodeToString(ed25519.Sign(key, message)),
	})
	var result struct {
		Type   string `json:"type"`
		VoidID string `json:"void_id"`
	}
	lifetimeRead(t, ctx, conn, &result)
	if result.Type != "auth_ok" || result.VoidID != voidID {
		t.Fatalf("authentication failed for %s", voidID)
	}
	return conn
}

func lifetimeWrite(t *testing.T, ctx context.Context, conn *websocket.Conn, value any) {
	t.Helper()
	data, err := json.Marshal(value)
	if err != nil {
		t.Fatal(err)
	}
	if err := conn.Write(ctx, websocket.MessageText, data); err != nil {
		t.Fatal(err)
	}
}

func lifetimeRead(t *testing.T, ctx context.Context, conn *websocket.Conn, value any) {
	t.Helper()
	kind, data, err := conn.Read(ctx)
	if err != nil {
		t.Fatal(err)
	}
	if kind != websocket.MessageText {
		t.Fatal("expected a JSON text frame")
	}
	if err := json.Unmarshal(data, value); err != nil {
		t.Fatal(err)
	}
}

package relay

import (
	"bytes"
	"context"
	"crypto/ed25519"
	"crypto/rand"
	"encoding/base64"
	"encoding/json"
	"fmt"
	"io"
	"log/slog"
	"net/http"
	"net/http/httptest"
	"reflect"
	"strconv"
	"strings"
	"sync"
	"testing"
	"time"

	"github.com/coder/websocket"

	"void-nothingness/internal/auth"
)

type keyLookup map[int64]ed25519.PublicKey

func (keys keyLookup) AuthenticationKey(ctx context.Context, id int64) ([]byte, error) {
	if ctx.Err() != nil {
		return nil, ctx.Err()
	}
	key, ok := keys[id]
	if !ok {
		return nil, auth.ErrNotFound
	}
	return key, nil
}

type fixture struct {
	server *Server
	http   *httptest.Server
	keys   map[string]ed25519.PrivateKey
}

func newFixture(t *testing.T, opts Options, count int) *fixture {
	t.Helper()
	lookup := make(keyLookup, count)
	private := make(map[string]ed25519.PrivateKey, count)
	for i := 1; i <= count; i++ {
		public, key, err := ed25519.GenerateKey(rand.Reader)
		if err != nil {
			t.Fatal(err)
		}
		lookup[int64(i)] = public
		private[strconv.Itoa(i)] = key
	}
	authenticator, err := auth.NewService(lookup, 10*time.Second)
	if err != nil {
		t.Fatal(err)
	}
	if opts.Logger == nil {
		opts.Logger = slog.New(slog.NewTextHandler(io.Discard, nil))
	}
	server, err := New(authenticator, opts)
	if err != nil {
		t.Fatal(err)
	}
	httpServer := httptest.NewServer(server)
	t.Cleanup(func() { _ = server.Close(); httpServer.Close() })
	return &fixture{server: server, http: httpServer, keys: private}
}

func (f *fixture) dial(t *testing.T) *websocket.Conn {
	t.Helper()
	ctx, cancel := context.WithTimeout(context.Background(), 2*time.Second)
	defer cancel()
	conn, _, err := websocket.Dial(ctx, "ws"+strings.TrimPrefix(f.http.URL, "http"), nil)
	if err != nil {
		t.Fatal(err)
	}
	t.Cleanup(func() { _ = conn.CloseNow() })
	return conn
}

func write(t *testing.T, conn *websocket.Conn, value any) {
	t.Helper()
	body, err := json.Marshal(value)
	if err != nil {
		t.Fatal(err)
	}
	writeRaw(t, conn, websocket.MessageText, body)
}

func writeRaw(t *testing.T, conn *websocket.Conn, kind websocket.MessageType, body []byte) {
	t.Helper()
	ctx, cancel := context.WithTimeout(context.Background(), 2*time.Second)
	defer cancel()
	if err := conn.Write(ctx, kind, body); err != nil {
		t.Fatal(err)
	}
}

func read(t *testing.T, conn *websocket.Conn) map[string]string {
	t.Helper()
	ctx, cancel := context.WithTimeout(context.Background(), 2*time.Second)
	defer cancel()
	kind, body, err := conn.Read(ctx)
	if err != nil {
		t.Fatal(err)
	}
	if kind != websocket.MessageText {
		t.Fatalf("message kind = %v", kind)
	}
	var fields map[string]string
	if err := json.Unmarshal(body, &fields); err != nil {
		t.Fatal(err)
	}
	return fields
}

func assertFields(t *testing.T, actual, expected map[string]string) {
	t.Helper()
	if !reflect.DeepEqual(actual, expected) {
		t.Fatalf("frame = %#v; want %#v", actual, expected)
	}
}

func begin(t *testing.T, conn *websocket.Conn, id string) map[string]string {
	t.Helper()
	write(t, conn, map[string]string{"type": "auth_begin", "void_id": id})
	challenge := read(t, conn)
	if challenge["type"] != "auth_challenge" || challenge["challenge_id"] == "" || challenge["message"] == "" || challenge["expires_at"] == "" || len(challenge) != 4 {
		t.Fatalf("invalid challenge envelope: %#v", challenge)
	}
	return challenge
}

func prove(t *testing.T, conn *websocket.Conn, challenge map[string]string, key ed25519.PrivateKey) {
	t.Helper()
	message, err := base64.StdEncoding.DecodeString(challenge["message"])
	if err != nil {
		t.Fatal(err)
	}
	sig := ed25519.Sign(key, message)
	write(t, conn, map[string]string{"type": "auth_proof", "challenge_id": challenge["challenge_id"], "signature": base64.StdEncoding.EncodeToString(sig)})
}

func (f *fixture) authenticate(t *testing.T, id string) *websocket.Conn {
	t.Helper()
	conn := f.dial(t)
	challenge := begin(t, conn, id)
	prove(t, conn, challenge, f.keys[id])
	assertFields(t, read(t, conn), map[string]string{"type": "auth_ok", "void_id": id})
	// auth_ok must mean registry publication is already complete.
	f.server.mu.Lock()
	active := f.server.active[id]
	f.server.mu.Unlock()
	if active == nil {
		t.Fatal("auth_ok received before session became active")
	}
	return conn
}

func send(id, recipient, payload string) map[string]string {
	return map[string]string{"type": "send", "request_id": id, "recipient_id": recipient, "payload": payload}
}

func eventually(t *testing.T, description string, predicate func() bool) {
	t.Helper()
	deadline := time.Now().Add(2 * time.Second)
	for !predicate() {
		if time.Now().After(deadline) {
			t.Fatal(description)
		}
		time.Sleep(time.Millisecond)
	}
}

func connectionCount(s *Server) int {
	s.mu.Lock()
	defer s.mu.Unlock()
	return len(s.connections)
}

func TestAuthenticatedRelayAndOfflineRecipient(t *testing.T) {
	f := newFixture(t, Options{}, 2)
	alice := f.authenticate(t, "1")
	bob := f.authenticate(t, "2")
	if f.server.ActiveCount() != 2 {
		t.Fatal("both sessions must be active")
	}
	opaque := []byte{0, 255, 9, 18, 0, 33, 128}
	payload := base64.StdEncoding.EncodeToString(opaque)
	write(t, alice, send("opaque:001", "2", payload))
	assertFields(t, read(t, bob), map[string]string{"type": "message", "request_id": "opaque:001", "sender_id": "1", "payload": payload})
	assertFields(t, read(t, alice), map[string]string{"type": "result", "request_id": "opaque:001", "status": "forwarded"})
	_ = bob.CloseNow()
	eventually(t, "disconnected recipient remained registered", func() bool { return f.server.ActiveCount() == 1 })
	write(t, alice, send("offline:002", "2", payload))
	assertFields(t, read(t, alice), map[string]string{"type": "result", "request_id": "offline:002", "status": "unavailable"})
	_ = alice.CloseNow()
	eventually(t, "session cleanup failed", func() bool { return f.server.ActiveCount() == 0 && connectionCount(f.server) == 0 })
}

func TestUnauthorizedAndFailedAuthentication(t *testing.T) {
	f := newFixture(t, Options{}, 2)
	t.Run("unauthenticated send", func(t *testing.T) {
		conn := f.dial(t)
		write(t, conn, send("attempt", "2", "AQ=="))
		assertFields(t, read(t, conn), map[string]string{"type": "error", "code": "unauthorized"})
	})
	t.Run("wrong private key", func(t *testing.T) {
		conn := f.dial(t)
		challenge := begin(t, conn, "1")
		prove(t, conn, challenge, f.keys["2"])
		assertFields(t, read(t, conn), map[string]string{"type": "error", "code": "authentication_failed"})
	})
	t.Run("unknown account", func(t *testing.T) {
		conn := f.dial(t)
		write(t, conn, map[string]string{"type": "auth_begin", "void_id": "3"})
		assertFields(t, read(t, conn), map[string]string{"type": "error", "code": "authentication_failed"})
	})
	t.Run("proof from another socket", func(t *testing.T) {
		first, second := f.dial(t), f.dial(t)
		challenge := begin(t, first, "1")
		_ = begin(t, second, "1")
		prove(t, second, challenge, f.keys["1"])
		assertFields(t, read(t, second), map[string]string{"type": "error", "code": "authentication_failed"})
		prove(t, first, challenge, f.keys["1"])
		assertFields(t, read(t, first), map[string]string{"type": "auth_ok", "void_id": "1"})
		// An authenticated socket cannot authenticate again or replay its proof.
		prove(t, first, challenge, f.keys["1"])
		assertFields(t, read(t, first), map[string]string{"type": "error", "code": "invalid_frame"})
	})
	eventually(t, "failed/closed authentication leaked session", func() bool { return f.server.ActiveCount() == 0 })
}

func TestStrictFramesAndSenderImpersonation(t *testing.T) {
	f := newFixture(t, Options{MaxPayloadBytes: 4}, 2)
	cases := []struct {
		name, body string
		binary     bool
	}{
		{"sender impersonation", `{"type":"send","request_id":"x","recipient_id":"2","payload":"AQ==","sender_id":"2"}`, false},
		{"unknown field", `{"type":"send","request_id":"x","recipient_id":"2","other":"AQ=="}`, false},
		{"duplicate field", `{"type":"send","request_id":"x","recipient_id":"2","recipient_id":"1"}`, false},
		{"null field", `{"type":"send","request_id":"x","recipient_id":"2","payload":null}`, false},
		{"multiple values", `{"type":"send","request_id":"x","recipient_id":"2","payload":"AQ=="} {}`, false},
		{"field case", `{"Type":"send","request_id":"x","recipient_id":"2","payload":"AQ=="}`, false},
		{"unknown type", `{"type":"ack"}`, false},
		{"not object", `[]`, false},
		{"invalid utf8", "{\"type\":\"\xff\"}", false},
		{"binary", `{"type":"send","request_id":"x","recipient_id":"2","payload":"AQ=="}`, true},
		{"over payload", `{"type":"send","request_id":"x","recipient_id":"2","payload":"AQIDBAU="}`, false},
		{"unpadded base64", `{"type":"send","request_id":"x","recipient_id":"2","payload":"AQ"}`, false},
		{"base64 whitespace", `{"type":"send","request_id":"x","recipient_id":"2","payload":"AQ==\n"}`, false},
		{"noncanonical base64", `{"type":"send","request_id":"x","recipient_id":"2","payload":"AR=="}`, false},
		{"empty payload", `{"type":"send","request_id":"x","recipient_id":"2","payload":""}`, false},
		{"invalid recipient", `{"type":"send","request_id":"x","recipient_id":"02","payload":"AQ=="}`, false},
		{"invalid request id", `{"type":"send","request_id":"has space","recipient_id":"2","payload":"AQ=="}`, false},
		{"long request id", fmt.Sprintf(`{"type":"send","request_id":%q,"recipient_id":"2","payload":"AQ=="}`, strings.Repeat("a", 65)), false},
		{"second authentication", `{"type":"auth_begin","void_id":"1"}`, false},
	}
	for _, test := range cases {
		t.Run(test.name, func(t *testing.T) {
			conn := f.authenticate(t, "1")
			kind := websocket.MessageText
			if test.binary {
				kind = websocket.MessageBinary
			}
			writeRaw(t, conn, kind, []byte(test.body))
			assertFields(t, read(t, conn), map[string]string{"type": "error", "code": "invalid_frame"})
		})
	}
}

func TestNewSuccessfulSessionReplacesOldAndStaleCleanupIsSafe(t *testing.T) {
	f := newFixture(t, Options{}, 2)
	old := f.authenticate(t, "1")
	bob := f.authenticate(t, "2")
	f.server.mu.Lock()
	oldSession := f.server.active["1"]
	f.server.mu.Unlock()
	failed := f.dial(t)
	challenge := begin(t, failed, "1")
	prove(t, failed, challenge, f.keys["2"])
	assertFields(t, read(t, failed), map[string]string{"type": "error", "code": "authentication_failed"})
	write(t, bob, send("old-remains", "1", "AQ=="))
	assertFields(t, read(t, old), map[string]string{"type": "message", "request_id": "old-remains", "sender_id": "2", "payload": "AQ=="})
	assertFields(t, read(t, bob), map[string]string{"type": "result", "request_id": "old-remains", "status": "forwarded"})
	newest := f.authenticate(t, "1")
	ctx, cancel := context.WithTimeout(context.Background(), time.Second)
	defer cancel()
	if _, _, err := old.Read(ctx); err == nil {
		t.Fatal("previous socket remained usable")
	}
	eventually(t, "old upgraded socket did not clean up", func() bool { return connectionCount(f.server) == 2 })
	if f.server.ActiveCount() != 2 {
		t.Fatal("stale cleanup removed replacement")
	}
	if status := oldSession.forward(send("stale", "2", "AQ==")); status != "failed" {
		t.Fatalf("stale sender forwarded: %s", status)
	}
	write(t, bob, send("replacement", "1", "Ag=="))
	assertFields(t, read(t, newest), map[string]string{"type": "message", "request_id": "replacement", "sender_id": "2", "payload": "Ag=="})
	assertFields(t, read(t, bob), map[string]string{"type": "result", "request_id": "replacement", "status": "forwarded"})
}

func TestAuthenticationTimeoutAndFrameLimit(t *testing.T) {
	t.Run("authentication timeout", func(t *testing.T) {
		f := newFixture(t, Options{AuthTimeout: 40 * time.Millisecond}, 1)
		conn := f.dial(t)
		ctx, cancel := context.WithTimeout(context.Background(), time.Second)
		defer cancel()
		if _, _, err := conn.Read(ctx); err == nil {
			t.Fatal("unauthenticated socket stayed open")
		}
		eventually(t, "timed-out socket leaked admission", func() bool { return connectionCount(f.server) == 0 })
	})
	t.Run("incoming frame limit", func(t *testing.T) {
		f := newFixture(t, Options{}, 1)
		conn := f.authenticate(t, "1")
		writeRaw(t, conn, websocket.MessageText, bytes.Repeat([]byte(" "), DefaultMaxFrameBytes+1))
		ctx, cancel := context.WithTimeout(context.Background(), time.Second)
		defer cancel()
		if _, _, err := conn.Read(ctx); err == nil {
			t.Fatal("oversized frame accepted")
		}
		eventually(t, "oversized frame socket leaked", func() bool { return f.server.ActiveCount() == 0 })
	})
}

func TestHeartbeatClosesUnresponsiveClient(t *testing.T) {
	f := newFixture(t, Options{PingInterval: 40 * time.Millisecond, PongTimeout: 30 * time.Millisecond}, 1)
	_ = f.authenticate(t, "1")
	// Coder's client handles ping/pong while Read is active. Do not read here,
	// simulating a peer that stops processing frames after authenticating.
	eventually(t, "heartbeat did not evict unresponsive peer", func() bool { return f.server.ActiveCount() == 0 })
}

func TestAdmissionShutdownAndOrigin(t *testing.T) {
	f := newFixture(t, Options{MaxConnections: 2}, 1)
	first, second := f.dial(t), f.dial(t)
	ctx, cancel := context.WithTimeout(context.Background(), 2*time.Second)
	defer cancel()
	url := "ws" + strings.TrimPrefix(f.http.URL, "http")
	conn, response, err := websocket.Dial(ctx, url, nil)
	if conn != nil {
		_ = conn.CloseNow()
	}
	if err == nil || response == nil || response.StatusCode != http.StatusServiceUnavailable {
		t.Fatalf("full admission: response=%v err=%v", response, err)
	}
	_ = first.CloseNow()
	eventually(t, "unauthenticated socket did not release admission", func() bool { return connectionCount(f.server) == 1 })
	third := f.dial(t)
	var closers sync.WaitGroup
	for i := 0; i < 4; i++ {
		closers.Add(1)
		go func() { defer closers.Done(); _ = f.server.Close() }()
	}
	closers.Wait()
	for _, client := range []*websocket.Conn{second, third} {
		if _, _, err := client.Read(ctx); err == nil {
			t.Fatal("Close left unauthenticated socket open")
		}
	}
	eventually(t, "shutdown leaked socket", func() bool { return connectionCount(f.server) == 0 })
	_, response, err = websocket.Dial(ctx, url, nil)
	if err == nil || response == nil || response.StatusCode != http.StatusServiceUnavailable {
		t.Fatal("closed server accepted new upgrade")
	}

	t.Run("origin", func(t *testing.T) {
		other := newFixture(t, Options{}, 1)
		otherURL := "ws" + strings.TrimPrefix(other.http.URL, "http")
		_, resp, err := websocket.Dial(ctx, otherURL, &websocket.DialOptions{HTTPHeader: http.Header{"Origin": []string{"https://unrelated.example"}}})
		if err == nil || resp == nil || resp.StatusCode != http.StatusForbidden {
			t.Fatal("cross-origin upgrade accepted")
		}
		eventually(t, "rejected upgrade leaked admission", func() bool { return connectionCount(other.server) == 0 })
		same, _, err := websocket.Dial(ctx, otherURL, &websocket.DialOptions{HTTPHeader: http.Header{"Origin": []string{other.http.URL}}})
		if err != nil {
			t.Fatal(err)
		}
		_ = same.CloseNow()
	})
}

func TestConcurrentClientsForwardWithoutLosingEnvelopes(t *testing.T) {
	const count = 12
	f := newFixture(t, Options{QueueSize: 16}, count+1)
	recipient := f.authenticate(t, "1")
	clients := make([]*websocket.Conn, count)
	for i := range clients {
		clients[i] = f.authenticate(t, strconv.Itoa(i+2))
	}
	errors := make(chan error, count)
	var workers sync.WaitGroup
	for i, client := range clients {
		workers.Add(1)
		go func() {
			defer workers.Done()
			ctx, cancel := context.WithTimeout(context.Background(), 2*time.Second)
			defer cancel()
			requestID := fmt.Sprintf("parallel-%d", i)
			body, _ := json.Marshal(send(requestID, "1", "AAECAw=="))
			if err := client.Write(ctx, websocket.MessageText, body); err != nil {
				errors <- err
				return
			}
			_, body, err := client.Read(ctx)
			if err != nil {
				errors <- err
				return
			}
			var result map[string]string
			if err = json.Unmarshal(body, &result); err != nil {
				errors <- err
				return
			}
			if result["status"] != "forwarded" || result["request_id"] != requestID {
				errors <- fmt.Errorf("result = %#v", result)
			}
		}()
	}
	seen := make(map[string]bool, count)
	for i := 0; i < count; i++ {
		message := read(t, recipient)
		if message["type"] != "message" || message["payload"] != "AAECAw==" || seen[message["sender_id"]] {
			t.Fatalf("bad concurrent envelope: %#v", message)
		}
		seen[message["sender_id"]] = true
	}
	workers.Wait()
	close(errors)
	for err := range errors {
		t.Error(err)
	}
}

func TestConfigurationAndHTTPMethod(t *testing.T) {
	f := newFixture(t, Options{}, 1)
	request, err := http.NewRequest(http.MethodPost, f.http.URL, nil)
	if err != nil {
		t.Fatal(err)
	}
	response, err := http.DefaultClient.Do(request)
	if err != nil {
		t.Fatal(err)
	}
	defer response.Body.Close()
	if response.StatusCode != http.StatusMethodNotAllowed || response.Header.Get("Allow") != "GET" {
		t.Fatal("method restriction failed")
	}
	for _, opts := range []Options{{MaxConnections: -1}, {QueueSize: -1}, {MaxPayloadBytes: DefaultMaxPayloadBytes + 1}, {MaxFrameBytes: DefaultMaxFrameBytes + 1}, {AuthTimeout: 11 * time.Second}, {WriteTimeout: -time.Second}} {
		if _, err := New(f.server.auth, opts); err == nil {
			t.Errorf("invalid configuration accepted: %+v", opts)
		}
	}
	if _, err := New(nil, Options{}); err == nil {
		t.Fatal("nil authenticator accepted")
	}
}

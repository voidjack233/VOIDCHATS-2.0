// Command void-demo registers two disposable accounts and exercises the real
// WebSocket relay on a loopback-only test server. It never reads .env, opens a
// database, contacts a public host, or saves authentication keys.
package main

import (
	"bytes"
	"context"
	"crypto/ed25519"
	"crypto/rand"
	"encoding/base64"
	"encoding/json"
	"errors"
	"fmt"
	"io"
	"net/http"
	"net/http/httptest"
	"os"
	"strconv"
	"strings"
	"sync"
	"sync/atomic"
	"time"

	"github.com/coder/websocket"

	"void-nothingness/internal/account"
	"void-nothingness/internal/auth"
	"void-nothingness/internal/httpapi"
	"void-nothingness/internal/queue"
	"void-nothingness/internal/relay"
)

const (
	apiPrefix = "/dm9pZG5vdGhpbmduZXNz"
	// These bytes are a fixed demonstration fixture, not an encryption format.
	simulatedCiphertext = "SIMULATED-CIPHERTEXT:alice-to-bob:opaque-demo-bytes"
	replyCiphertext     = "SIMULATED-CIPHERTEXT:bob-to-alice:opaque-reply-bytes"
	forwardEventID      = "00000000-0000-4000-8000-000000000001"
	replyEventID        = "00000000-0000-4000-8000-000000000002"
	offlineEventID      = "00000000-0000-4000-8000-000000000003"
)

type storedAccount struct {
	account.Account
	publicKey []byte
}

// memoryAccounts supplies the same registration and key lookup interfaces as
// PostgreSQL without touching any real account records.
type memoryAccounts struct {
	mu    sync.Mutex
	byID  map[int64]storedAccount
	byKey map[string]struct{}
}

func (m *memoryAccounts) Insert(ctx context.Context, id int64, key []byte) (account.Account, error) {
	m.mu.Lock()
	defer m.mu.Unlock()
	if err := ctx.Err(); err != nil {
		return account.Account{}, err
	}
	if _, ok := m.byID[id]; ok {
		return account.Account{}, account.ErrIDCollision
	}
	if _, ok := m.byKey[string(key)]; ok {
		return account.Account{}, account.ErrDuplicateKey
	}
	result := account.Account{VoidID: strconv.FormatInt(id, 10), CreatedAt: time.Now().UTC()}
	m.byID[id] = storedAccount{Account: result, publicKey: bytes.Clone(key)}
	m.byKey[string(key)] = struct{}{}
	return result, nil
}

func (m *memoryAccounts) AuthenticationKey(ctx context.Context, id int64) ([]byte, error) {
	m.mu.Lock()
	defer m.mu.Unlock()
	if err := ctx.Err(); err != nil {
		return nil, err
	}
	row, ok := m.byID[id]
	if !ok {
		return nil, auth.ErrNotFound
	}
	return bytes.Clone(row.publicKey), nil
}

type demoIDs struct{ next atomic.Int64 }

func (d *demoIDs) Next(ctx context.Context) (int64, error) {
	if err := ctx.Err(); err != nil {
		return 0, err
	}
	return d.next.Add(1), nil
}

type demoUser struct {
	account    account.Account
	privateKey ed25519.PrivateKey
}

type authEnvelope struct {
	Type        string    `json:"type"`
	VoidID      string    `json:"void_id"`
	ChallengeID string    `json:"challenge_id"`
	Message     string    `json:"message"`
	ExpiresAt   time.Time `json:"expires_at"`
}

type relayEnvelope struct {
	Type         string    `json:"type"`
	EventID      string    `json:"event_id"`
	FromVoidID   string    `json:"from_void_id,omitempty"`
	SenderVoidID string    `json:"sender_void_id,omitempty"`
	Payload      string    `json:"payload_base64,omitempty"`
	CreatedAt    time.Time `json:"created_at,omitempty"`
	ExpiresAt    time.Time `json:"expires_at,omitempty"`
	Code         string    `json:"code,omitempty"`
}

// demoReport retains observations from actual network exchanges for the test.
// Keys, signatures, and challenge material are deliberately excluded.
type demoReport struct {
	Alice, Bob           account.Account
	Active               int
	Forward, Reverse     relayEnvelope
	Forwarded, Replied   relayEnvelope
	Offline, Reconnected relayEnvelope
	FinalActive          int
	FinalPending         int
}

func main() {
	if err := run(os.Stdout); err != nil {
		fmt.Fprintf(os.Stderr, "void-demo: %v\n", err)
		os.Exit(1)
	}
}

func run(out io.Writer) error {
	ctx, cancel := context.WithTimeout(context.Background(), 15*time.Second)
	defer cancel()
	_, err := runDemo(ctx, out)
	return err
}

func runDemo(ctx context.Context, out io.Writer) (demoReport, error) {
	var report demoReport
	log := &demoLog{out: out}
	log.stage("Local demonstration: in-memory accounts and ephemeral keys; no real database writes or network configuration changes.")
	repo := &memoryAccounts{byID: make(map[int64]storedAccount), byKey: make(map[string]struct{})}
	registration, err := account.NewService(repo, &demoIDs{}, account.Options{ChallengeTTL: time.Minute, MaxChallenges: 8})
	if err != nil {
		return report, fmt.Errorf("initialize registration: %w", err)
	}
	authentication, err := auth.NewService(repo, 10*time.Second)
	if err != nil {
		return report, fmt.Errorf("initialize authentication: %w", err)
	}
	mux := http.NewServeMux()
	mux.Handle("/", httpapi.New(registration, func(context.Context) error { return nil }, httpapi.Options{}))
	server := httptest.NewServer(mux)
	defer server.Close()
	client := server.Client()
	client.Timeout = 5 * time.Second
	defer client.CloseIdleConnections()

	alice, err := registerUser(ctx, client, server.URL)
	if err != nil {
		return report, fmt.Errorf("register Alice through HTTP challenge/proof: %w", err)
	}
	bob, err := registerUser(ctx, client, server.URL)
	if err != nil {
		return report, fmt.Errorf("register Bob through HTTP challenge/proof: %w", err)
	}
	report.Alice, report.Bob = alice.account, bob.account
	if alice.account.VoidID == bob.account.VoidID {
		return report, errors.New("registration returned duplicate account identities")
	}
	// Only these two synthetic identities exist in this demonstration queue.
	// This setup runs after HTTP registration, without changing production APIs.
	messageStore := queue.NewMemoryStore([]queue.Person{
		{VoidID: alice.account.VoidID, AuthPublicKey: base64.StdEncoding.EncodeToString(alice.privateKey.Public().(ed25519.PublicKey))},
		{VoidID: bob.account.VoidID, AuthPublicKey: base64.StdEncoding.EncodeToString(bob.privateKey.Public().(ed25519.PublicKey))},
	})
	messages, err := queue.New(messageStore)
	if err != nil {
		return report, fmt.Errorf("initialize temporary queue: %w", err)
	}
	relayServer, err := relay.New(authentication, relay.Options{Queue: messages})
	if err != nil {
		return report, fmt.Errorf("initialize relay: %w", err)
	}
	mux.Handle(apiPrefix+"/ws", relayServer)
	// Upgraded connections outlive HTTP requests; close the relay before HTTP.
	defer relayServer.Close()
	log.stage("Registered two disposable accounts through the existing HTTP registration handlers.")
	wsURL := "ws" + strings.TrimPrefix(server.URL, "http") + apiPrefix + "/ws"
	aliceConn, err := connectUser(ctx, wsURL, alice)
	if err != nil {
		return report, fmt.Errorf("authenticate Alice: %w", err)
	}
	defer aliceConn.CloseNow()
	log.envelope("Alice authenticated", map[string]string{"type": "auth_ok", "void_id": alice.account.VoidID})
	bobConn, err := connectUser(ctx, wsURL, bob)
	if err != nil {
		return report, fmt.Errorf("authenticate Bob: %w", err)
	}
	defer bobConn.CloseNow()
	log.envelope("Bob authenticated", map[string]string{"type": "auth_ok", "void_id": bob.account.VoidID})
	if err := waitActive(ctx, relayServer, 2); err != nil {
		return report, err
	}
	report.Active = relayServer.ActiveCount()
	log.stage("Both authenticated WebSocket connections are active.")

	report.Forward, report.Forwarded, err = exchange(ctx, aliceConn, bobConn, alice.account.VoidID, bob.account.VoidID, forwardEventID, simulatedCiphertext)
	if err != nil {
		return report, fmt.Errorf("Alice to Bob exchange: %w", err)
	}
	log.envelope("Bob received simulated ciphertext", report.Forward)
	log.envelope("Alice received queue acceptance", report.Forwarded)
	report.Reverse, report.Replied, err = exchange(ctx, bobConn, aliceConn, bob.account.VoidID, alice.account.VoidID, replyEventID, replyCiphertext)
	if err != nil {
		return report, fmt.Errorf("Bob to Alice exchange: %w", err)
	}
	log.envelope("Alice received simulated ciphertext", report.Reverse)
	log.envelope("Bob received queue acceptance", report.Replied)
	log.stage("accepted means the temporary queue stored an event; recipient ACK separately deletes it. This demo uses memory, not durable PostgreSQL.")

	if err := bobConn.Close(websocket.StatusNormalClosure, "demo recipient disconnect"); err != nil {
		return report, fmt.Errorf("disconnect Bob: %w", err)
	}
	if err := waitActive(ctx, relayServer, 1); err != nil {
		return report, err
	}
	log.stage("Bob disconnected; his authenticated connection was removed.")
	if err := writeFrame(ctx, aliceConn, map[string]string{
		"type": "send", "event_id": offlineEventID, "to_void_id": bob.account.VoidID,
		"payload_base64": base64.StdEncoding.EncodeToString([]byte(simulatedCiphertext)),
	}); err != nil {
		return report, fmt.Errorf("send to disconnected recipient: %w", err)
	}
	if err := readFrame(ctx, aliceConn, &report.Offline); err != nil {
		return report, fmt.Errorf("read offline queue acceptance: %w", err)
	}
	if report.Offline.Type != "accepted" || report.Offline.EventID != offlineEventID {
		return report, errors.New("event for disconnected recipient was not accepted into the queue")
	}
	log.envelope("Alice received acceptance while Bob was offline", report.Offline)
	reconnected, err := connectUser(ctx, wsURL, bob)
	if err != nil {
		return report, fmt.Errorf("reconnect Bob: %w", err)
	}
	defer reconnected.CloseNow()
	if err = readFrame(ctx, reconnected, &report.Reconnected); err != nil {
		return report, fmt.Errorf("read queued event after reconnect: %w", err)
	}
	if report.Reconnected.Type != "event" || report.Reconnected.EventID != offlineEventID || report.Reconnected.FromVoidID != alice.account.VoidID || report.Reconnected.Payload != base64.StdEncoding.EncodeToString([]byte(simulatedCiphertext)) {
		return report, errors.New("queued event changed while recipient was offline")
	}
	if err = acknowledge(ctx, reconnected, report.Reconnected); err != nil {
		return report, fmt.Errorf("acknowledge reconnect delivery: %w", err)
	}
	log.envelope("Bob received and acknowledged the queued event after reconnect", report.Reconnected)
	for _, id := range []string{alice.account.VoidID, bob.account.VoidID} {
		pending, err := messages.Pending(ctx, id, 100)
		if err != nil {
			return report, err
		}
		report.FinalPending += len(pending)
	}
	if report.FinalPending != 0 {
		return report, errors.New("acknowledged ciphertext remained in the temporary queue")
	}
	if err = reconnected.Close(websocket.StatusNormalClosure, "demo complete"); err != nil {
		return report, fmt.Errorf("disconnect reconnected Bob: %w", err)
	}
	if err := aliceConn.Close(websocket.StatusNormalClosure, "demo complete"); err != nil {
		return report, fmt.Errorf("disconnect Alice: %w", err)
	}
	if err := waitActive(ctx, relayServer, 0); err != nil {
		return report, err
	}
	report.FinalActive = relayServer.ActiveCount()
	log.stage("All simulated clients disconnected; recipient ACKs removed every queued event and no active sessions remain.")
	return report, log.err
}

func registerUser(ctx context.Context, client *http.Client, baseURL string) (demoUser, error) {
	publicKey, privateKey, err := ed25519.GenerateKey(rand.Reader)
	if err != nil {
		return demoUser{}, errors.New("generate ephemeral authentication key")
	}
	var challenge account.Challenge
	if err := postJSON(ctx, client, baseURL+apiPrefix+"/challenges", map[string]string{
		"auth_public_key": base64.StdEncoding.EncodeToString(publicKey),
	}, &challenge); err != nil {
		return demoUser{}, err
	}
	message, err := registrationSigningMessage(challenge, publicKey)
	if err != nil {
		return demoUser{}, err
	}
	var result account.Account
	if err := postJSON(ctx, client, baseURL+apiPrefix+"/accounts", map[string]string{
		"challenge_id": challenge.ChallengeID,
		"signature":    base64.StdEncoding.EncodeToString(ed25519.Sign(privateKey, message)),
	}, &result); err != nil {
		return demoUser{}, err
	}
	if !auth.ValidVoidID(result.VoidID) || result.CreatedAt.IsZero() {
		return demoUser{}, errors.New("registration returned an invalid account identity")
	}
	return demoUser{account: result, privateKey: privateKey}, nil
}

func postJSON(ctx context.Context, client *http.Client, url string, value, result any) error {
	data, err := json.Marshal(value)
	if err != nil {
		return err
	}
	request, err := http.NewRequestWithContext(ctx, http.MethodPost, url, bytes.NewReader(data))
	if err != nil {
		return err
	}
	request.Header.Set("Content-Type", "application/json")
	response, err := client.Do(request)
	if err != nil {
		return err
	}
	defer response.Body.Close()
	if response.StatusCode != http.StatusCreated {
		return fmt.Errorf("registration HTTP status %d", response.StatusCode)
	}
	return json.NewDecoder(io.LimitReader(response.Body, 8192)).Decode(result)
}

func registrationSigningMessage(challenge account.Challenge, publicKey ed25519.PublicKey) ([]byte, error) {
	message, err := decodeChallengeMessage(challenge.ChallengeID, challenge.Message, challenge.ExpiresAt)
	if err != nil {
		return nil, err
	}
	fields := strings.Split(string(message), "\n")
	if len(fields) != 7 || fields[0] != "VOID-NOTHINGNESS:register:v1" || fields[1] != challenge.ChallengeID ||
		fields[2] != challenge.ExpiresAt.UTC().Format(time.RFC3339Nano) || fields[3] != base64.StdEncoding.EncodeToString(publicKey) ||
		fields[4] != "" || !validChallengeToken(fields[5]) || fields[6] != "" {
		return nil, errors.New("registration challenge does not bind the requested account")
	}
	return message, nil
}

func authenticationSigningMessage(challenge authEnvelope, voidID string) ([]byte, error) {
	message, err := decodeChallengeMessage(challenge.ChallengeID, challenge.Message, challenge.ExpiresAt)
	if err != nil {
		return nil, err
	}
	fields := strings.Split(string(message), "\n")
	if challenge.Type != "auth_challenge" || len(fields) != 6 || fields[0] != "VOID-NOTHINGNESS:authenticate:v1" ||
		fields[1] != challenge.ChallengeID || fields[2] != voidID || fields[3] != challenge.ExpiresAt.UTC().Format(time.RFC3339Nano) ||
		!validChallengeToken(fields[4]) || fields[5] != "" {
		return nil, errors.New("authentication challenge does not bind the requested identity")
	}
	return message, nil
}

func decodeChallengeMessage(id, encoded string, expires time.Time) ([]byte, error) {
	message, err := base64.StdEncoding.Strict().DecodeString(encoded)
	if err != nil || base64.StdEncoding.EncodeToString(message) != encoded || !validChallengeToken(id) ||
		!time.Now().Before(expires) || expires.After(time.Now().Add(2*time.Minute)) {
		return nil, errors.New("invalid or expired signing challenge")
	}
	return message, nil
}

func validChallengeToken(encoded string) bool {
	decoded, err := base64.RawURLEncoding.Strict().DecodeString(encoded)
	return err == nil && len(decoded) == 32 && base64.RawURLEncoding.EncodeToString(decoded) == encoded
}

func connectUser(ctx context.Context, url string, user demoUser) (*websocket.Conn, error) {
	conn, _, err := websocket.Dial(ctx, url, nil)
	if err != nil {
		return nil, err
	}
	ok := false
	defer func() {
		if !ok {
			conn.CloseNow()
		}
	}()
	if err := writeFrame(ctx, conn, map[string]string{"type": "auth_begin", "void_id": user.account.VoidID}); err != nil {
		return nil, err
	}
	var challenge authEnvelope
	if err := readFrame(ctx, conn, &challenge); err != nil {
		return nil, err
	}
	message, err := authenticationSigningMessage(challenge, user.account.VoidID)
	if err != nil {
		return nil, err
	}
	if err := writeFrame(ctx, conn, map[string]string{
		"type": "auth_proof", "challenge_id": challenge.ChallengeID,
		"signature": base64.StdEncoding.EncodeToString(ed25519.Sign(user.privateKey, message)),
	}); err != nil {
		return nil, err
	}
	var result authEnvelope
	if err := readFrame(ctx, conn, &result); err != nil {
		return nil, err
	}
	if result.Type != "auth_ok" || result.VoidID != user.account.VoidID {
		return nil, errors.New("server did not authenticate the requested identity")
	}
	ok = true
	return conn, nil
}

func exchange(ctx context.Context, sender, recipient *websocket.Conn, senderID, recipientID, eventID, opaque string) (relayEnvelope, relayEnvelope, error) {
	// Read the recipient concurrently with the sender's result. Every goroutine
	// exits before this function returns, including on a failed write or read.
	readCtx, cancel := context.WithCancel(ctx)
	defer cancel()
	type received struct {
		frame relayEnvelope
		err   error
	}
	incoming := make(chan received, 1)
	go func() {
		var frame relayEnvelope
		err := readFrame(readCtx, recipient, &frame)
		incoming <- received{frame: frame, err: err}
	}()
	encoded := base64.StdEncoding.EncodeToString([]byte(opaque))
	var result relayEnvelope
	err := writeFrame(ctx, sender, map[string]string{
		"type": "send", "event_id": eventID, "to_void_id": recipientID, "payload_base64": encoded,
	})
	if err == nil {
		err = readFrame(ctx, sender, &result)
	}
	if err != nil {
		cancel()
		<-incoming
		return relayEnvelope{}, result, err
	}
	if result.Type != "accepted" || result.EventID != eventID || result.CreatedAt.IsZero() || !result.ExpiresAt.Equal(result.CreatedAt.Add(queue.Retention)) {
		cancel()
		<-incoming
		return relayEnvelope{}, result, errors.New("server did not acknowledge temporary queue storage")
	}
	receivedMessage := <-incoming
	if receivedMessage.err != nil {
		return receivedMessage.frame, result, receivedMessage.err
	}
	message := receivedMessage.frame
	if message.Type != "event" || message.EventID != eventID || message.FromVoidID != senderID || message.Payload != encoded || !message.CreatedAt.Equal(result.CreatedAt) || !message.ExpiresAt.Equal(result.ExpiresAt) {
		return message, result, errors.New("recipient envelope or opaque ciphertext changed in transit")
	}
	if err := acknowledge(ctx, recipient, message); err != nil {
		return message, result, err
	}
	return message, result, nil
}

func acknowledge(ctx context.Context, recipient *websocket.Conn, message relayEnvelope) error {
	if err := writeFrame(ctx, recipient, map[string]string{"type": "ack", "sender_void_id": message.FromVoidID, "event_id": message.EventID}); err != nil {
		return err
	}
	var result relayEnvelope
	if err := readFrame(ctx, recipient, &result); err != nil {
		return err
	}
	if result.Type != "ack_ok" || result.EventID != message.EventID || result.SenderVoidID != message.FromVoidID {
		return errors.New("recipient acknowledgment was not confirmed")
	}
	return nil
}

func writeFrame(ctx context.Context, conn *websocket.Conn, value any) error {
	data, err := json.Marshal(value)
	if err != nil {
		return err
	}
	return conn.Write(ctx, websocket.MessageText, data)
}

func readFrame(ctx context.Context, conn *websocket.Conn, result any) error {
	kind, data, err := conn.Read(ctx)
	if err != nil {
		return err
	}
	if kind != websocket.MessageText {
		return errors.New("expected a JSON text frame")
	}
	return json.Unmarshal(data, result)
}

func waitActive(ctx context.Context, server *relay.Server, want int) error {
	ticker := time.NewTicker(time.Millisecond)
	defer ticker.Stop()
	for {
		if server.ActiveCount() == want {
			return nil
		}
		select {
		case <-ctx.Done():
			return fmt.Errorf("wait for %d active connections: %w", want, ctx.Err())
		case <-ticker.C:
		}
	}
}

// Only fixed stage labels, account IDs and simulated application envelopes are
// printed. Authentication challenges and proofs never reach the output writer.
type demoLog struct {
	out io.Writer
	err error
}

func (l *demoLog) stage(message string) {
	if l.err == nil {
		_, l.err = fmt.Fprintln(l.out, message)
	}
}

func (l *demoLog) envelope(label string, envelope any) {
	if l.err != nil {
		return
	}
	data, err := json.Marshal(envelope)
	if err != nil {
		l.err = err
		return
	}
	_, l.err = fmt.Fprintf(l.out, "%s: %s\n", label, data)
}

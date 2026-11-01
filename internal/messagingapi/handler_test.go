package messagingapi

import (
	"bytes"
	"context"
	"crypto/ed25519"
	"crypto/rand"
	"crypto/sha256"
	"encoding/base64"
	"encoding/json"
	"fmt"
	"net/http"
	"net/http/httptest"
	"strconv"
	"strings"
	"sync"
	"testing"
	"time"

	"void-nothingness/internal/auth"
	"void-nothingness/internal/messaging"
	"void-nothingness/internal/session"
)

const audience = "https://api.voidchats.xyz/dm9pZG5vdGhpbmduZXNz"

type testLookup map[int64]ed25519.PublicKey

func (l testLookup) AuthenticationKey(_ context.Context, id int64) ([]byte, error) {
	if key, ok := l[id]; ok {
		return key, nil
	}
	return nil, auth.ErrNotFound
}

type testSessions struct {
	mu      sync.Mutex
	records map[[32]byte]session.Record
}

func (s *testSessions) Persist(_ context.Context, r session.Record, max int) error {
	s.mu.Lock()
	defer s.mu.Unlock()
	count := 0
	for hash, row := range s.records {
		if !time.Now().Before(row.ExpiresAt) {
			delete(s.records, hash)
		} else if row.VoidID == r.VoidID {
			count++
		}
	}
	if count >= max {
		return session.ErrCapacity
	}
	s.records[r.TokenHash] = r
	return nil
}
func (s *testSessions) Find(_ context.Context, hash [32]byte) (session.Record, error) {
	s.mu.Lock()
	defer s.mu.Unlock()
	r, ok := s.records[hash]
	if !ok {
		return r, session.ErrAuthentication
	}
	return r, nil
}
func (s *testSessions) Revoke(_ context.Context, hash [32]byte) error {
	s.mu.Lock()
	defer s.mu.Unlock()
	delete(s.records, hash)
	return nil
}

type fixture struct {
	t        *testing.T
	h        http.Handler
	lookup   testLookup
	private  map[string]ed25519.PrivateKey
	store    *messaging.MemoryStore
	sessions *testSessions
}

func newFixture(t *testing.T) *fixture {
	t.Helper()
	f := &fixture{t: t, lookup: make(testLookup), private: make(map[string]ed25519.PrivateKey), sessions: &testSessions{records: make(map[[32]byte]session.Record)}}
	people := []messaging.Person{}
	for i := int64(1); i <= 3; i++ {
		pub, priv, err := ed25519.GenerateKey(rand.Reader)
		if err != nil {
			t.Fatal(err)
		}
		id := strconv.FormatInt(i, 10)
		f.lookup[i] = pub
		f.private[id] = priv
		people = append(people, messaging.Person{VoidID: id, Username: "person" + id, AuthPublicKey: base64.StdEncoding.EncodeToString(pub)})
	}
	f.store = messaging.NewMemoryStore(people)
	f.restart()
	return f
}
func (f *fixture) restart() {
	f.t.Helper()
	sessions, err := session.New(f.lookup, f.sessions, audience)
	if err != nil {
		f.t.Fatal(err)
	}
	service, err := messaging.NewService(f.store)
	if err != nil {
		f.t.Fatal(err)
	}
	f.h = New(sessions, service, Options{RatePerMinute: 10000, RateBurst: 1000, AuthRatePerMinute: 10000, AuthBurst: 1000})
}
func (f *fixture) raw(method, path, body, token string) *httptest.ResponseRecorder {
	r := httptest.NewRequest(method, Prefix+path, strings.NewReader(body))
	r.RemoteAddr = "127.0.0.1:1234"
	r.Header.Set("Content-Type", "application/json")
	if token != "" {
		r.Header.Set("Authorization", "Bearer "+token)
	}
	w := httptest.NewRecorder()
	f.h.ServeHTTP(w, r)
	return w
}
func (f *fixture) call(method, path string, body any, token string, want int, out any) *httptest.ResponseRecorder {
	f.t.Helper()
	encoded := ""
	if body != nil {
		data, err := json.Marshal(body)
		if err != nil {
			f.t.Fatal(err)
		}
		encoded = string(data)
	}
	w := f.raw(method, path, encoded, token)
	if w.Code != want {
		f.t.Fatalf("%s %s: status %d want %d: %s", method, path, w.Code, want, w.Body.String())
	}
	if out != nil {
		if err := json.Unmarshal(w.Body.Bytes(), out); err != nil {
			f.t.Fatal(err)
		}
	}
	return w
}
func (f *fixture) login(id string) string {
	f.t.Helper()
	var challenge session.Challenge
	f.call("POST", "/auth/challenges", map[string]string{"void_id": id}, "", 201, &challenge)
	message, err := base64.StdEncoding.DecodeString(challenge.Message)
	if err != nil {
		f.t.Fatal(err)
	}
	fields := strings.Split(string(message), "\n")
	parsed, _ := strconv.ParseInt(id, 10, 64)
	if len(fields) != 8 || fields[0] != "VOID-NOTHINGNESS:auth:v1" || fields[1] != challenge.ChallengeID || fields[2] != challenge.ExpiresAt.UTC().Format(time.RFC3339Nano) || fields[3] != id || fields[4] != base64.StdEncoding.EncodeToString(f.lookup[parsed]) || fields[5] != base64.StdEncoding.EncodeToString([]byte(audience)) || fields[7] != "" {
		f.t.Fatal("authentication bindings mismatch")
	}
	var result session.Token
	f.call("POST", "/auth/sessions", map[string]string{"challenge_id": challenge.ChallengeID, "signature": base64.StdEncoding.EncodeToString(ed25519.Sign(f.private[id], message))}, "", 201, &result)
	if result.TokenType != "Bearer" || len(result.AccessToken) != 43 {
		f.t.Fatal("invalid bearer response")
	}
	return result.AccessToken
}
func id(n int) string         { return fmt.Sprintf("00000000-0000-4000-8000-%012d", n) }
func encoded(s string) string { return base64.StdEncoding.EncodeToString([]byte(s)) }
func (f *fixture) invite(alice, bob string, n int) messaging.Request {
	f.t.Helper()
	f.call("POST", "/key-packages", map[string]any{"packages": []string{encoded(fmt.Sprintf("synthetic public MLS package %d", n))}}, bob, 201, nil)
	var claim messaging.Claim
	f.call("POST", "/people/2/key-packages/claim", map[string]string{"client_claim_id": id(n)}, alice, 200, &claim)
	var request messaging.Request
	f.call("POST", "/requests", messaging.CreateRequestInput{ClientRequestID: id(n + 1), ToVoidID: "2", KeyPackageID: claim.KeyPackageID, GroupIDBase64: encoded("group" + strconv.Itoa(n)), WelcomeBase64: encoded("synthetic opaque Welcome")}, alice, 201, &request)
	return request
}

func TestTwoIdentitiesDurableHTTPFlow(t *testing.T) {
	f := newFixture(t)
	alice, bob, outsider := f.login("1"), f.login("2"), f.login("3")
	var person messaging.Person
	f.call("GET", "/people/2", nil, alice, 200, &person)
	if person.VoidID != "2" || person.AuthPublicKey != base64.StdEncoding.EncodeToString(f.lookup[2]) {
		t.Fatal("lookup returned wrong identity")
	}
	request := f.invite(alice, bob, 10)
	var requests messaging.RequestPage
	f.call("GET", "/requests", nil, bob, 200, &requests)
	if len(requests.Requests) != 1 || requests.Requests[0].RequestID != request.RequestID {
		t.Fatal("recipient request missing")
	}
	wrongRecipient := f.raw("POST", "/requests/"+request.RequestID+"/accept", `{}`, alice)
	if wrongRecipient.Code != 403 && wrongRecipient.Code != 404 {
		t.Fatalf("sender accepted their own invitation: %d", wrongRecipient.Code)
	}
	pending := f.raw("POST", "/conversations/"+request.ConversationID+"/messages", `{"client_message_id":"`+id(12)+`","kind":"application","ciphertext_base64":"`+encoded("opaque")+`"}`, alice)
	if pending.Code != 409 && pending.Code != 404 {
		t.Fatalf("pending conversation accepted send: %d", pending.Code)
	}
	var accepted messaging.Request
	f.call("POST", "/requests/"+request.RequestID+"/accept", struct{}{}, bob, 200, &accepted)
	f.call("POST", "/requests/"+request.RequestID+"/accept", struct{}{}, bob, 200, nil)
	if accepted.Status != "accepted" {
		t.Fatal("accept state missing")
	}
	input := messaging.SendInput{ClientMessageID: id(13), Kind: "application", CiphertextBase64: encoded("synthetic encrypted wire bytes")}
	var sent, retried messaging.Message
	f.call("POST", "/conversations/"+request.ConversationID+"/messages", input, alice, 201, &sent)
	f.call("POST", "/conversations/"+request.ConversationID+"/messages", input, alice, 201, &retried)
	if sent.MessageID != retried.MessageID || sent.Sequence != retried.Sequence {
		t.Fatal("retry created duplicate message")
	}
	input.CiphertextBase64 = encoded("different bytes")
	f.call("POST", "/conversations/"+request.ConversationID+"/messages", input, alice, 409, nil)
	unauthorized := f.raw("POST", "/conversations/"+request.ConversationID+"/messages", `{"client_message_id":"`+id(14)+`","kind":"application","ciphertext_base64":"eA=="}`, outsider)
	if unauthorized.Code != 403 && unauthorized.Code != 404 {
		t.Fatalf("outsider sent message: %d", unauthorized.Code)
	}
	// Reconstruct all services and the handler over the retained stores. This is
	// a synthetic persistence boundary, not a claim of a live PostgreSQL restart.
	f.restart()
	var page messaging.SyncPage
	f.call("GET", "/sync?after=0&limit=50", nil, bob, 200, &page)
	last := int64(0)
	messageEvent := ""
	acceptCursor := int64(0)
	for _, event := range page.Events {
		cursor, err := strconv.ParseInt(event.Cursor, 10, 64)
		if err != nil || cursor <= last {
			t.Fatal("unordered account cursors")
		}
		last = cursor
		if event.Type == "request.accepted" {
			acceptCursor = cursor
		}
		if event.Type == "message.created" {
			var message messaging.Message
			if err := json.Unmarshal(event.Payload, &message); err != nil {
				t.Fatal(err)
			}
			if message.MessageID != sent.MessageID || message.CiphertextBase64 != sent.CiphertextBase64 {
				t.Fatal("opaque message changed")
			}
			messageEvent = event.EventID
			if acceptCursor == 0 || acceptCursor >= cursor {
				t.Fatal("message preceded acceptance")
			}
		}
	}
	if messageEvent == "" {
		t.Fatalf("message absent after service reconstruction: %+v", page)
	}
	foreign := f.raw("POST", "/sync/ack", `{"events":[{"event_id":"`+messageEvent+`","outcome":"applied"}]}`, outsider)
	if foreign.Code != 403 && foreign.Code != 404 {
		t.Fatalf("outsider ACK succeeded: %d", foreign.Code)
	}
	acks := map[string]any{"events": []messaging.Acknowledgment{{EventID: messageEvent, Outcome: "applied"}}}
	f.call("POST", "/sync/ack", acks, bob, 204, nil)
	f.call("POST", "/sync/ack", acks, bob, 204, nil)
	f.call("POST", "/sync/ack", map[string]any{"events": []messaging.Acknowledgment{{EventID: messageEvent, Outcome: "rejected"}}}, bob, 409, nil)
	var receipts messaging.SyncPage
	f.call("GET", "/sync?after=0", nil, alice, 200, &receipts)
	found := false
	for _, event := range receipts.Events {
		if event.Type == "message.delivered" {
			found = true
		}
	}
	if !found {
		t.Fatalf("applied message produced no delivery receipt: %+v", receipts)
	}
	var commit, reply messaging.Message
	f.call("POST", "/conversations/"+request.ConversationID+"/messages", messaging.SendInput{ClientMessageID: id(15), Kind: "commit", CiphertextBase64: encoded("synthetic opaque commit")}, bob, 201, &commit)
	f.call("POST", "/conversations/"+request.ConversationID+"/messages", messaging.SendInput{ClientMessageID: id(16), Kind: "application", CiphertextBase64: encoded("synthetic encrypted reply")}, bob, 201, &reply)
	if commit.Sequence != "2" || reply.Sequence != "3" {
		t.Fatal("conversation sequence did not order both senders and commits")
	}
	var incoming messaging.SyncPage
	f.call("GET", "/sync?after=0", nil, alice, 200, &incoming)
	var delivered []messaging.Message
	for _, event := range incoming.Events {
		if event.Type == "message.created" {
			var message messaging.Message
			if err := json.Unmarshal(event.Payload, &message); err != nil {
				t.Fatal(err)
			}
			delivered = append(delivered, message)
		}
	}
	if len(delivered) != 2 || delivered[0].MessageID != commit.MessageID || delivered[1].MessageID != reply.MessageID || delivered[1].CiphertextBase64 != reply.CiphertextBase64 {
		t.Fatal("reverse messages or commit ordering changed")
	}
	f.call("DELETE", "/auth/session", nil, alice, 204, nil)
	f.call("GET", "/people/2", nil, alice, 401, nil)
	if _, err := f.sessions.Find(context.Background(), sha256.Sum256([]byte(alice))); err == nil {
		t.Fatal("revoked token remains")
	}
}

func TestDeclineAndBlockStopDelivery(t *testing.T) {
	f := newFixture(t)
	alice, bob := f.login("1"), f.login("2")
	request := f.invite(alice, bob, 100)
	f.call("POST", "/requests/"+request.RequestID+"/decline", struct{}{}, bob, 200, nil)
	f.call("POST", "/requests/"+request.RequestID+"/decline", struct{}{}, bob, 200, nil)
	f.call("POST", "/requests/"+request.RequestID+"/accept", struct{}{}, bob, 409, nil)
	request = f.invite(alice, bob, 110)
	f.call("POST", "/requests/"+request.RequestID+"/accept", struct{}{}, bob, 200, nil)
	f.call("POST", "/conversations/"+request.ConversationID+"/messages", messaging.SendInput{ClientMessageID: id(112), Kind: "application", CiphertextBase64: encoded("not fetched before block")}, alice, 201, nil)
	f.call("PUT", "/blocks/1", struct{}{}, bob, 204, nil)
	f.call("PUT", "/blocks/1", struct{}{}, bob, 204, nil)
	for _, pair := range []struct{ token, target string }{{alice, "2"}, {bob, "1"}} {
		f.call("GET", "/people/"+pair.target, nil, pair.token, 404, nil)
		f.call("POST", "/people/"+pair.target+"/key-packages/claim", map[string]string{"client_claim_id": id(113)}, pair.token, 404, nil)
		f.call("POST", "/conversations/"+request.ConversationID+"/messages", messaging.SendInput{ClientMessageID: id(114), Kind: "application", CiphertextBase64: encoded("blocked")}, pair.token, 404, nil)
		var page messaging.SyncPage
		f.call("GET", "/sync?after=0", nil, pair.token, 200, &page)
		for _, event := range page.Events {
			if event.Type == "message.created" || strings.HasPrefix(event.Type, "request.") {
				t.Fatal("blocked pair's pending payload was delivered")
			}
			if pair.token == alice && event.Type == "block.updated" {
				t.Fatal("block was revealed to blocked peer")
			}
		}
	}
}

func TestStrictJSONAuthorizationAndBounds(t *testing.T) {
	f := newFixture(t)
	token := f.login("1")
	for _, body := range []string{`{"void_id":"1","void_id":"2"}`, `{"void_id":"1","VOID_ID":"2"}`, `{"VOID_ID":"1"}`, `{"void_id":null}`, `{"void_id":"1","private_key":"x"}`, `{"void_id":1}`, `{}`, `{"void_id":"1"}{}`, `[]`} {
		if w := f.raw("POST", "/auth/challenges", body, ""); w.Code != 400 {
			t.Errorf("accepted invalid challenge body %s: %d", body, w.Code)
		}
	}
	for _, body := range []string{`{"events":[{"event_id":"` + id(1) + `","outcome":"applied","Outcome":"rejected"}]}`, `{"events":[{"event_id":"` + id(1) + `","outcome":"applied","outcome":"rejected"}]}`, `{"events":null}`, `{"events":[null]}`} {
		if w := f.raw("POST", "/sync/ack", body, token); w.Code != 400 {
			t.Errorf("accepted invalid nested body %s: %d", body, w.Code)
		}
	}
	for _, path := range []string{"/people/2", "/requests", "/conversations", "/sync"} {
		f.call("GET", path, nil, "", 401, nil)
	}
	for _, query := range []string{"?limit=0", "?limit=101", "?limit=01", "?limit=2&limit=3", "?sender_id=2", "?after=", "?%xx=y"} {
		if w := f.raw("GET", "/sync"+query, "", token); w.Code != 400 {
			t.Errorf("accepted invalid query %s: %d", query, w.Code)
		}
	}
	oversize := `{"packages":["` + strings.Repeat("A", 2<<20) + `"]}`
	if w := f.raw("POST", "/key-packages", oversize, token); w.Code != 413 {
		t.Fatalf("large body status %d", w.Code)
	}
	if w := f.raw("POST", "/auth/challenges", strings.Repeat(" ", 4097), ""); w.Code != 413 {
		t.Fatalf("large authentication body status %d", w.Code)
	}
	f.call("PUT", "/people/2", nil, token, 405, nil)
	f.call("GET", "/unknown", nil, token, 404, nil)
	for _, bad := range []string{"Basic " + token, "Bearer " + token + " ", "Bearer " + token + "," + token} {
		r := httptest.NewRequest("GET", Prefix+"/people/2", nil)
		r.Header.Set("Authorization", bad)
		w := httptest.NewRecorder()
		f.h.ServeHTTP(w, r)
		if w.Code != 401 {
			t.Errorf("accepted malformed authorization: %d", w.Code)
		}
	}
}

func TestAccountRatesSeparateFromProxyAndRegistration(t *testing.T) {
	f := newFixture(t)
	alice, bob := f.login("1"), f.login("2")
	sessions, _ := session.New(f.lookup, f.sessions, audience)
	service, _ := messaging.NewService(f.store)
	f.h = New(sessions, service, Options{RatePerMinute: 1, RateBurst: 1})
	f.call("GET", "/people/2", nil, alice, 200, nil)
	f.call("GET", "/people/2", nil, alice, 429, nil)
	f.call("GET", "/people/1", nil, bob, 200, nil)
	mux := http.NewServeMux()
	Mount(mux, f.h)
	mux.HandleFunc(Prefix+"/accounts", func(w http.ResponseWriter, r *http.Request) { w.WriteHeader(201) })
	r := httptest.NewRequest("POST", Prefix+"/accounts", bytes.NewReader([]byte(`{}`)))
	w := httptest.NewRecorder()
	mux.ServeHTTP(w, r)
	if w.Code != 201 {
		t.Fatal("messaging throttling affected registration")
	}
}

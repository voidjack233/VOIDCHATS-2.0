package httpapi

import (
	"bytes"
	"context"
	"encoding/base64"
	"encoding/json"
	"errors"
	"fmt"
	"net/http"
	"net/http/httptest"
	"strings"
	"sync"
	"sync/atomic"
	"testing"
	"time"

	"void-nothingness/internal/account"
)

type stubService struct {
	challenge func(string, []byte) (account.Challenge, error)
	register  func(context.Context, string, []byte) (account.Account, error)
}

func (s stubService) Challenge(username string, key []byte) (account.Challenge, error) {
	if s.challenge != nil {
		return s.challenge(username, key)
	}
	return account.Challenge{ChallengeID: "challenge", Message: "c2lnbiBtZQ==", ExpiresAt: time.Unix(100, 0).UTC()}, nil
}

func (s stubService) Register(ctx context.Context, challengeID string, signature []byte) (account.Account, error) {
	if s.register != nil {
		return s.register(ctx, challengeID, signature)
	}
	return account.Account{VoidID: "9223372036854775807", Username: "Alice", CreatedAt: time.Unix(100, 0).UTC()}, nil
}

func serve(h http.Handler, method, path, body string) *httptest.ResponseRecorder {
	r := httptest.NewRequest(method, path, strings.NewReader(body))
	r.Header.Set("Content-Type", "application/json")
	w := httptest.NewRecorder()
	h.ServeHTTP(w, r)
	return w
}

func challengeBody() string {
	return fmt.Sprintf(`{"username":"Alice","auth_public_key":%q}`, base64.StdEncoding.EncodeToString(bytes.Repeat([]byte{42}, 32)))
}

func registrationBody() string {
	return fmt.Sprintf(`{"challenge_id":"challenge","signature":%q}`, base64.StdEncoding.EncodeToString(bytes.Repeat([]byte{7}, 64)))
}

func TestChallengeAndRegistrationResponses(t *testing.T) {
	var challengeCalled, registrationCalled bool
	service := stubService{
		challenge: func(username string, key []byte) (account.Challenge, error) {
			challengeCalled = true
			if username != "Alice" || !bytes.Equal(key, bytes.Repeat([]byte{42}, 32)) {
				t.Fatal("challenge received incorrect decoded inputs")
			}
			return account.Challenge{ChallengeID: "challenge", Message: "c2lnbiBtZQ==", ExpiresAt: time.Unix(100, 0).UTC()}, nil
		},
		register: func(ctx context.Context, id string, signature []byte) (account.Account, error) {
			registrationCalled = true
			if id != "challenge" || !bytes.Equal(signature, bytes.Repeat([]byte{7}, 64)) {
				t.Fatal("registration received incorrect decoded inputs")
			}
			if _, ok := ctx.Deadline(); !ok {
				t.Fatal("registration has no deadline")
			}
			return account.Account{VoidID: "9223372036854775807", Username: "Alice", CreatedAt: time.Unix(100, 0).UTC()}, nil
		},
	}
	h := New(service, nil, Options{})
	w := serve(h, http.MethodPost, "/dm9pZG5vdGhpbmduZXNz/challenges", challengeBody())
	if w.Code != http.StatusCreated || !challengeCalled {
		t.Fatalf("challenge: status %d body %s", w.Code, w.Body.String())
	}
	var challenge account.Challenge
	if err := json.Unmarshal(w.Body.Bytes(), &challenge); err != nil || challenge.Message != "c2lnbiBtZQ==" || challenge.ChallengeID != "challenge" {
		t.Fatalf("invalid challenge response: %s", w.Body.String())
	}
	w = serve(h, http.MethodPost, "/dm9pZG5vdGhpbmduZXNz/accounts", registrationBody())
	if w.Code != http.StatusCreated || !registrationCalled {
		t.Fatalf("registration: status %d body %s", w.Code, w.Body.String())
	}
	var result map[string]any
	if err := json.Unmarshal(w.Body.Bytes(), &result); err != nil || result["void_id"] != "9223372036854775807" || len(result) != 3 {
		t.Fatalf("account should expose only public identity fields: %s", w.Body.String())
	}
	if w.Header().Get("Set-Cookie") != "" || w.Header().Get("Authorization") != "" {
		t.Fatal("registration must not create an authenticated session")
	}
}

func TestStrictJSONRejectsAmbiguousOrUnexpectedInput(t *testing.T) {
	key := base64.StdEncoding.EncodeToString(bytes.Repeat([]byte{42}, 32))
	tests := []struct {
		name, body string
	}{
		{"empty", ""},
		{"null object", "null"},
		{"array", "[]"},
		{"unknown field", fmt.Sprintf(`{"username":"Alice","auth_public_key":%q,"private_key":"secret"}`, key)},
		{"duplicate field", fmt.Sprintf(`{"username":"Alice","username":"Bob","auth_public_key":%q}`, key)},
		{"escaped duplicate", fmt.Sprintf(`{"username":"Alice","\u0075sername":"Bob","auth_public_key":%q}`, key)},
		{"missing field", `{"username":"Alice"}`},
		{"null field", fmt.Sprintf(`{"username":null,"auth_public_key":%q}`, key)},
		{"numeric field", fmt.Sprintf(`{"username":123,"auth_public_key":%q}`, key)},
		{"nested field", fmt.Sprintf(`{"username":{},"auth_public_key":%q}`, key)},
		{"trailing object", challengeBody() + `{}`},
		{"trailing garbage", challengeBody() + `garbage`},
		{"truncated object", `{"username":"Alice"`},
		{"invalid UTF8", `{"username":"` + string([]byte{0xff}) + `","auth_public_key":"AA=="}`},
		{"case variant field", fmt.Sprintf(`{"Username":"Alice","auth_public_key":%q}`, key)},
	}
	for _, tt := range tests {
		t.Run(tt.name, func(t *testing.T) {
			h := New(stubService{challenge: func(string, []byte) (account.Challenge, error) {
				t.Fatal("invalid request reached service")
				return account.Challenge{}, nil
			}}, nil, Options{})
			w := serve(h, http.MethodPost, "/dm9pZG5vdGhpbmduZXNz/challenges", tt.body)
			if w.Code != http.StatusBadRequest {
				t.Fatalf("status %d, expected 400; body %s", w.Code, w.Body.String())
			}
		})
	}
	for _, body := range []string{`null`, `{"challenge_id":"x","signature":null}`, registrationBody() + `{}`, `{"challenge_id":"x","challenge_id":"y","signature":"AA=="}`} {
		w := serve(New(stubService{}, nil, Options{}), http.MethodPost, "/dm9pZG5vdGhpbmduZXNz/accounts", body)
		if w.Code != http.StatusBadRequest {
			t.Fatalf("registration accepted ambiguous JSON %q: %d", body, w.Code)
		}
	}
}

func TestBase64MustBeCanonicalStandardEncoding(t *testing.T) {
	for _, value := range []string{"not-base64", "AQ", "AQ_+", "AR==", "AQ==\n", " AQ=="} {
		t.Run(fmt.Sprintf("%q", value), func(t *testing.T) {
			for _, endpoint := range []string{"/dm9pZG5vdGhpbmduZXNz/challenges", "/dm9pZG5vdGhpbmduZXNz/accounts"} {
				body := fmt.Sprintf(`{"username":"Alice","auth_public_key":%q}`, value)
				if endpoint == "/dm9pZG5vdGhpbmduZXNz/accounts" {
					body = fmt.Sprintf(`{"challenge_id":"challenge","signature":%q}`, value)
				}
				w := serve(New(stubService{}, nil, Options{}), http.MethodPost, endpoint, body)
				if w.Code != http.StatusBadRequest {
					t.Fatalf("%s accepted %q: %d", endpoint, value, w.Code)
				}
			}
		})
	}
}

func TestBoundedBodiesAndContentType(t *testing.T) {
	h := New(stubService{}, nil, Options{})
	for _, endpoint := range []string{"/dm9pZG5vdGhpbmduZXNz/challenges", "/dm9pZG5vdGhpbmduZXNz/accounts"} {
		w := serve(h, http.MethodPost, endpoint, strings.Repeat(" ", maxBodyBytes+1))
		if w.Code != http.StatusRequestEntityTooLarge {
			t.Fatalf("oversized %s body: %d", endpoint, w.Code)
		}
	}
	for _, contentType := range []string{"", "text/plain", "application/json; broken"} {
		r := httptest.NewRequest(http.MethodPost, "/dm9pZG5vdGhpbmduZXNz/challenges", strings.NewReader(challengeBody()))
		r.Header.Set("Content-Type", contentType)
		w := httptest.NewRecorder()
		h.ServeHTTP(w, r)
		if w.Code != http.StatusUnsupportedMediaType {
			t.Fatalf("Content-Type %q status %d", contentType, w.Code)
		}
	}
	r := httptest.NewRequest(http.MethodPost, "/dm9pZG5vdGhpbmduZXNz/challenges", strings.NewReader(challengeBody()))
	r.Header.Set("Content-Type", "application/json; charset=utf-8")
	w := httptest.NewRecorder()
	h.ServeHTTP(w, r)
	if w.Code != http.StatusCreated {
		t.Fatalf("JSON with charset rejected: %d", w.Code)
	}
}

func TestRoutingMethodsAndSecurityHeaders(t *testing.T) {
	h := New(stubService{}, func(context.Context) error { return nil }, Options{})
	for _, tt := range []struct {
		method, path string
		status       int
		allow        string
	}{
		{http.MethodGet, "/healthz", 200, ""},
		{http.MethodGet, "/readyz", 200, ""},
		{http.MethodPost, "/healthz", 405, "GET"},
		{http.MethodHead, "/healthz", 405, "GET"},
		{http.MethodGet, "/dm9pZG5vdGhpbmduZXNz/challenges", 405, "POST"},
		{http.MethodDelete, "/dm9pZG5vdGhpbmduZXNz/accounts", 405, "POST"},
		{http.MethodGet, "/unknown", 404, ""},
		{http.MethodGet, "/healthz/", 404, ""},
	} {
		w := serve(h, tt.method, tt.path, "")
		if w.Code != tt.status || w.Header().Get("Allow") != tt.allow {
			t.Fatalf("%s %s: status %d Allow %q", tt.method, tt.path, w.Code, w.Header().Get("Allow"))
		}
		if w.Header().Get("Content-Type") != "application/json" || w.Header().Get("Cache-Control") != "no-store" || w.Header().Get("X-Content-Type-Options") != "nosniff" {
			t.Fatalf("missing response headers for %s %s: %v", tt.method, tt.path, w.Header())
		}
	}
}

func TestObsoleteRoutesAreNotExposed(t *testing.T) {
	service := stubService{
		challenge: func(string, []byte) (account.Challenge, error) {
			t.Fatal("obsolete challenge route reached service")
			return account.Challenge{}, nil
		},
		register: func(context.Context, string, []byte) (account.Account, error) {
			t.Fatal("obsolete registration route reached service")
			return account.Account{}, nil
		},
	}
	h := New(service, nil, Options{})
	for _, route := range []struct {
		path, body string
	}{
		{"/v1/registration/challenges", challengeBody()},
		{"/v1/accounts", registrationBody()},
		{"/registration/challenges", challengeBody()},
		{"/accounts", registrationBody()},
	} {
		t.Run(route.path, func(t *testing.T) {
			w := serve(h, http.MethodPost, route.path, route.body)
			if w.Code != http.StatusNotFound || w.Header().Get("Allow") != "" {
				t.Fatalf("obsolete route returned status %d Allow %q", w.Code, w.Header().Get("Allow"))
			}
		})
	}
}

func TestSafeErrorMapping(t *testing.T) {
	const secret = "postgres password=private-secret signed-private-key"
	for _, tt := range []struct {
		err    error
		status int
		code   string
	}{
		{account.ErrInvalidInput, 400, "invalid_input"},
		{account.ErrInvalidChallenge, 401, "invalid_proof"},
		{account.ErrInvalidSignature, 401, "invalid_proof"},
		{account.ErrDuplicateKey, 409, "key_registered"},
		{account.ErrCapacity, 429, "capacity_reached"},
		{account.ErrUnavailable, 503, "unavailable"},
		{account.ErrIDCollision, 503, "unavailable"},
		{context.DeadlineExceeded, 504, "request_timeout"},
		{context.Canceled, 408, "request_canceled"},
		{errors.New(secret), 500, "internal_error"},
	} {
		t.Run(tt.code+tt.err.Error(), func(t *testing.T) {
			service := stubService{register: func(context.Context, string, []byte) (account.Account, error) {
				return account.Account{}, fmt.Errorf("%s: %w", secret, tt.err)
			}}
			w := serve(New(service, nil, Options{}), http.MethodPost, "/dm9pZG5vdGhpbmduZXNz/accounts", registrationBody())
			if w.Code != tt.status || !strings.Contains(w.Body.String(), `"code":"`+tt.code+`"`) {
				t.Fatalf("status %d body %s", w.Code, w.Body.String())
			}
			if strings.Contains(w.Body.String(), secret) || strings.Contains(w.Body.String(), "password") {
				t.Fatalf("sensitive error leaked: %s", w.Body.String())
			}
			if tt.status == http.StatusTooManyRequests && w.Header().Get("Retry-After") == "" {
				t.Fatal("missing Retry-After")
			}
		})
	}
}

func TestReadinessAndRequestDeadlines(t *testing.T) {
	const secret = "connection-string-with-secret"
	for _, ready := range []func(context.Context) error{
		nil,
		func(context.Context) error { return errors.New(secret) },
		func(ctx context.Context) error {
			<-ctx.Done()
			return ctx.Err()
		},
	} {
		h := New(stubService{}, ready, Options{RequestTimeout: 10 * time.Millisecond})
		w := serve(h, http.MethodGet, "/readyz", "")
		if w.Code != http.StatusServiceUnavailable || strings.Contains(w.Body.String(), secret) {
			t.Fatalf("readiness status %d body %s", w.Code, w.Body.String())
		}
		if w := serve(h, http.MethodGet, "/healthz", ""); w.Code != http.StatusOK {
			t.Fatalf("liveness depends on database readiness: %d", w.Code)
		}
	}
	service := stubService{register: func(ctx context.Context, _ string, _ []byte) (account.Account, error) {
		<-ctx.Done()
		return account.Account{}, ctx.Err()
	}}
	h := New(service, nil, Options{RequestTimeout: 10 * time.Millisecond})
	w := serve(h, http.MethodPost, "/dm9pZG5vdGhpbmduZXNz/accounts", registrationBody())
	if w.Code != http.StatusGatewayTimeout {
		t.Fatalf("request timeout status %d body %s", w.Code, w.Body.String())
	}
}

func TestCanceledRequestDoesNotReachService(t *testing.T) {
	service := stubService{register: func(context.Context, string, []byte) (account.Account, error) {
		t.Fatal("canceled request reached service")
		return account.Account{}, nil
	}}
	ctx, cancel := context.WithCancel(context.Background())
	cancel()
	r := httptest.NewRequest(http.MethodPost, "/dm9pZG5vdGhpbmduZXNz/accounts", strings.NewReader(registrationBody())).WithContext(ctx)
	w := httptest.NewRecorder()
	New(service, nil, Options{}).ServeHTTP(w, r)
	if w.Code != http.StatusRequestTimeout {
		t.Fatalf("canceled request status %d", w.Code)
	}
}

type interruptedBody struct {
	*strings.Reader
	interrupt func()
}

func (b *interruptedBody) Read(p []byte) (int, error) {
	b.interrupt()
	return b.Reader.Read(p)
}

func (b *interruptedBody) Close() error { return nil }

func TestInterruptedBodyDoesNotCreateChallengeOrRegister(t *testing.T) {
	for _, timedOut := range []bool{false, true} {
		for _, endpoint := range []string{"/dm9pZG5vdGhpbmduZXNz/challenges", "/dm9pZG5vdGhpbmduZXNz/accounts"} {
			t.Run(fmt.Sprintf("%s/timeout=%v", endpoint, timedOut), func(t *testing.T) {
				service := stubService{
					challenge: func(string, []byte) (account.Challenge, error) {
						t.Fatal("interrupted body created a challenge")
						return account.Challenge{}, nil
					},
					register: func(context.Context, string, []byte) (account.Account, error) {
						t.Fatal("interrupted body reached registration")
						return account.Account{}, nil
					},
				}
				ctx, cancel := context.WithCancel(context.Background())
				if timedOut {
					cancel()
					ctx, cancel = context.WithTimeout(context.Background(), time.Millisecond)
				}
				defer cancel()
				body := challengeBody()
				if endpoint == "/dm9pZG5vdGhpbmduZXNz/accounts" {
					body = registrationBody()
				}
				r := httptest.NewRequest(http.MethodPost, endpoint, nil).WithContext(ctx)
				r.Header.Set("Content-Type", "application/json")
				r.Body = &interruptedBody{Reader: strings.NewReader(body), interrupt: func() {
					if timedOut {
						<-ctx.Done()
					} else {
						cancel()
					}
				}}
				w := httptest.NewRecorder()
				New(service, nil, Options{}).ServeHTTP(w, r)
				want := http.StatusRequestTimeout
				if timedOut {
					want = http.StatusGatewayTimeout
				}
				if w.Code != want {
					t.Fatalf("status %d, expected %d; body %s", w.Code, want, w.Body.String())
				}
			})
		}
	}
}

func TestRateLimitIgnoresForwardingHeadersAndKeepsLivenessAvailable(t *testing.T) {
	h := New(stubService{}, nil, Options{RatePerMinute: 1, RateBurst: 2})
	for i := 0; i < 3; i++ {
		r := httptest.NewRequest(http.MethodPost, "/dm9pZG5vdGhpbmduZXNz/challenges", strings.NewReader(challengeBody()))
		r.RemoteAddr = fmt.Sprintf("192.0.2.1:%d", 1000+i)
		r.Header.Set("Content-Type", "application/json")
		r.Header.Set("X-Forwarded-For", fmt.Sprintf("198.51.100.%d", i))
		r.Header.Set("X-Real-IP", fmt.Sprintf("203.0.113.%d", i))
		w := httptest.NewRecorder()
		h.ServeHTTP(w, r)
		if i < 2 && w.Code != http.StatusCreated {
			t.Fatalf("initial burst rejected: %d", w.Code)
		}
		if i == 2 && (w.Code != http.StatusTooManyRequests || w.Header().Get("Retry-After") == "") {
			t.Fatalf("forwarding header bypassed rate limit: %d", w.Code)
		}
	}
	if w := serve(h, http.MethodGet, "/healthz", ""); w.Code != http.StatusOK {
		t.Fatalf("rate-limited liveness: %d", w.Code)
	}
}

func TestConcurrencyAdmissionDoesNotQueueAndProtectsReadiness(t *testing.T) {
	started, release := make(chan struct{}), make(chan struct{})
	service := stubService{register: func(context.Context, string, []byte) (account.Account, error) {
		close(started)
		<-release
		return account.Account{VoidID: "1"}, nil
	}}
	h := New(service, func(context.Context) error {
		t.Error("readiness work entered exhausted concurrency budget")
		return nil
	}, Options{MaxConcurrentRequests: 1})
	completed := make(chan *httptest.ResponseRecorder, 1)
	go func() { completed <- serve(h, http.MethodPost, "/dm9pZG5vdGhpbmduZXNz/accounts", registrationBody()) }()
	select {
	case <-started:
	case <-time.After(time.Second):
		t.Fatal("first request did not start")
	}
	for _, path := range []string{"/dm9pZG5vdGhpbmduZXNz/accounts", "/readyz"} {
		method, body := http.MethodPost, registrationBody()
		if path == "/readyz" {
			method, body = http.MethodGet, ""
		}
		if w := serve(h, method, path, body); w.Code != http.StatusServiceUnavailable {
			t.Fatalf("concurrency limit did not reject %s: %d", path, w.Code)
		}
	}
	if w := serve(h, http.MethodGet, "/healthz", ""); w.Code != http.StatusOK {
		t.Fatalf("liveness unavailable during busy registration: %d", w.Code)
	}
	close(release)
	if w := <-completed; w.Code != http.StatusCreated {
		t.Fatalf("admitted request failed: %d", w.Code)
	}
}

func TestIPLimiterBoundsClientsExpiresIdleEntriesAndRefills(t *testing.T) {
	now := time.Unix(1000, 0)
	l := newIPLimiter(1, 1, 2)
	l.now = func() time.Time { return now }
	if ok, _ := l.allow("192.0.2.1:100"); !ok {
		t.Fatal("first token unavailable")
	}
	if ok, retry := l.allow("[::ffff:192.0.2.1]:200"); ok || retry != 60 {
		t.Fatalf("mapped IPv4 bypassed limit or wrong retry: %v %d", ok, retry)
	}
	now = now.Add(59 * time.Second)
	if ok, retry := l.allow("192.0.2.1:300"); ok || retry != 1 {
		t.Fatalf("refilled too early or wrong retry: %v %d", ok, retry)
	}
	now = now.Add(time.Second)
	if ok, _ := l.allow("192.0.2.1:300"); !ok {
		t.Fatal("token did not refill")
	}
	if ok, _ := l.allow("192.0.2.2:100"); !ok {
		t.Fatal("second client rejected")
	}
	for i := 3; i < 200; i++ {
		if ok, _ := l.allow(fmt.Sprintf("192.0.2.%d:100", i)); ok {
			t.Fatal("client map exceeded capacity")
		}
	}
	if len(l.clients) != 2 {
		t.Fatalf("client map size %d", len(l.clients))
	}
	now = now.Add(11 * time.Minute)
	if ok, _ := l.allow("192.0.2.3:100"); !ok || len(l.clients) != 1 {
		t.Fatalf("idle clients not evicted: ok=%v clients=%d", ok, len(l.clients))
	}
}

func TestIPLimiterConcurrentRequestsRespectBurst(t *testing.T) {
	l := newIPLimiter(1, 7, 10)
	fixed := time.Unix(1000, 0)
	l.now = func() time.Time { return fixed }
	var accepted atomic.Int64
	var wg sync.WaitGroup
	for i := 0; i < 100; i++ {
		wg.Add(1)
		go func(i int) {
			defer wg.Done()
			if allowed, _ := l.allow(fmt.Sprintf("192.0.2.1:%d", 1000+i)); allowed {
				accepted.Add(1)
			}
		}(i)
	}
	wg.Wait()
	if accepted.Load() != 7 {
		t.Fatalf("accepted %d concurrent requests, expected burst 7", accepted.Load())
	}
}

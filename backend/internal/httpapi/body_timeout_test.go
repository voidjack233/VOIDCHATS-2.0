package httpapi

import (
	"bufio"
	"context"
	"fmt"
	"io"
	"net"
	"net/http"
	"net/http/httptest"
	"os"
	"strings"
	"sync"
	"sync/atomic"
	"testing"
	"time"

	"void-nothingness/internal/account"
)

type observedBody struct {
	io.ReadCloser
	started chan struct{}
	once    sync.Once
}

func (b *observedBody) Read(p []byte) (int, error) {
	b.once.Do(func() { close(b.started) })
	return b.ReadCloser.Read(p)
}

func TestStalledBodyReleasesAdmissionAtRequestDeadline(t *testing.T) {
	for _, endpoint := range []string{"/v1/registration/challenges", "/v1/accounts"} {
		t.Run(endpoint, func(t *testing.T) {
			var serviceCalls atomic.Int64
			service := stubService{
				challenge: func(string, []byte) (account.Challenge, error) {
					serviceCalls.Add(1)
					return account.Challenge{}, account.ErrUnavailable
				},
				register: func(context.Context, string, []byte) (account.Account, error) {
					serviceCalls.Add(1)
					return account.Account{}, account.ErrUnavailable
				},
			}
			started := make(chan struct{})
			h := New(service, func(context.Context) error { return nil }, Options{
				RequestTimeout: 150 * time.Millisecond, MaxConcurrentRequests: 1,
			})
			server := httptest.NewUnstartedServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
				if r.URL.Path == endpoint {
					r.Body = &observedBody{ReadCloser: r.Body, started: started}
				}
				h.ServeHTTP(w, r)
			}))
			// Longer connection deadlines expose a handler that only cancels its
			// context while the underlying body read remains blocked.
			server.Config.ReadTimeout = 2 * time.Second
			server.Config.WriteTimeout = 2 * time.Second
			server.Start()
			defer server.Close()
			client := server.Client()
			client.Timeout = time.Second
			conn, err := net.DialTimeout("tcp", server.Listener.Addr().String(), time.Second)
			if err != nil {
				t.Fatal(err)
			}
			defer conn.Close()
			if err := conn.SetDeadline(time.Now().Add(time.Second)); err != nil {
				t.Fatal(err)
			}
			before := time.Now()
			if _, err := fmt.Fprintf(conn, "POST %s HTTP/1.1\r\nHost: localhost\r\nContent-Type: application/json\r\nContent-Length: 100\r\nConnection: close\r\n\r\n{", endpoint); err != nil {
				t.Fatal(err)
			}
			select {
			case <-started:
			case <-time.After(time.Second):
				t.Fatal("server did not start reading the partial body")
			}
			busy, err := client.Get(server.URL + "/readyz")
			if err != nil {
				t.Fatal(err)
			}
			busy.Body.Close()
			if busy.StatusCode != http.StatusServiceUnavailable {
				t.Fatalf("stalled body did not occupy admission slot: %d", busy.StatusCode)
			}
			response, err := http.ReadResponse(bufio.NewReader(conn), nil)
			if err != nil {
				t.Fatalf("stalled body did not return before the client deadline: %v", err)
			}
			body, err := io.ReadAll(response.Body)
			response.Body.Close()
			if err != nil {
				t.Fatal(err)
			}
			if response.StatusCode != http.StatusGatewayTimeout || string(body) != "{\"error\":{\"code\":\"request_timeout\",\"message\":\"Request timed out.\"}}\n" {
				t.Fatalf("unexpected timeout response: %d %s", response.StatusCode, body)
			}
			if elapsed := time.Since(before); elapsed >= time.Second {
				t.Fatalf("body read exceeded request timeout allowance: %s", elapsed)
			}
			ready, err := client.Get(server.URL + "/readyz")
			if err != nil {
				t.Fatal(err)
			}
			ready.Body.Close()
			if ready.StatusCode != http.StatusOK {
				t.Fatalf("timed-out body retained admission slot: %d", ready.StatusCode)
			}
			if serviceCalls.Load() != 0 {
				t.Fatalf("stalled request reached service %d times", serviceCalls.Load())
			}
		})
	}
}

type failedBody struct {
	err        error
	beforeRead func()
}

func (b failedBody) Read([]byte) (int, error) {
	if b.beforeRead != nil {
		b.beforeRead()
	}
	return 0, b.err
}

func (b failedBody) Close() error { return nil }

func TestBodyTimeoutBeforeContextTimerReturnsSafeTimeout(t *testing.T) {
	// Socket deadlines may report expiry before the context timer is scheduled.
	// Even then, the network error must map to the same safe timeout response.
	for _, cancelOnRead := range []bool{false, true} {
		t.Run(fmt.Sprintf("context_canceled=%v", cancelOnRead), func(t *testing.T) {
			ctx, cancel := context.WithCancel(context.Background())
			defer cancel()
			request := httptest.NewRequest(http.MethodPost, "/v1/accounts", nil).WithContext(ctx)
			request.Header.Set("Content-Type", "application/json")
			body := failedBody{err: fmt.Errorf("private connection diagnostic: %w", os.ErrDeadlineExceeded)}
			if cancelOnRead {
				// net/http cancels the request after a connection read error.
				body.beforeRead = cancel
			}
			request.Body = body
			response := httptest.NewRecorder()
			New(stubService{register: func(context.Context, string, []byte) (account.Account, error) {
				t.Fatal("timed-out body reached service")
				return account.Account{}, nil
			}}, nil, Options{}).ServeHTTP(response, request)
			if response.Code != http.StatusGatewayTimeout || !strings.Contains(response.Body.String(), `"code":"request_timeout"`) || strings.Contains(response.Body.String(), "private") {
				t.Fatalf("unsafe timeout response: %d %s", response.Code, response.Body.String())
			}
			if (ctx.Err() != nil) != cancelOnRead {
				t.Fatalf("incorrect test context state: %v", ctx.Err())
			}
		})
	}
}

package httptransport

import (
	"context"
	"errors"
	"net/http"
	"net/http/httptest"
	"testing"
	"time"

	"github.com/voidjack233/VOIDCHATS-2.0/backend/internal/account"
)

func TestHealth(t *testing.T) {
	server := NewServer("127.0.0.1:8080", account.NewService(account.NewMemoryRepository()), nil)
	request := httptest.NewRequest(http.MethodGet, "/health", nil)
	response := httptest.NewRecorder()
	server.Handler.ServeHTTP(response, request)

	if response.Code != http.StatusOK {
		t.Fatalf("status = %d, want %d", response.Code, http.StatusOK)
	}
	if got := response.Header().Get("Content-Type"); got != "application/json" {
		t.Errorf("Content-Type = %q, want application/json", got)
	}
	if got := response.Body.String(); got != `{"status":"ok"}` {
		t.Errorf("body = %q, want health JSON", got)
	}
}

func TestReadiness(t *testing.T) {
	for _, test := range []struct {
		name   string
		check  func(context.Context) error
		status int
		body   string
	}{
		{name: "healthy", check: func(ctx context.Context) error {
			deadline, ok := ctx.Deadline()
			if !ok || time.Until(deadline) <= 0 || time.Until(deadline) > 2*time.Second {
				t.Error("readiness check does not have a bounded deadline")
			}
			return nil
		}, status: http.StatusOK, body: "{\"status\":\"ready\"}\n"},
		{name: "unavailable", check: func(context.Context) error { return errors.New("private database details") }, status: http.StatusServiceUnavailable, body: "{\"status\":\"not_ready\"}\n"},
		{name: "unconfigured", status: http.StatusServiceUnavailable, body: "{\"status\":\"not_ready\"}\n"},
	} {
		t.Run(test.name, func(t *testing.T) {
			server := NewServer("", account.NewService(account.NewMemoryRepository()), test.check)
			response := performRequest(server.Handler, http.MethodGet, "/ready", "")
			if response.Code != test.status || response.Body.String() != test.body || response.Header().Get("Content-Type") != "application/json" {
				t.Fatalf("unexpected readiness response: %d %q", response.Code, response.Body.String())
			}
		})
	}
}

func TestReadinessRejectsOtherMethodsWithoutCheckingDatabase(t *testing.T) {
	server := NewServer("", account.NewService(account.NewMemoryRepository()), func(context.Context) error {
		t.Error("database check ran for an unsupported method")
		return nil
	})
	for _, method := range []string{http.MethodPost, http.MethodHead, http.MethodPut, http.MethodDelete} {
		response := performRequest(server.Handler, method, "/ready", "")
		assertJSONError(t, response, http.StatusMethodNotAllowed)
		if response.Header().Get("Allow") != http.MethodGet {
			t.Fatal("readiness method response missing Allow: GET")
		}
	}
}

func TestHealthRejectsOtherMethods(t *testing.T) {
	for _, method := range []string{http.MethodHead, http.MethodPost, http.MethodPut, http.MethodDelete, http.MethodOptions} {
		t.Run(method, func(t *testing.T) {
			response := httptest.NewRecorder()
			NewServer("", account.NewService(account.NewMemoryRepository()), nil).Handler.ServeHTTP(response, httptest.NewRequest(method, "/health", nil))
			assertJSONError(t, response, http.StatusMethodNotAllowed)
			if got := response.Header().Get("Allow"); got != http.MethodGet {
				t.Errorf("Allow = %q, want GET", got)
			}
		})
	}
}

func TestUnknownRoutes(t *testing.T) {
	for _, path := range []string{"/", "/health/", "/health/extra", "/accounts", "/v1/accounts/", "/v1/accounts/id/extra"} {
		t.Run(path, func(t *testing.T) {
			response := httptest.NewRecorder()
			NewServer("", account.NewService(account.NewMemoryRepository()), nil).Handler.ServeHTTP(response, httptest.NewRequest(http.MethodGet, path, nil))
			assertJSONError(t, response, http.StatusNotFound)
		})
	}
}

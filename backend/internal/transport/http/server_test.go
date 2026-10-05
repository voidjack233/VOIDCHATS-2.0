package httptransport

import (
	"net/http"
	"net/http/httptest"
	"testing"

	"github.com/voidjack233/VOIDCHATS-2.0/backend/internal/account"
)

func TestHealth(t *testing.T) {
	server := NewServer("127.0.0.1:8080", account.NewService(account.NewMemoryRepository()))
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

func TestHealthRejectsOtherMethods(t *testing.T) {
	for _, method := range []string{http.MethodHead, http.MethodPost, http.MethodPut, http.MethodDelete, http.MethodOptions} {
		t.Run(method, func(t *testing.T) {
			response := httptest.NewRecorder()
			NewServer("", account.NewService(account.NewMemoryRepository())).Handler.ServeHTTP(response, httptest.NewRequest(method, "/health", nil))
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
			NewServer("", account.NewService(account.NewMemoryRepository())).Handler.ServeHTTP(response, httptest.NewRequest(http.MethodGet, path, nil))
			assertJSONError(t, response, http.StatusNotFound)
		})
	}
}

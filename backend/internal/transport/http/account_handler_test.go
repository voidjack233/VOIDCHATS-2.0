package httptransport

import (
	"context"
	"encoding/json"
	"errors"
	"net/http"
	"net/http/httptest"
	"net/url"
	"strings"
	"testing"

	"github.com/voidjack233/VOIDCHATS-2.0/backend/internal/account"
)

const validAccountJSON = `{"void_id":"client-generated-id","display_name":"Jack","identity_public_key":"encoded-public-key"}`

func TestAccountCreateGetAndDuplicate(t *testing.T) {
	handler := NewServer("", account.NewService(account.NewMemoryRepository())).Handler
	response := performRequest(handler, http.MethodPost, "/v1/accounts", validAccountJSON)
	created := decodeAccount(t, response, http.StatusCreated)
	if created.VOIDID != "client-generated-id" || created.DisplayName != "Jack" || created.IdentityPublicKey != "encoded-public-key" || created.CreatedAt.IsZero() {
		t.Fatalf("created account = %+v", created)
	}

	duplicate := `{"void_id":"client-generated-id","display_name":"Replacement","identity_public_key":"different-public-key"}`
	response = performRequest(handler, http.MethodPost, "/v1/accounts", duplicate)
	assertJSONError(t, response, http.StatusConflict)
	if !strings.Contains(response.Body.String(), "void_id already exists") {
		t.Fatalf("duplicate response = %q", response.Body.String())
	}

	response = performRequest(handler, http.MethodGet, "/v1/accounts/client-generated-id", "")
	stored := decodeAccount(t, response, http.StatusOK)
	if stored != created {
		t.Fatalf("stored account = %+v, want original %+v", stored, created)
	}
}

func TestAccountNotFound(t *testing.T) {
	handler := NewServer("", account.NewService(account.NewMemoryRepository())).Handler
	response := performRequest(handler, http.MethodGet, "/v1/accounts/missing", "")
	assertJSONError(t, response, http.StatusNotFound)
}

func TestAccountPreservesOpaqueClientIdentity(t *testing.T) {
	handler := NewServer("", account.NewService(account.NewMemoryRepository())).Handler
	request := createAccountRequest{
		VOIDID:            "Client:MiXeD/opaque id",
		DisplayName:       " Jack ",
		IdentityPublicKey: "opaque-client-public-key",
	}
	body, err := json.Marshal(request)
	if err != nil {
		t.Fatal(err)
	}
	created := decodeAccount(t, performRequest(handler, http.MethodPost, "/v1/accounts", string(body)), http.StatusCreated)
	if created.VOIDID != request.VOIDID || created.DisplayName != request.DisplayName || created.IdentityPublicKey != request.IdentityPublicKey {
		t.Fatalf("submitted strings changed: %+v", created)
	}
	path := "/v1/accounts/" + url.PathEscape(request.VOIDID)
	stored := decodeAccount(t, performRequest(handler, http.MethodGet, path, ""), http.StatusOK)
	if stored != created {
		t.Fatalf("stored account = %+v, want %+v", stored, created)
	}
}

func TestAccountRejectsInvalidJSONBeforeStorage(t *testing.T) {
	for _, test := range []struct {
		name string
		body string
	}{
		{name: "empty"},
		{name: "malformed", body: `{"void_id":`},
		{name: "unknown field", body: strings.TrimSuffix(validAccountJSON, "}") + `,"unexpected":true}`},
		{name: "client creation timestamp", body: strings.TrimSuffix(validAccountJSON, "}") + `,"created_at":"2026-10-05T00:00:00Z"}`},
		{name: "private key", body: strings.TrimSuffix(validAccountJSON, "}") + `,"identity_private_key":"never-send-this"}`},
		{name: "trailing object", body: validAccountJSON + `{}`},
		{name: "trailing null", body: validAccountJSON + `null`},
		{name: "trailing garbage", body: validAccountJSON + `garbage`},
		{name: "null", body: `null`},
		{name: "array", body: `[]`},
		{name: "string", body: `"text"`},
		{name: "number", body: `42`},
		{name: "wrong field type", body: `{"void_id":42,"display_name":"Jack","identity_public_key":"public"}`},
		{name: "invalid UTF-8 public key", body: strings.Replace(validAccountJSON, "encoded-public-key", "public-"+string([]byte{0xff})+"-key", 1)},
	} {
		t.Run(test.name, func(t *testing.T) {
			service := account.NewService(account.NewMemoryRepository())
			handler := NewServer("", service).Handler
			response := performRequest(handler, http.MethodPost, "/v1/accounts", test.body)
			assertJSONError(t, response, http.StatusBadRequest)
			if _, err := service.GetByVOIDID(context.Background(), "client-generated-id"); !errors.Is(err, account.ErrNotFound) {
				t.Fatalf("rejected request stored an account: %v", err)
			}
		})
	}
}

func TestAccountRequiresEveryField(t *testing.T) {
	for _, field := range []string{"void_id", "display_name", "identity_public_key"} {
		for _, test := range []struct {
			name  string
			value any
		}{
			{name: "missing"},
			{name: "empty", value: ""},
			{name: "blank", value: " \t\n"},
			{name: "null"},
		} {
			t.Run(field+"/"+test.name, func(t *testing.T) {
				payload := map[string]any{"void_id": "client-generated-id", "display_name": "Jack", "identity_public_key": "encoded-public-key"}
				if test.name == "missing" {
					delete(payload, field)
				} else {
					payload[field] = test.value
				}
				body, err := json.Marshal(payload)
				if err != nil {
					t.Fatal(err)
				}
				handler := NewServer("", account.NewService(account.NewMemoryRepository())).Handler
				response := performRequest(handler, http.MethodPost, "/v1/accounts", string(body))
				assertJSONError(t, response, http.StatusBadRequest)
			})
		}
	}
}

func TestAccountBodyLimit(t *testing.T) {
	service := account.NewService(account.NewMemoryRepository())
	handler := NewServer("", service).Handler
	for _, body := range []string{
		`{"void_id":"client-generated-id","display_name":"` + strings.Repeat("x", maxAccountBodyBytes) + `","identity_public_key":"public"}`,
		validAccountJSON + strings.Repeat(" ", maxAccountBodyBytes),
	} {
		response := performRequest(handler, http.MethodPost, "/v1/accounts", body)
		assertJSONError(t, response, http.StatusRequestEntityTooLarge)
	}
	if _, err := service.GetByVOIDID(context.Background(), "client-generated-id"); !errors.Is(err, account.ErrNotFound) {
		t.Fatalf("oversized request stored an account: %v", err)
	}
}

func TestAccountMethods(t *testing.T) {
	for _, route := range []struct {
		path    string
		allowed string
	}{
		{path: "/v1/accounts", allowed: http.MethodPost},
		{path: "/v1/accounts/client-generated-id", allowed: http.MethodGet},
	} {
		for _, method := range []string{http.MethodGet, http.MethodPost, http.MethodPut, http.MethodDelete, http.MethodHead, http.MethodOptions} {
			if method == route.allowed {
				continue
			}
			t.Run(method+route.path, func(t *testing.T) {
				handler := NewServer("", account.NewService(account.NewMemoryRepository())).Handler
				response := performRequest(handler, method, route.path, "")
				assertJSONError(t, response, http.StatusMethodNotAllowed)
				if got := response.Header().Get("Allow"); got != route.allowed {
					t.Errorf("Allow = %q, want %q", got, route.allowed)
				}
			})
		}
	}
}

func TestAccountRepositoryErrorsArePrivate(t *testing.T) {
	service := account.NewService(failingRepository{})
	handler := NewServer("", service).Handler
	for _, request := range []struct {
		method string
		path   string
		body   string
	}{
		{method: http.MethodPost, path: "/v1/accounts", body: validAccountJSON},
		{method: http.MethodGet, path: "/v1/accounts/client-generated-id"},
	} {
		t.Run(request.method, func(t *testing.T) {
			response := performRequest(handler, request.method, request.path, request.body)
			assertJSONError(t, response, http.StatusInternalServerError)
			if got := response.Body.String(); got != "{\"error\":\"internal server error\"}\n" {
				t.Fatalf("internal failure response = %q", got)
			}
		})
	}
}

func performRequest(handler http.Handler, method, path, body string) *httptest.ResponseRecorder {
	request := httptest.NewRequest(method, path, strings.NewReader(body))
	request.Header.Set("Content-Type", "application/json")
	response := httptest.NewRecorder()
	handler.ServeHTTP(response, request)
	return response
}

func decodeAccount(t *testing.T, response *httptest.ResponseRecorder, status int) account.Account {
	t.Helper()
	if response.Code != status {
		t.Fatalf("status = %d, want %d; body = %s", response.Code, status, response.Body.String())
	}
	if got := response.Header().Get("Content-Type"); got != "application/json" {
		t.Fatalf("Content-Type = %q, want application/json", got)
	}
	var decoded account.Account
	if err := json.Unmarshal(response.Body.Bytes(), &decoded); err != nil {
		t.Fatal(err)
	}
	return decoded
}

func assertJSONError(t *testing.T, response *httptest.ResponseRecorder, status int) {
	t.Helper()
	if response.Code != status {
		t.Fatalf("status = %d, want %d; body = %s", response.Code, status, response.Body.String())
	}
	if got := response.Header().Get("Content-Type"); got != "application/json" {
		t.Fatalf("Content-Type = %q, want application/json", got)
	}
	var payload map[string]string
	if err := json.Unmarshal(response.Body.Bytes(), &payload); err != nil {
		t.Fatalf("invalid JSON error response: %v", err)
	}
	if len(payload) != 1 || payload["error"] == "" {
		t.Fatalf("error response = %v, want one nonempty error field", payload)
	}
}

type failingRepository struct{}

func (failingRepository) Create(context.Context, account.Account) error {
	return errors.New("internal repository detail that must not reach clients")
}

func (failingRepository) GetByVOIDID(context.Context, string) (account.Account, error) {
	return account.Account{}, errors.New("internal repository detail that must not reach clients")
}

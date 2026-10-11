// Package httptransport configures the standard-library HTTP API server.
package httptransport

import (
	"context"
	"net/http"
	"time"

	"github.com/voidjack233/VOIDCHATS-2.0/backend/internal/account"
)

func NewServer(addr string, accounts *account.Service, checkReady func(context.Context) error) *http.Server {
	mux := http.NewServeMux()
	mux.HandleFunc("/health", health)
	mux.HandleFunc("/ready", readiness(checkReady))
	handler := accountHandler{accounts: accounts}
	mux.HandleFunc("/v1/accounts", handler.create)
	mux.HandleFunc("/v1/accounts/{void_id}", handler.get)
	mux.HandleFunc("/", func(w http.ResponseWriter, r *http.Request) {
		writeError(w, http.StatusNotFound, "route not found")
	})

	return &http.Server{
		Addr:              addr,
		Handler:           mux,
		ReadTimeout:       10 * time.Second,
		WriteTimeout:      10 * time.Second,
		ReadHeaderTimeout: 5 * time.Second,
		IdleTimeout:       60 * time.Second,
		MaxHeaderBytes:    1 << 20,
	}
}

func readiness(check func(context.Context) error) http.HandlerFunc {
	return func(w http.ResponseWriter, r *http.Request) {
		if r.Method != http.MethodGet {
			methodNotAllowed(w, http.MethodGet)
			return
		}
		ctx, cancel := context.WithTimeout(r.Context(), 2*time.Second)
		defer cancel()
		status, value := http.StatusOK, "ready"
		if check == nil || check(ctx) != nil {
			status, value = http.StatusServiceUnavailable, "not_ready"
		}
		writeJSON(w, status, struct {
			Status string `json:"status"`
		}{Status: value})
	}
}

func health(w http.ResponseWriter, r *http.Request) {
	if r.Method != http.MethodGet {
		methodNotAllowed(w, http.MethodGet)
		return
	}

	w.Header().Set("Content-Type", "application/json")
	w.WriteHeader(http.StatusOK)
	_, _ = w.Write([]byte(`{"status":"ok"}`))
}

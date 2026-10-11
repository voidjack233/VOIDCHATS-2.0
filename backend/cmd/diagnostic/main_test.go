package main

import (
	"bytes"
	"context"
	"encoding/json"
	"log"
	"net"
	"net/http"
	"net/http/httptest"
	"strings"
	"testing"
	"time"
)

func TestDiagnosticOnlyExposesHealth(t *testing.T) {
	for _, test := range []struct {
		method string
		path   string
		status int
	}{
		{http.MethodGet, "/health", http.StatusOK},
		{http.MethodGet, "/health/", http.StatusNotFound},
		{http.MethodGet, "/ready", http.StatusNotFound},
		{http.MethodPost, "/v1/accounts", http.StatusNotFound},
		{http.MethodGet, "/v1/accounts/id", http.StatusNotFound},
		{http.MethodPost, "/health", http.StatusMethodNotAllowed},
		{http.MethodHead, "/health", http.StatusMethodNotAllowed},
	} {
		t.Run(test.method+test.path, func(t *testing.T) {
			response := httptest.NewRecorder()
			newServer("").Handler.ServeHTTP(response, httptest.NewRequest(test.method, test.path, nil))
			if response.Code != test.status {
				t.Fatalf("status = %d, want %d", response.Code, test.status)
			}
			if response.Header().Get("X-VOID-Diagnostic-Origin") != "health-only" {
				t.Fatal("missing health-only origin identity")
			}
			if response.Header().Get("Content-Type") != "application/json" || !json.Valid(response.Body.Bytes()) {
				t.Fatalf("invalid JSON response: %q", response.Body.String())
			}
			if test.status == http.StatusOK && response.Body.String() != `{"status":"ok"}` {
				t.Fatalf("health response = %q", response.Body.String())
			}
			if test.status == http.StatusMethodNotAllowed && response.Header().Get("Allow") != http.MethodGet {
				t.Fatal("missing Allow: GET")
			}
		})
	}
}

func TestDiagnosticStopsOnCancellation(t *testing.T) {
	listener, err := net.Listen("tcp", "127.0.0.1:0")
	if err != nil {
		t.Fatal(err)
	}
	ctx, cancel := context.WithCancel(context.Background())
	defer cancel()
	var logs bytes.Buffer
	result := make(chan error, 1)
	go func() { result <- serve(ctx, newServer(""), listener, log.New(&logs, "", 0)) }()
	cancel()
	select {
	case err := <-result:
		if err != nil {
			t.Fatal(err)
		}
	case <-time.After(5 * time.Second):
		t.Fatal("diagnostic origin did not shut down")
	}
	if !strings.Contains(logs.String(), "shutdown complete") {
		t.Fatal("missing completed shutdown log")
	}
	connection, err := net.DialTimeout("tcp", listener.Addr().String(), time.Second)
	if err == nil {
		connection.Close()
		t.Fatal("diagnostic origin listener remains open")
	}
}

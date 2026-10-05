package main

import (
	"context"
	"fmt"
	"io"
	"log"
	"net"
	"net/http"
	"sync"
	"testing"
	"time"
)

func TestServeWaitsForInflightRequestDuringShutdown(t *testing.T) {
	listener, err := net.Listen("tcp", "127.0.0.1:0")
	if err != nil {
		t.Fatal(err)
	}
	tracked := &trackedListener{Listener: listener, closed: make(chan struct{})}
	requestStarted := make(chan struct{})
	finishRequest := make(chan struct{})
	var finishOnce sync.Once
	defer finishOnce.Do(func() { close(finishRequest) })

	server := &http.Server{Handler: http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		close(requestStarted)
		<-finishRequest
		_, _ = w.Write([]byte("done"))
	})}
	defer server.Close()
	ctx, cancel := context.WithCancel(context.Background())
	defer cancel()
	serveResult := make(chan error, 1)
	go func() {
		serveResult <- serve(ctx, server, tracked, log.New(io.Discard, "", 0))
	}()
	clientResult := make(chan error, 1)
	go func() {
		client := &http.Client{Timeout: 5 * time.Second}
		response, err := client.Get("http://" + listener.Addr().String())
		if err != nil {
			clientResult <- err
			return
		}
		defer response.Body.Close()
		body, err := io.ReadAll(response.Body)
		if err == nil && (response.StatusCode != http.StatusOK || string(body) != "done") {
			err = fmt.Errorf("response = %d %q, want 200 done", response.StatusCode, body)
		}
		clientResult <- err
	}()

	select {
	case <-requestStarted:
	case <-time.After(5 * time.Second):
		t.Fatal("request did not start")
	}
	cancel()
	select {
	case <-tracked.closed:
	case <-time.After(5 * time.Second):
		t.Fatal("shutdown did not close the listener")
	}
	select {
	case err := <-serveResult:
		t.Fatalf("serve returned before the in-flight request finished: %v", err)
	case <-time.After(25 * time.Millisecond):
	}
	finishOnce.Do(func() { close(finishRequest) })
	select {
	case err := <-clientResult:
		if err != nil {
			t.Fatalf("in-flight request failed: %v", err)
		}
	case <-time.After(5 * time.Second):
		t.Fatal("in-flight request did not finish")
	}
	select {
	case err := <-serveResult:
		if err != nil {
			t.Fatalf("graceful shutdown failed: %v", err)
		}
	case <-time.After(5 * time.Second):
		t.Fatal("serve did not return after graceful shutdown")
	}
}

func TestServeReportsListenerFailure(t *testing.T) {
	listener, err := net.Listen("tcp", "127.0.0.1:0")
	if err != nil {
		t.Fatal(err)
	}
	if err := listener.Close(); err != nil {
		t.Fatal(err)
	}
	server := &http.Server{Handler: http.NotFoundHandler()}
	if err := serve(context.Background(), server, listener, log.New(io.Discard, "", 0)); err == nil {
		t.Fatal("serve accepted a closed listener")
	}
}

type trackedListener struct {
	net.Listener
	closed chan struct{}
	once   sync.Once
}

func (listener *trackedListener) Close() error {
	err := listener.Listener.Close()
	listener.once.Do(func() { close(listener.closed) })
	return err
}

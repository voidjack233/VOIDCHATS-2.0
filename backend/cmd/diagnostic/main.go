// The diagnostic process is an independent health-only Cloudflare Tunnel origin.
package main

import (
	"context"
	"errors"
	"fmt"
	"log"
	"net"
	"net/http"
	"os"
	"os/signal"
	"syscall"
	"time"

	"github.com/voidjack233/VOIDCHATS-2.0/backend/internal/config"
)

func main() {
	ctx, stop := signal.NotifyContext(context.Background(), os.Interrupt, syscall.SIGTERM)
	defer stop()
	if err := run(ctx, log.Default()); err != nil {
		log.Printf("Diagnostic origin failed: %v", err)
		os.Exit(1)
	}
}

func run(ctx context.Context, logger *log.Logger) error {
	addr, err := config.LoadDiagnosticAddr()
	if err != nil {
		return err
	}
	listener, err := net.Listen("tcp", addr)
	if err != nil {
		return fmt.Errorf("listen on diagnostic origin: %w", err)
	}
	return serve(ctx, newServer(addr), listener, logger)
}

func newServer(addr string) *http.Server {
	// This handler deliberately has no account service, database, or proxy.
	handler := http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		w.Header().Set("Content-Type", "application/json")
		w.Header().Set("X-VOID-Diagnostic-Origin", "health-only")
		if r.URL.Path != "/health" {
			w.WriteHeader(http.StatusNotFound)
			_, _ = w.Write([]byte(`{"error":"route not found"}`))
			return
		}
		if r.Method != http.MethodGet {
			w.Header().Set("Allow", http.MethodGet)
			w.WriteHeader(http.StatusMethodNotAllowed)
			_, _ = w.Write([]byte(`{"error":"method not allowed"}`))
			return
		}
		w.WriteHeader(http.StatusOK)
		_, _ = w.Write([]byte(`{"status":"ok"}`))
	})
	return &http.Server{
		Addr:              addr,
		Handler:           handler,
		ReadHeaderTimeout: 5 * time.Second,
		ReadTimeout:       10 * time.Second,
		WriteTimeout:      10 * time.Second,
		IdleTimeout:       60 * time.Second,
		MaxHeaderBytes:    16 * 1024,
	}
}

func serve(ctx context.Context, server *http.Server, listener net.Listener, logger *log.Logger) error {
	logger.Printf("Health-only diagnostic origin listening on %s", listener.Addr())
	result := make(chan error, 1)
	go func() { result <- server.Serve(listener) }()
	select {
	case err := <-result:
		if errors.Is(err, http.ErrServerClosed) {
			return nil
		}
		return errors.Join(fmt.Errorf("serve diagnostic HTTP: %w", err), server.Close())
	case <-ctx.Done():
		logger.Print("Diagnostic origin shutdown requested")
	}
	shutdownCtx, cancel := context.WithTimeout(context.Background(), 10*time.Second)
	defer cancel()
	if err := server.Shutdown(shutdownCtx); err != nil {
		closeErr := server.Close()
		<-result
		return errors.Join(fmt.Errorf("shutdown diagnostic HTTP: %w", err), closeErr)
	}
	if err := <-result; !errors.Is(err, http.ErrServerClosed) {
		return fmt.Errorf("serve diagnostic HTTP during shutdown: %w", err)
	}
	logger.Print("Diagnostic origin shutdown complete")
	return nil
}

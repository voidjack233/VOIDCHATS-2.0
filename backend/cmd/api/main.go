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

	"github.com/voidjack233/VOIDCHATS-2.0/backend/internal/account"
	"github.com/voidjack233/VOIDCHATS-2.0/backend/internal/config"
	httptransport "github.com/voidjack233/VOIDCHATS-2.0/backend/internal/transport/http"
)

const shutdownTimeout = 10 * time.Second

func main() {
	ctx, stop := signal.NotifyContext(context.Background(), os.Interrupt, syscall.SIGTERM)
	defer stop()

	if err := run(ctx, log.Default()); err != nil {
		log.Printf("API failed: %v", err)
		os.Exit(1)
	}
}

func run(ctx context.Context, logger *log.Logger) error {
	cfg, err := config.Load()
	if err != nil {
		return err
	}
	accounts := account.NewService(account.NewMemoryRepository())
	server := httptransport.NewServer(cfg.HTTPAddr, accounts)
	listener, err := net.Listen("tcp", cfg.HTTPAddr)
	if err != nil {
		return fmt.Errorf("listen on %s: %w", cfg.HTTPAddr, err)
	}
	return serve(ctx, server, listener, logger)
}

func serve(ctx context.Context, server *http.Server, listener net.Listener, logger *log.Logger) error {
	logger.Printf("API listening on %s", listener.Addr())
	serveResult := make(chan error, 1)
	go func() {
		serveResult <- server.Serve(listener)
	}()

	select {
	case err := <-serveResult:
		if errors.Is(err, http.ErrServerClosed) {
			return nil
		}
		return errors.Join(fmt.Errorf("serve HTTP: %w", err), server.Close())
	case <-ctx.Done():
		logger.Print("API shutdown requested")
	}

	// The signal context is canceled, so shutdown needs its own fresh deadline.
	shutdownCtx, cancel := context.WithTimeout(context.Background(), shutdownTimeout)
	defer cancel()
	if err := server.Shutdown(shutdownCtx); err != nil {
		closeErr := server.Close()
		<-serveResult
		return errors.Join(fmt.Errorf("shutdown HTTP: %w", err), closeErr)
	}
	if err := <-serveResult; !errors.Is(err, http.ErrServerClosed) {
		return fmt.Errorf("serve HTTP during shutdown: %w", err)
	}
	logger.Print("API shutdown complete")
	return nil
}

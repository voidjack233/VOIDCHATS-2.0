package main

import (
	"context"
	"errors"
	"flag"
	"log/slog"
	"net"
	"net/http"
	"os"
	"os/signal"
	"syscall"
	"time"

	"github.com/jackc/pgx/v5/pgxpool"
	"void-nothingness/internal/account"
	"void-nothingness/internal/config"
	"void-nothingness/internal/httpapi"
	"void-nothingness/internal/postgres"
	"void-nothingness/internal/snowflake"
)

func main() {
	if err := run(); err != nil {
		slog.Error(err.Error())
		os.Exit(1)
	}
}

func run() error {
	envFile := flag.String("env-file", ".env", "literal environment file (process environment overrides it)")
	flag.Parse()
	cfg, err := config.Load(*envFile)
	if err != nil {
		return err
	}
	root, stop := signal.NotifyContext(context.Background(), os.Interrupt, syscall.SIGTERM)
	defer stop()
	poolCfg, err := pgxpool.ParseConfig(cfg.DatabaseURL)
	if err != nil {
		return errors.New("invalid database connection configuration")
	}
	poolCfg.MaxConns = cfg.DatabaseMaxConns
	poolCfg.MinConns = 0
	poolCfg.MaxConnLifetime = 30 * time.Minute
	poolCfg.MaxConnIdleTime = 5 * time.Minute
	poolCfg.ConnConfig.ConnectTimeout = 5 * time.Second
	poolCfg.ConnConfig.RuntimeParams["application_name"] = "void-server"
	poolCfg.ConnConfig.RuntimeParams["statement_timeout"] = "5000"
	poolCfg.ConnConfig.RuntimeParams["lock_timeout"] = "2000"
	poolCfg.ConnConfig.RuntimeParams["search_path"] = "pg_catalog"
	startup, cancel := context.WithTimeout(root, 10*time.Second)
	pool, err := pgxpool.NewWithConfig(startup, poolCfg)
	if err != nil {
		cancel()
		return errors.New("database pool initialization failed")
	}
	defer pool.Close()
	repo := postgres.New(pool)
	err = repo.Validate(startup)
	cancel()
	if err != nil {
		return err
	}
	ids, err := snowflake.New(cfg.WorkerID, cfg.Epoch)
	if err != nil {
		return errors.New("invalid Snowflake configuration")
	}
	service, err := account.NewService(repo, ids, account.Options{ChallengeTTL: cfg.ChallengeTTL, MaxChallenges: cfg.MaxChallenges})
	if err != nil {
		return err
	}
	handler := httpapi.New(service, repo.Ready, httpapi.Options{RequestTimeout: cfg.RequestTimeout, RatePerMinute: cfg.RatePerMinute, RateBurst: cfg.RateBurst, MaxRateClients: cfg.MaxRateClients, MaxConcurrentRequests: cfg.MaxConcurrentRequests})
	server := &http.Server{Addr: cfg.ListenAddr, Handler: handler, ReadHeaderTimeout: 3 * time.Second, ReadTimeout: 5 * time.Second, WriteTimeout: cfg.RequestTimeout + time.Second, IdleTimeout: 30 * time.Second, MaxHeaderBytes: 8192}
	listener, err := net.Listen("tcp", cfg.ListenAddr)
	if err != nil {
		return errors.New("cannot listen on configured HTTP address")
	}
	// Also cancel active handlers if Serve exits unexpectedly, before closing
	// their database pool. Close is harmless after a successful Shutdown.
	defer server.Close()
	serverErr := make(chan error, 1)
	go func() { serverErr <- server.Serve(listener) }()
	slog.Info("VOID-NOTHINGNESS listening", "address", listener.Addr().String())
	select {
	case err := <-serverErr:
		if !errors.Is(err, http.ErrServerClosed) {
			return errors.New("HTTP server failed")
		}
		return nil
	case <-root.Done():
	}
	shutdown, cancel := context.WithTimeout(context.Background(), cfg.ShutdownTimeout)
	defer cancel()
	if err := server.Shutdown(shutdown); err != nil {
		_ = server.Close()
		return errors.New("HTTP graceful shutdown deadline exceeded")
	}
	slog.Info("VOID-NOTHINGNESS stopped")
	return nil
}

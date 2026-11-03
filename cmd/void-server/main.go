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
	"void-nothingness/internal/auth"
	"void-nothingness/internal/config"
	"void-nothingness/internal/httpapi"
	"void-nothingness/internal/postgres"
	"void-nothingness/internal/prekeys"
	"void-nothingness/internal/queue"
	"void-nothingness/internal/relay"
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
	// Keep catalog constraint rendering deterministic for read-only validation.
	poolCfg.ConnConfig.RuntimeParams["intervalstyle"] = "postgres"
	startup, cancel := context.WithTimeout(root, 10*time.Second)
	defer cancel()
	pool, err := pgxpool.NewWithConfig(startup, poolCfg)
	if err != nil {
		return errors.New("database pool initialization failed")
	}
	defer pool.Close()
	repo := postgres.New(pool)
	queueStore := postgres.NewQueueStore(pool)
	prekeyStore := postgres.NewPrekeyStore(pool)
	prekeysEnabled, err := prekeySchemaPresent(startup, pool)
	if err != nil {
		return errors.New("prekey schema inspection unavailable; startup never applies migrations")
	}
	ready := func(ctx context.Context) error {
		if err := repo.Ready(ctx); err != nil {
			return err
		}
		if err := queueStore.Validate(ctx); err != nil {
			return err
		}
		present, err := prekeySchemaPresent(ctx, pool)
		if err != nil || present != prekeysEnabled {
			return errors.New("reviewed schema changed; a matching backend restart is required")
		}
		if present {
			return prekeyStore.Validate(ctx)
		}
		return nil
	}
	if err = ready(startup); err != nil {
		return errors.New("reviewed relay schema or account access unavailable; startup never applies migrations")
	}
	cancel()
	ids, err := snowflake.New(cfg.WorkerID, cfg.Epoch)
	if err != nil {
		return errors.New("invalid Snowflake configuration")
	}
	registration, err := account.NewService(repo, ids, account.Options{ChallengeTTL: cfg.ChallengeTTL, MaxChallenges: cfg.MaxChallenges})
	if err != nil {
		return err
	}
	queued, err := queue.New(queueStore)
	if err != nil {
		return err
	}
	var publicPrekeys *prekeys.Service
	if prekeysEnabled {
		publicPrekeys, err = prekeys.New(prekeyStore, repo, cfg.APIAudience)
		if err != nil {
			return errors.New("invalid public prekey service configuration")
		}
	}
	authentication, err := auth.NewServiceForAudience(repo, cfg.WSAuthTimeout, cfg.APIAudience)
	if err != nil {
		return errors.New("invalid WebSocket authentication configuration")
	}
	sockets, err := relay.New(authentication, relay.Options{
		AuthTimeout: cfg.WSAuthTimeout, MaxConnections: cfg.WSMaxConnections,
		QueueSize: cfg.WSQueueSize, MaxPayloadBytes: cfg.WSMaxPayloadBytes,
		MaxFrameBytes: int64(((cfg.WSMaxPayloadBytes + 2) / 3 * 4) + 2048), Queue: queued, Prekeys: publicPrekeys,
	})
	if err != nil {
		return errors.New("invalid WebSocket configuration")
	}
	defer sockets.Close()
	routes := http.NewServeMux()
	// Upgraded sockets own their deadlines, outside registration's short timeout.
	routes.Handle("/dm9pZG5vdGhpbmduZXNz/ws", sockets)
	routes.Handle("/", httpapi.New(registration, ready, httpapi.Options{
		RequestTimeout: cfg.RequestTimeout, RatePerMinute: cfg.RatePerMinute,
		RateBurst: cfg.RateBurst, MaxRateClients: cfg.MaxRateClients,
		MaxConcurrentRequests: cfg.MaxConcurrentRequests,
	}))
	server := &http.Server{Addr: cfg.ListenAddr, Handler: routes, ReadHeaderTimeout: 3 * time.Second, ReadTimeout: 5 * time.Second, WriteTimeout: cfg.RequestTimeout + time.Second, IdleTimeout: 30 * time.Second, MaxHeaderBytes: 8192}
	listener, err := net.Listen("tcp", cfg.ListenAddr)
	if err != nil {
		return errors.New("cannot listen on configured HTTP address")
	}
	defer server.Close()
	cleanupCtx, cleanupCancel := context.WithCancel(root)
	cleanupDone := make(chan struct{})
	go func() { defer close(cleanupDone); maintainQueue(cleanupCtx, queued, publicPrekeys) }()
	defer func() { cleanupCancel(); <-cleanupDone }()
	serverErr := make(chan error, 1)
	go func() { serverErr <- server.Serve(listener) }()
	slog.Info("VOID-NOTHINGNESS listening", "address", listener.Addr().String(), "messaging", "temporary_websocket_relay")
	select {
	case err := <-serverErr:
		if !errors.Is(err, http.ErrServerClosed) {
			return errors.New("HTTP server failed")
		}
		return nil
	case <-root.Done():
	}
	_ = sockets.Close()
	shutdown, cancel := context.WithTimeout(context.Background(), cfg.ShutdownTimeout)
	defer cancel()
	if err := server.Shutdown(shutdown); err != nil {
		_ = server.Close()
		return errors.New("HTTP graceful shutdown deadline exceeded")
	}
	slog.Info("VOID-NOTHINGNESS stopped")
	return nil
}

// Cleanup uses the same process and store, with short bounded batches. Expiry is
// also checked by every queue read; a cleanup delay never permits old delivery.
func maintainQueue(ctx context.Context, service *queue.Service, publicPrekeys *prekeys.Service) {
	ticker := time.NewTicker(time.Minute)
	defer ticker.Stop()
	for {
		batchCtx, cancel := context.WithTimeout(ctx, 5*time.Second)
		for batch := 0; batch < 10; batch++ {
			n, err := service.Cleanup(batchCtx, 1000)
			if err != nil {
				if ctx.Err() == nil {
					slog.Warn("temporary queue cleanup unavailable")
				}
				break
			}
			if n < 1000 {
				break
			}
		}
		cancel()
		if publicPrekeys != nil {
			prekeyCtx, prekeyCancel := context.WithTimeout(ctx, 5*time.Second)
			for batch := 0; batch < 10; batch++ {
				n, err := publicPrekeys.Cleanup(prekeyCtx, 1000)
				if err != nil {
					if ctx.Err() == nil {
						slog.Warn("public prekey cleanup unavailable")
					}
					break
				}
				if n < 1000 {
					break
				}
			}
			prekeyCancel()
		}
		select {
		case <-ctx.Done():
			return
		case <-ticker.C:
		}
	}
}

// The extension is optional until its reviewed migration is applied. A partial
// installation is rejected, and no startup path creates or changes tables.
func prekeySchemaPresent(ctx context.Context, pool *pgxpool.Pool) (bool, error) {
	var count int
	err := pool.QueryRow(ctx, `SELECT count(*) FROM pg_catalog.pg_class c JOIN pg_catalog.pg_namespace n ON n.oid=c.relnamespace WHERE n.nspname='public' AND c.relkind IN ('r','p') AND c.relname IN ('signal_prekey_inventory','signal_prekey_claims')`).Scan(&count)
	if err != nil {
		return false, errors.New("public prekey schema unavailable")
	}
	if count != 0 && count != 2 {
		return false, errors.New("public prekey schema is incomplete")
	}
	return count == 2, nil
}

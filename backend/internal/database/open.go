package database

import (
	"context"
	"errors"
	"net"
	"net/url"
	"strconv"
	"time"

	"github.com/jackc/pgx/v5/pgxpool"
	"github.com/voidjack233/VOIDCHATS-2.0/backend/internal/config"
)

const startupTimeout = 5 * time.Second

// Open creates a bounded pool and verifies connectivity before returning it.
// Returned errors omit driver details, which may contain connection credentials.
func Open(ctx context.Context, cfg config.Database) (*pgxpool.Pool, error) {
	poolConfig, err := poolConfig(cfg)
	if err != nil {
		return nil, err
	}
	startupCtx, cancel := context.WithTimeout(ctx, startupTimeout)
	defer cancel()
	pool, err := pgxpool.NewWithConfig(startupCtx, poolConfig)
	if err != nil {
		return nil, errors.New("database pool initialization failed; check database configuration")
	}
	if err := pool.Ping(startupCtx); err != nil {
		pool.Close()
		return nil, errors.New("database unavailable; check PostgreSQL status and connection configuration")
	}
	return pool, nil
}

func poolConfig(cfg config.Database) (*pgxpool.Config, error) {
	connectionURL := url.URL{
		Scheme: "postgres",
		Host:   net.JoinHostPort(cfg.Host, strconv.Itoa(int(cfg.Port))),
		User:   url.UserPassword(cfg.User, cfg.Password),
		Path:   "/" + cfg.Name,
	}
	parameters := url.Values{
		"sslmode":          {cfg.SSLMode},
		"connect_timeout":  {"5"},
		"application_name": {"void-backend"},
		"timezone":         {"UTC"},
	}
	if cfg.SSLRootCert != "" {
		parameters.Set("sslrootcert", cfg.SSLRootCert)
	}
	connectionURL.RawQuery = parameters.Encode()
	parsed, err := pgxpool.ParseConfig(connectionURL.String())
	if err != nil {
		return nil, errors.New("invalid database configuration")
	}
	parsed.MaxConns = 4
	parsed.MinConns = 0
	parsed.MaxConnLifetime = 30 * time.Minute
	parsed.MaxConnIdleTime = 5 * time.Minute
	parsed.HealthCheckPeriod = time.Minute
	return parsed, nil
}

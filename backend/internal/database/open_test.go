package database

import (
	"context"
	"strings"
	"testing"
	"time"

	"github.com/voidjack233/VOIDCHATS-2.0/backend/internal/config"
)

func TestPoolConfigPreservesEscapedCredentialsAndBoundsConnections(t *testing.T) {
	cfg := config.Database{Host: "127.0.0.1", Port: 5432, User: "test@user", Password: "test:/@'%password", Name: "test database", SSLMode: "disable"}
	parsed, err := poolConfig(cfg)
	if err != nil {
		t.Fatal(err)
	}
	if parsed.ConnConfig.User != cfg.User || parsed.ConnConfig.Password != cfg.Password || parsed.ConnConfig.Database != cfg.Name {
		t.Fatal("connection configuration changed escaped credentials or database name")
	}
	if parsed.MaxConns != 4 || parsed.ConnConfig.ConnectTimeout != 5*time.Second || parsed.ConnConfig.RuntimeParams["timezone"] != "UTC" {
		t.Fatal("pool bounds, connection deadline, or UTC session setting incorrect")
	}
	if parsed.ConnConfig.TLSConfig != nil || len(parsed.ConnConfig.Fallbacks) != 0 {
		t.Fatal("loopback-only disable mode has unexpected TLS/fallback configuration")
	}
}

func TestPoolConfigVerifiedTLSHasNoUnencryptedFallback(t *testing.T) {
	cfg := config.Database{Host: "database.example.test", Port: 5432, User: "test", Password: "test", Name: "test", SSLMode: "verify-full"}
	parsed, err := poolConfig(cfg)
	if err != nil {
		t.Fatal(err)
	}
	tls := parsed.ConnConfig.TLSConfig
	if tls == nil || tls.InsecureSkipVerify || tls.ServerName != cfg.Host || len(parsed.ConnConfig.Fallbacks) != 0 {
		t.Fatal("remote database is not configured for verified TLS without plaintext fallback")
	}
}

func TestOpenFailureDoesNotExposeCredentials(t *testing.T) {
	cfg := config.Database{Host: "127.0.0.1", Port: 5432, User: "test-user", Password: "test-secret-never-print", Name: "test", SSLMode: "invalid"}
	pool, err := Open(context.Background(), cfg)
	if err == nil || pool != nil {
		t.Fatal("invalid configuration did not fail")
	}
	if strings.Contains(err.Error(), cfg.Password) || strings.Contains(err.Error(), "postgres://") {
		t.Fatal("configuration error exposed sensitive connection details")
	}
	cfg.SSLMode = "disable"
	ctx, cancel := context.WithCancel(context.Background())
	cancel()
	pool, err = Open(ctx, cfg)
	if err == nil || pool != nil {
		t.Fatal("canceled startup did not fail")
	}
	if strings.Contains(err.Error(), cfg.Password) || strings.Contains(err.Error(), "postgres://") {
		t.Fatal("connection failure exposed sensitive connection details")
	}
}

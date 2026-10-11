package database

import (
	"context"
	"crypto/rand"
	"errors"
	"os"
	"strings"
	"sync"
	"testing"
	"time"

	"github.com/jackc/pgx/v5"
	"github.com/jackc/pgx/v5/pgxpool"
	"github.com/voidjack233/VOIDCHATS-2.0/backend/internal/config"
)

func TestPostgresMigrationsConcurrentIdempotentAndSchemaCheck(t *testing.T) {
	pool := migrationTestPool(t)
	if err := CheckSchema(context.Background(), pool); !errors.Is(err, ErrSchemaNotReady) {
		t.Fatalf("fresh schema check = %v, want ErrSchemaNotReady", err)
	}
	const runners = 6
	var wg sync.WaitGroup
	results := make(chan error, runners)
	for i := 0; i < runners; i++ {
		wg.Add(1)
		go func() {
			defer wg.Done()
			results <- Migrate(context.Background(), pool)
		}()
	}
	wg.Wait()
	close(results)
	for err := range results {
		if err != nil {
			t.Fatalf("concurrent migration: %v", err)
		}
	}
	if err := Migrate(context.Background(), pool); err != nil {
		t.Fatalf("repeated migration: %v", err)
	}
	if err := CheckSchema(context.Background(), pool); err != nil {
		t.Fatalf("migrated schema check: %v", err)
	}
	var count int
	if err := pool.QueryRow(context.Background(), "SELECT count(*) FROM schema_migrations").Scan(&count); err != nil {
		t.Fatal("could not read test migration history")
	}
	if count != 1 {
		t.Fatalf("history entry count = %d, want 1", count)
	}
	var privateColumns int
	if err := pool.QueryRow(context.Background(), `
		SELECT count(*) FROM information_schema.columns
		WHERE table_schema = current_schema() AND table_name = 'accounts'
		AND column_name IN ('identity_private_key', 'password', 'recovery_key')`).Scan(&privateColumns); err != nil {
		t.Fatal("could not inspect test account columns")
	}
	if privateColumns != 0 {
		t.Fatal("private cryptographic material must have no account column")
	}
}

func TestPostgresMigrationsRejectChangedHistory(t *testing.T) {
	pool := migrationTestPool(t)
	if err := Migrate(context.Background(), pool); err != nil {
		t.Fatalf("initial migration: %v", err)
	}
	if _, err := pool.Exec(context.Background(), "UPDATE schema_migrations SET checksum = $1 WHERE version = $2", "changed-test-checksum", 1); err != nil {
		t.Fatal("could not change test-owned migration history")
	}
	if err := Migrate(context.Background(), pool); !errors.Is(err, ErrMigrationHistory) {
		t.Fatalf("changed migration history = %v, want ErrMigrationHistory", err)
	}
	if err := CheckSchema(context.Background(), pool); !errors.Is(err, ErrSchemaNotReady) {
		t.Fatalf("changed schema history check = %v, want ErrSchemaNotReady", err)
	}
}

func TestPostgresSchemaCheckRequiresUniqueConstraint(t *testing.T) {
	pool := migrationTestPool(t)
	if err := Migrate(context.Background(), pool); err != nil {
		t.Fatalf("initial migration: %v", err)
	}
	if _, err := pool.Exec(context.Background(), "ALTER TABLE accounts DROP CONSTRAINT accounts_void_id_key"); err != nil {
		t.Fatal("could not alter test-owned account constraint")
	}
	if err := CheckSchema(context.Background(), pool); !errors.Is(err, ErrSchemaNotReady) {
		t.Fatalf("missing uniqueness check = %v, want ErrSchemaNotReady", err)
	}
}

func TestPostgresSchemaCheckRequiresIdentifierLimit(t *testing.T) {
	pool := migrationTestPool(t)
	if err := Migrate(context.Background(), pool); err != nil {
		t.Fatalf("initial migration: %v", err)
	}
	if _, err := pool.Exec(context.Background(), `
		ALTER TABLE accounts DROP CONSTRAINT accounts_void_id_length_check;
		ALTER TABLE accounts ADD CONSTRAINT accounts_void_id_length_check CHECK (true)`); err != nil {
		t.Fatal("could not change isolated identifier constraint")
	}
	if err := CheckSchema(context.Background(), pool); !errors.Is(err, ErrSchemaNotReady) {
		t.Fatalf("changed identifier limit check = %v, want ErrSchemaNotReady", err)
	}
}

// Each integration test creates and removes its own unpredictable schema. No
// existing account, schema, container, or volume is changed by these tests.
func migrationTestPool(t *testing.T) *pgxpool.Pool {
	t.Helper()
	if os.Getenv("VOID_POSTGRES_TEST") != "1" {
		t.Skip("set VOID_POSTGRES_TEST=1 and development PG environment to run PostgreSQL integration tests")
	}
	cfg, err := config.Load()
	if err != nil {
		t.Fatal("invalid PostgreSQL integration-test configuration")
	}
	ctx, cancel := context.WithTimeout(context.Background(), 5*time.Second)
	defer cancel()
	admin, err := Open(ctx, cfg.Database)
	if err != nil {
		t.Fatal("PostgreSQL integration-test database unavailable")
	}
	t.Cleanup(admin.Close)
	schema := "void_migration_test_" + strings.ToLower(rand.Text())
	quotedSchema := pgx.Identifier{schema}.Sanitize()
	if _, err := admin.Exec(ctx, "CREATE SCHEMA "+quotedSchema); err != nil {
		t.Fatal("could not create isolated PostgreSQL test schema")
	}
	t.Cleanup(func() {
		cleanupCtx, cleanupCancel := context.WithTimeout(context.Background(), 5*time.Second)
		defer cleanupCancel()
		if _, err := admin.Exec(cleanupCtx, "DROP SCHEMA "+quotedSchema+" CASCADE"); err != nil {
			t.Error("could not remove isolated PostgreSQL test schema")
		}
	})
	poolConfig := admin.Config()
	poolConfig.ConnConfig.RuntimeParams["search_path"] = schema
	pool, err := pgxpool.NewWithConfig(ctx, poolConfig)
	if err != nil {
		t.Fatal("could not open isolated PostgreSQL test pool")
	}
	t.Cleanup(pool.Close)
	return pool
}

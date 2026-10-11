package account

import (
	"context"
	"crypto/rand"
	"errors"
	"fmt"
	"os"
	"strings"
	"sync"
	"testing"
	"time"

	"github.com/jackc/pgx/v5"
	"github.com/jackc/pgx/v5/pgxpool"
	"github.com/voidjack233/VOIDCHATS-2.0/backend/internal/config"
	"github.com/voidjack233/VOIDCHATS-2.0/backend/internal/database"
)

func TestPostgresAccountCreationLookupDuplicatesAndPoolRestart(t *testing.T) {
	pool := accountTestPool(t)
	repository := NewPostgresRepository(pool)
	first := Account{
		VOIDID:            " client-ID'; DROP TABLE accounts; -- ",
		DisplayName:       " Jack 台灣 ",
		IdentityPublicKey: " public-test-fixture'; -- ",
		CreatedAt:         time.Date(2026, time.October, 11, 12, 34, 56, 123456000, time.FixedZone("fixture", 8*60*60)),
	}
	if err := repository.Create(context.Background(), first); err != nil {
		t.Fatalf("create fixture: %v", err)
	}
	expected := first
	expected.CreatedAt = expected.CreatedAt.UTC()
	stored, err := repository.GetByVOIDID(context.Background(), first.VOIDID)
	if err != nil || stored != expected || stored.CreatedAt.Location() != time.UTC {
		t.Fatal("retrieval must preserve client fields and the UTC timestamp")
	}
	if _, err := repository.GetByVOIDID(context.Background(), "not-found"); !errors.Is(err, ErrNotFound) {
		t.Fatalf("missing fixture lookup = %v, want ErrNotFound", err)
	}
	duplicate := first
	duplicate.DisplayName = "replacement must not be stored"
	duplicate.IdentityPublicKey = "replacement-public-test-fixture"
	if err := repository.Create(context.Background(), duplicate); !errors.Is(err, ErrAlreadyExists) {
		t.Fatalf("duplicate fixture = %v, want ErrAlreadyExists", err)
	}
	stored, err = repository.GetByVOIDID(context.Background(), first.VOIDID)
	if err != nil || stored != expected {
		t.Fatal("duplicate must preserve the first stored account")
	}
	for _, id := range []string{"client-id", "CLIENT-ID", " client-id "} {
		candidate := first
		candidate.VOIDID = id
		if err := repository.Create(context.Background(), candidate); err != nil {
			t.Fatal("case and whitespace variants must remain distinct identifiers")
		}
	}
	poolConfig := pool.Config()
	pool.Close()
	reopened, err := pgxpool.NewWithConfig(context.Background(), poolConfig)
	if err != nil {
		t.Fatal("could not reopen PostgreSQL fixture pool")
	}
	t.Cleanup(reopened.Close)
	stored, err = NewPostgresRepository(reopened).GetByVOIDID(context.Background(), first.VOIDID)
	if err != nil || stored != expected {
		t.Fatal("account must persist across pool/API connection lifecycle restarts")
	}
}

func TestPostgresConcurrentDuplicateAccountCreation(t *testing.T) {
	pool := accountTestPool(t)
	repository := NewPostgresRepository(pool)
	const attempts = 32
	var wg sync.WaitGroup
	results := make(chan error, attempts)
	winners := make(chan Account, attempts)
	start := make(chan struct{})
	for i := 0; i < attempts; i++ {
		candidate := Account{
			VOIDID:            "same-fixture-id",
			DisplayName:       fmt.Sprintf("fixture-%d", i),
			IdentityPublicKey: fmt.Sprintf("public-test-fixture-%d", i),
			CreatedAt:         time.Unix(int64(i), 0).UTC(),
		}
		wg.Add(1)
		go func() {
			defer wg.Done()
			<-start
			err := repository.Create(context.Background(), candidate)
			if err == nil {
				winners <- candidate
			}
			results <- err
		}()
	}
	close(start)
	wg.Wait()
	close(results)
	close(winners)
	successes, duplicates := 0, 0
	for err := range results {
		switch {
		case err == nil:
			successes++
		case errors.Is(err, ErrAlreadyExists):
			duplicates++
		default:
			t.Fatalf("unexpected concurrent creation error: %v", err)
		}
	}
	if successes != 1 || duplicates != attempts-1 {
		t.Fatalf("concurrent results: %d successes, %d duplicates", successes, duplicates)
	}
	expected := <-winners
	stored, err := repository.GetByVOIDID(context.Background(), expected.VOIDID)
	if err != nil || stored != expected {
		t.Fatal("stored account must exactly match the successful concurrent creator")
	}
}

func TestPostgresServiceReturnsExactStoredCreationTime(t *testing.T) {
	pool := accountTestPool(t)
	service := NewService(NewPostgresRepository(pool))
	created, err := service.Create(context.Background(), "client-service-fixture", "Jack", "public-test-fixture")
	if err != nil {
		t.Fatalf("service creation: %v", err)
	}
	stored, err := service.GetByVOIDID(context.Background(), created.VOIDID)
	if err != nil || stored != created {
		t.Fatal("creation and retrieval must return identical public fields and UTC microsecond timestamps")
	}
}

func TestPostgresContextDeadlineWhileTableLocked(t *testing.T) {
	pool := accountTestPool(t)
	tx, err := pool.Begin(context.Background())
	if err != nil {
		t.Fatal("could not begin fixture lock transaction")
	}
	defer func() { _ = tx.Rollback(context.Background()) }()
	if _, err := tx.Exec(context.Background(), "LOCK TABLE accounts IN ACCESS EXCLUSIVE MODE"); err != nil {
		t.Fatal("could not lock isolated fixture table")
	}
	ctx, cancel := context.WithTimeout(context.Background(), 100*time.Millisecond)
	defer cancel()
	err = NewPostgresRepository(pool).Create(ctx, Account{
		VOIDID: "deadline-fixture", DisplayName: "Jack", IdentityPublicKey: "public-test-fixture", CreatedAt: time.Now().UTC(),
	})
	if !errors.Is(err, context.DeadlineExceeded) {
		t.Fatalf("blocked Create() = %v, want context.DeadlineExceeded", err)
	}
}

func TestPostgresEnforcesIdentifierByteLimit(t *testing.T) {
	pool := accountTestPool(t)
	repository := NewPostgresRepository(pool)
	candidate := Account{VOIDID: strings.Repeat("界", 170) + "ab", DisplayName: "Jack", IdentityPublicKey: "public-test-fixture", CreatedAt: time.Now().UTC()}
	if err := repository.Create(context.Background(), candidate); err != nil {
		t.Fatalf("512-byte ID rejected: %v", err)
	}
	candidate.VOIDID = strings.Repeat("界", 171)
	if err := repository.Create(context.Background(), candidate); err != errPostgresOperation {
		t.Fatalf("513-byte storage insert = %v, want sanitized constraint failure", err)
	}
}

func accountTestPool(t *testing.T) *pgxpool.Pool {
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
	admin, err := database.Open(ctx, cfg.Database)
	if err != nil {
		t.Fatal("PostgreSQL integration-test database unavailable")
	}
	t.Cleanup(admin.Close)
	schema := "void_account_test_" + strings.ToLower(rand.Text())
	quotedSchema := pgx.Identifier{schema}.Sanitize()
	if _, err := admin.Exec(ctx, "CREATE SCHEMA "+quotedSchema); err != nil {
		t.Fatal("could not create isolated PostgreSQL account schema")
	}
	t.Cleanup(func() {
		cleanupCtx, cleanupCancel := context.WithTimeout(context.Background(), 5*time.Second)
		defer cleanupCancel()
		if _, err := admin.Exec(cleanupCtx, "DROP SCHEMA "+quotedSchema+" CASCADE"); err != nil {
			t.Error("could not remove isolated PostgreSQL account schema")
		}
	})
	poolConfig := admin.Config()
	poolConfig.ConnConfig.RuntimeParams["search_path"] = schema
	pool, err := pgxpool.NewWithConfig(ctx, poolConfig)
	if err != nil {
		t.Fatal("could not open isolated PostgreSQL account pool")
	}
	t.Cleanup(pool.Close)
	if err := database.Migrate(context.Background(), pool); err != nil {
		t.Fatalf("fixture schema migration: %v", err)
	}
	return pool
}

// These tests only use an explicitly configured disposable database. Schema and
// role provisioning belongs to the integration runner, never the application.
package integration

import (
	"bytes"
	"context"
	"crypto/ed25519"
	"crypto/rand"
	"encoding/base64"
	"encoding/binary"
	"encoding/json"
	"errors"
	"io"
	"net/http"
	"net/http/httptest"
	"os"
	"strconv"
	"strings"
	"sync"
	"testing"
	"time"

	"github.com/jackc/pgx/v5/pgconn"
	"github.com/jackc/pgx/v5/pgxpool"

	"void-nothingness/internal/account"
	"void-nothingness/internal/httpapi"
	"void-nothingness/internal/postgres"
	"void-nothingness/internal/snowflake"
)

func testPools(t *testing.T) (*pgxpool.Pool, *pgxpool.Pool) {
	t.Helper()
	if os.Getenv("VOID_RUN_POSTGRES_INTEGRATION") != "1" {
		t.Skip("PostgreSQL integration tests are disabled; explicit permission and VOID_RUN_POSTGRES_INTEGRATION=1 are required")
	}
	appURL := os.Getenv("VOID_TEST_DATABASE_URL")
	if appURL == "" {
		t.Skip("set VOID_TEST_DATABASE_URL to an explicitly provisioned disposable void_test_ database")
	}
	open := func(value, wantDatabase string) *pgxpool.Pool {
		t.Helper()
		cfg, err := pgxpool.ParseConfig(value)
		if err != nil {
			t.Fatal("invalid integration database configuration")
		}
		if !strings.HasPrefix(cfg.ConnConfig.Database, "void_test_") || cfg.ConnConfig.Database == "void_test_" ||
			(wantDatabase != "" && cfg.ConnConfig.Database != wantDatabase) {
			t.Fatal("refusing integration access: database must be a dedicated void_test_ database shared by both test URLs")
		}
		cfg.MaxConns = 4
		cfg.ConnConfig.ConnectTimeout = 3 * time.Second
		ctx, cancel := context.WithTimeout(context.Background(), 5*time.Second)
		defer cancel()
		pool, err := pgxpool.NewWithConfig(ctx, cfg)
		if err != nil {
			t.Fatal("could not initialize integration database pool")
		}
		t.Cleanup(pool.Close)
		var actualDatabase string
		if err := pool.QueryRow(ctx, "SELECT current_database()").Scan(&actualDatabase); err != nil || actualDatabase != cfg.ConnConfig.Database {
			t.Fatal("could not verify disposable integration database connection")
		}
		return pool
	}
	app := open(appURL, "")
	var admin *pgxpool.Pool
	if adminURL := os.Getenv("VOID_TEST_ADMIN_URL"); adminURL != "" {
		admin = open(adminURL, app.Config().ConnConfig.Database)
	}
	return app, admin
}

func freshID(t *testing.T) int64 {
	t.Helper()
	var data [8]byte
	if _, err := rand.Read(data[:]); err != nil {
		t.Fatal(err)
	}
	return int64(binary.BigEndian.Uint64(data[:])&((1<<63)-2)) + 1
}

func freshKey(t *testing.T) ed25519.PublicKey {
	t.Helper()
	key, _, err := ed25519.GenerateKey(rand.Reader)
	if err != nil {
		t.Fatal(err)
	}
	return key
}

func TestPostgresPersistenceAndUniqueness(t *testing.T) {
	app, admin := testPools(t)
	repo := postgres.New(app)
	ctx, cancel := context.WithTimeout(context.Background(), 15*time.Second)
	defer cancel()
	if err := repo.Validate(ctx); err != nil {
		t.Fatalf("runtime schema/role validation: %v", err)
	}
	id, key := freshID(t), freshKey(t)
	before := time.Now().Add(-time.Second)
	result, err := repo.Insert(ctx, id, "O'Reilly", key)
	if err != nil {
		t.Fatal(err)
	}
	if result.VoidID != strconv.FormatInt(id, 10) || result.Username != "O'Reilly" || result.CreatedAt.Before(before) || result.CreatedAt.After(time.Now().Add(time.Second)) {
		t.Fatalf("incorrect persisted public account: %+v", result)
	}
	if _, err := repo.Insert(ctx, freshID(t), "duplicate-key", key); !errors.Is(err, account.ErrDuplicateKey) {
		t.Fatalf("duplicate key: got %v", err)
	}
	if _, err := repo.Insert(ctx, id, "duplicate-id", freshKey(t)); !errors.Is(err, account.ErrIDCollision) {
		t.Fatalf("duplicate VOID-ID: got %v", err)
	}
	if err := repo.Ready(ctx); err != nil {
		t.Fatalf("readiness after insert: %v", err)
	}
	if admin == nil {
		t.Log("VOID_TEST_ADMIN_URL absent: internal UUID inspection omitted")
		return
	}
	var internalID string
	var savedKey []byte
	var savedCreated time.Time
	if err := admin.QueryRow(ctx, "SELECT id::text, auth_public_key, created_at FROM public.accounts WHERE void_id = $1", id).
		Scan(&internalID, &savedKey, &savedCreated); err != nil {
		t.Fatal("could not inspect persisted account")
	}
	if len(internalID) != 36 || internalID[14] != '7' || !bytes.Equal(savedKey, key) || !savedCreated.Equal(result.CreatedAt) {
		t.Fatal("PostgreSQL did not persist UUIDv7, public key, and default creation timestamp correctly")
	}
}

func TestPostgresRuntimePrivileges(t *testing.T) {
	app, _ := testPools(t)
	ctx, cancel := context.WithTimeout(context.Background(), 10*time.Second)
	defer cancel()
	if err := postgres.New(app).Validate(ctx); err != nil {
		t.Fatal(err)
	}
	queries := []string{
		"SELECT id FROM public.accounts LIMIT 1",
		"SELECT auth_public_key FROM public.accounts LIMIT 1",
		"SELECT * FROM public.accounts LIMIT 1",
		"UPDATE public.accounts SET username = 'forbidden' WHERE false",
		"DELETE FROM public.accounts WHERE false",
		"INSERT INTO public.accounts (id, void_id, username, auth_public_key) VALUES (uuidv7(), 1, 'forbidden', decode(repeat('00', 32), 'hex'))",
		"INSERT INTO public.accounts (void_id, username, auth_public_key, created_at) VALUES (1, 'forbidden', decode(repeat('00', 32), 'hex'), now())",
	}
	for _, query := range queries {
		_, err := app.Exec(ctx, query)
		var databaseError *pgconn.PgError
		if !errors.As(err, &databaseError) || databaseError.Code != "42501" {
			t.Fatalf("runtime role unexpectedly permitted a restricted operation; error code = %v", err)
		}
	}
}

func TestPostgresConcurrentDuplicateKeys(t *testing.T) {
	app, _ := testPools(t)
	repo := postgres.New(app)
	ctx, cancel := context.WithTimeout(context.Background(), 15*time.Second)
	defer cancel()
	key := freshKey(t)
	const workers = 12
	ids := make([]int64, workers)
	for i := range ids {
		ids[i] = freshID(t)
	}
	results := make(chan error, workers)
	var group sync.WaitGroup
	for _, id := range ids {
		group.Go(func() {
			_, err := repo.Insert(ctx, id, "concurrent", key)
			results <- err
		})
	}
	group.Wait()
	close(results)
	successes := 0
	for err := range results {
		if err == nil {
			successes++
		} else if !errors.Is(err, account.ErrDuplicateKey) {
			t.Fatalf("unexpected concurrent registration error: %v", err)
		}
	}
	if successes != 1 {
		t.Fatalf("got %d successful inserts for one public key, want 1", successes)
	}
}

func TestPostgresInterruptedInsert(t *testing.T) {
	app, admin := testPools(t)
	if admin == nil {
		t.Skip("VOID_TEST_ADMIN_URL required to hold a temporary table lock")
	}
	ctx, cancel := context.WithTimeout(context.Background(), 10*time.Second)
	defer cancel()
	tx, err := admin.Begin(ctx)
	if err != nil {
		t.Fatal("could not start disposable test transaction")
	}
	defer tx.Rollback(context.Background())
	if _, err := tx.Exec(ctx, "LOCK TABLE public.accounts IN ACCESS EXCLUSIVE MODE"); err != nil {
		t.Fatal("could not acquire disposable test lock")
	}
	id := freshID(t)
	interrupted, stop := context.WithTimeout(ctx, 100*time.Millisecond)
	defer stop()
	_, err = postgres.New(app).Insert(interrupted, id, "interrupted", freshKey(t))
	if !errors.Is(err, context.DeadlineExceeded) {
		t.Fatalf("interrupted insert returned %v", err)
	}
	if err := tx.Rollback(ctx); err != nil {
		t.Fatal("could not release disposable test lock")
	}
	var count int
	if err := app.QueryRow(ctx, "SELECT count(void_id) FROM public.accounts WHERE void_id = $1", id).Scan(&count); err != nil || count != 0 {
		t.Fatal("interrupted insert persisted an account")
	}
}

func TestHTTPRegistrationPersistsInPostgres(t *testing.T) {
	app, admin := testPools(t)
	repo := postgres.New(app)
	ids, err := snowflake.New(777, time.Date(2026, 1, 1, 0, 0, 0, 0, time.UTC))
	if err != nil {
		t.Fatal(err)
	}
	service, err := account.NewService(repo, ids, account.Options{ChallengeTTL: time.Minute, MaxChallenges: 20})
	if err != nil {
		t.Fatal(err)
	}
	server := httptest.NewServer(httpapi.New(service, repo.Ready, httpapi.Options{}))
	defer server.Close()
	client := server.Client()
	client.Timeout = 5 * time.Second
	publicKey, privateKey, err := ed25519.GenerateKey(rand.Reader)
	if err != nil {
		t.Fatal(err)
	}
	post := func(path string, body any) (int, []byte) {
		t.Helper()
		payload, err := json.Marshal(body)
		if err != nil {
			t.Fatal(err)
		}
		response, err := client.Post(server.URL+path, "application/json", bytes.NewReader(payload))
		if err != nil {
			t.Fatal(err)
		}
		defer response.Body.Close()
		data, err := io.ReadAll(io.LimitReader(response.Body, 8192))
		if err != nil {
			t.Fatal(err)
		}
		return response.StatusCode, data
	}
	status, data := post("/v1/registration/challenges", map[string]string{
		"username": "http-integration", "auth_public_key": base64.StdEncoding.EncodeToString(publicKey),
	})
	if status != http.StatusCreated {
		t.Fatalf("challenge status %d: %s", status, data)
	}
	var challenge account.Challenge
	if err := json.Unmarshal(data, &challenge); err != nil {
		t.Fatal(err)
	}
	message, err := base64.StdEncoding.DecodeString(challenge.Message)
	if err != nil {
		t.Fatal(err)
	}
	registration := map[string]string{
		"challenge_id": challenge.ChallengeID,
		"signature":    base64.StdEncoding.EncodeToString(ed25519.Sign(privateKey, message)),
	}
	status, data = post("/v1/accounts", registration)
	if status != http.StatusCreated {
		t.Fatalf("registration status %d: %s", status, data)
	}
	var registered account.Account
	if err := json.Unmarshal(data, &registered); err != nil {
		t.Fatal(err)
	}
	id, err := strconv.ParseInt(registered.VoidID, 10, 64)
	if err != nil || id <= 0 || registered.Username != "http-integration" || registered.CreatedAt.IsZero() {
		t.Fatalf("invalid registration response: %s", data)
	}
	var responseFields map[string]json.RawMessage
	if err := json.Unmarshal(data, &responseFields); err != nil || len(responseFields) != 3 || responseFields["id"] != nil || responseFields["auth_public_key"] != nil {
		t.Fatal("registration exposed internal fields")
	}
	var username string
	if err := app.QueryRow(context.Background(), "SELECT username FROM public.accounts WHERE void_id = $1", id).Scan(&username); err != nil || username != registered.Username {
		t.Fatal("HTTP registration did not persist")
	}
	if status, _ := post("/v1/accounts", registration); status != http.StatusUnauthorized {
		t.Fatalf("replayed challenge returned status %d", status)
	}
	status, data = post("/v1/registration/challenges", map[string]string{
		"username": "same-key", "auth_public_key": base64.StdEncoding.EncodeToString(publicKey),
	})
	if status != http.StatusCreated {
		t.Fatalf("second challenge status %d", status)
	}
	if err := json.Unmarshal(data, &challenge); err != nil {
		t.Fatal(err)
	}
	message, err = base64.StdEncoding.DecodeString(challenge.Message)
	if err != nil {
		t.Fatal(err)
	}
	status, _ = post("/v1/accounts", map[string]string{
		"challenge_id": challenge.ChallengeID,
		"signature":    base64.StdEncoding.EncodeToString(ed25519.Sign(privateKey, message)),
	})
	if status != http.StatusConflict {
		t.Fatalf("fresh proof for registered public key returned status %d", status)
	}
	if admin != nil {
		var internalID string
		if err := admin.QueryRow(context.Background(), "SELECT id::text FROM public.accounts WHERE void_id = $1", id).Scan(&internalID); err != nil || len(internalID) != 36 || internalID[14] != '7' {
			t.Fatal("HTTP registration did not use PostgreSQL's UUIDv7 default")
		}
	}
}

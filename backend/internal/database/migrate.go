package database

import (
	"context"
	"crypto/sha256"
	"encoding/hex"
	"errors"
	"io/fs"
	"sort"
	"strconv"
	"strings"
	"time"

	"github.com/jackc/pgx/v5"
	"github.com/jackc/pgx/v5/pgxpool"
	"github.com/voidjack233/VOIDCHATS-2.0/backend/migrations"
)

const (
	migrationTimeout = 30 * time.Second
	migrationLockID  = int64(0x564f49444d494752) // "VOIDMIGR", scoped to this database.
)

var (
	// ErrMigrationFailed is safe to log; raw driver errors can contain secrets
	// or submitted account metadata and are deliberately not wrapped.
	ErrMigrationFailed = errors.New("database migration failed")

	// ErrMigrationHistory means an applied migration was edited or the database
	// includes versions unknown to this backend. No changes are committed.
	ErrMigrationHistory = errors.New("database migration history does not match this backend")

	// ErrSchemaNotReady tells callers to apply migrations before serving requests.
	ErrSchemaNotReady = errors.New("database schema is unavailable or incompatible; apply backend migrations")
)

type migration struct {
	version  int64
	name     string
	sql      string
	checksum string
}

// CheckSchema verifies migration history and the required account schema
// without applying SQL or changing the database. Startup must fail rather than
// falling back to memory when the configured schema is absent or incompatible.
func CheckSchema(ctx context.Context, pool *pgxpool.Pool) error {
	files, err := loadMigrations(migrations.Files)
	if err != nil {
		return ErrSchemaNotReady
	}
	ctx, cancel := context.WithTimeout(ctx, 5*time.Second)
	defer cancel()
	rows, err := pool.Query(ctx, "SELECT version, name, checksum FROM schema_migrations ORDER BY version")
	if err != nil {
		return ErrSchemaNotReady
	}
	applied := make(map[int64]migration)
	for rows.Next() {
		var entry migration
		if err := rows.Scan(&entry.version, &entry.name, &entry.checksum); err != nil {
			rows.Close()
			return ErrSchemaNotReady
		}
		applied[entry.version] = entry
	}
	rows.Close()
	if rows.Err() != nil || len(applied) != len(files) || validateHistory(files, applied) != nil {
		return ErrSchemaNotReady
	}
	var columnsOK, constraintsOK bool
	err = pool.QueryRow(ctx, `
		SELECT count(*) = 4 FROM information_schema.columns
		WHERE table_schema = current_schema() AND table_name = 'accounts'
		AND is_nullable = 'NO' AND (
			(column_name IN ('void_id', 'display_name', 'identity_public_key') AND data_type = 'text')
			OR (column_name = 'created_at' AND data_type = 'timestamp with time zone')
		)`).Scan(&columnsOK)
	if err != nil || !columnsOK {
		return ErrSchemaNotReady
	}
	err = pool.QueryRow(ctx, `
		SELECT
			EXISTS (
				SELECT 1 FROM pg_constraint
				WHERE conrelid = 'accounts'::regclass AND conname = 'accounts_void_id_key'
				AND contype = 'u' AND convalidated
				AND conkey = ARRAY[(SELECT attnum FROM pg_attribute WHERE attrelid = 'accounts'::regclass AND attname = 'void_id')]::smallint[]
			) AND EXISTS (
				SELECT 1 FROM pg_constraint
				WHERE conrelid = 'accounts'::regclass AND conname = 'accounts_void_id_length_check'
				AND contype = 'c' AND convalidated
				AND pg_get_constraintdef(oid) = 'CHECK ((octet_length(void_id) <= 512))'
			) AND EXISTS (
				SELECT 1 FROM pg_attribute
				WHERE attrelid = 'accounts'::regclass AND attname = 'void_id'
				AND attcollation = '"C"'::regcollation
			)`).Scan(&constraintsOK)
	if err != nil || !constraintsOK {
		return ErrSchemaNotReady
	}
	return nil
}

// Migrate applies embedded migrations once, in version order. An advisory
// transaction lock serializes concurrent runners, including the first run.
// Schema changes and history entries commit together. Applied file checksums
// prevent silently accepting changed SQL, and no down migrations are run.
func Migrate(ctx context.Context, pool *pgxpool.Pool) error {
	files, err := loadMigrations(migrations.Files)
	if err != nil {
		return ErrMigrationFailed
	}
	ctx, cancel := context.WithTimeout(ctx, migrationTimeout)
	defer cancel()
	tx, err := pool.BeginTx(ctx, pgx.TxOptions{})
	if err != nil {
		return migrationError(err)
	}
	defer func() {
		rollbackCtx, rollbackCancel := context.WithTimeout(context.Background(), 5*time.Second)
		defer rollbackCancel()
		_ = tx.Rollback(rollbackCtx)
	}()
	if _, err := tx.Exec(ctx, "SET LOCAL lock_timeout = '5s'"); err != nil {
		return migrationError(err)
	}
	if _, err := tx.Exec(ctx, "SET LOCAL statement_timeout = '25s'"); err != nil {
		return migrationError(err)
	}
	if _, err := tx.Exec(ctx, "SELECT pg_advisory_xact_lock($1)", migrationLockID); err != nil {
		return migrationError(err)
	}
	if _, err := tx.Exec(ctx, `
		CREATE TABLE IF NOT EXISTS schema_migrations (
			version bigint PRIMARY KEY,
			name text NOT NULL,
			checksum text NOT NULL,
			applied_at timestamptz NOT NULL DEFAULT now()
		)`); err != nil {
		return migrationError(err)
	}
	rows, err := tx.Query(ctx, "SELECT version, name, checksum FROM schema_migrations ORDER BY version")
	if err != nil {
		return migrationError(err)
	}
	applied := make(map[int64]migration)
	for rows.Next() {
		var entry migration
		if err := rows.Scan(&entry.version, &entry.name, &entry.checksum); err != nil {
			rows.Close()
			return migrationError(err)
		}
		applied[entry.version] = entry
	}
	rows.Close()
	if err := rows.Err(); err != nil {
		return migrationError(err)
	}
	if err := validateHistory(files, applied); err != nil {
		return err
	}
	for _, file := range files {
		if _, exists := applied[file.version]; exists {
			continue
		}
		if _, err := tx.Exec(ctx, file.sql); err != nil {
			return migrationError(err)
		}
		if _, err := tx.Exec(ctx, "INSERT INTO schema_migrations (version, name, checksum) VALUES ($1, $2, $3)",
			file.version, file.name, file.checksum); err != nil {
			return migrationError(err)
		}
	}
	if err := tx.Commit(ctx); err != nil {
		return migrationError(err)
	}
	return nil
}

func loadMigrations(files fs.FS) ([]migration, error) {
	entries, err := fs.ReadDir(files, ".")
	if err != nil {
		return nil, err
	}
	var result []migration
	seen := make(map[int64]bool)
	for _, entry := range entries {
		if entry.IsDir() || !strings.HasSuffix(entry.Name(), ".up.sql") {
			continue
		}
		versionText, _, ok := strings.Cut(entry.Name(), "_")
		version, err := strconv.ParseInt(versionText, 10, 64)
		if !ok || err != nil || version < 1 || seen[version] {
			return nil, ErrMigrationFailed
		}
		body, err := fs.ReadFile(files, entry.Name())
		if err != nil || strings.TrimSpace(string(body)) == "" {
			return nil, ErrMigrationFailed
		}
		digest := sha256.Sum256(body)
		result = append(result, migration{
			version: version, name: entry.Name(), sql: string(body), checksum: hex.EncodeToString(digest[:]),
		})
		seen[version] = true
	}
	if len(result) == 0 {
		return nil, ErrMigrationFailed
	}
	sort.Slice(result, func(i, j int) bool { return result[i].version < result[j].version })
	return result, nil
}

func validateHistory(files []migration, applied map[int64]migration) error {
	known := make(map[int64]migration, len(files))
	for _, file := range files {
		known[file.version] = file
	}
	for version, entry := range applied {
		file, exists := known[version]
		if !exists || file.name != entry.name || file.checksum != entry.checksum {
			return ErrMigrationHistory
		}
	}
	// A newer version cannot have been applied while an older embedded version
	// remains missing; applying old SQL to a newer schema is unsafe.
	missing := false
	for _, file := range files {
		_, exists := applied[file.version]
		if !exists {
			missing = true
		} else if missing {
			return ErrMigrationHistory
		}
	}
	return nil
}

func migrationError(err error) error {
	if errors.Is(err, context.Canceled) {
		return context.Canceled
	}
	if errors.Is(err, context.DeadlineExceeded) {
		return context.DeadlineExceeded
	}
	return ErrMigrationFailed
}

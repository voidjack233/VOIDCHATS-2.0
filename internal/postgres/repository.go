// Package postgres persists accounts in the existing public.accounts table.
package postgres

import (
	"context"
	"errors"
	"strconv"
	"strings"
	"sync"

	"github.com/jackc/pgx/v5"
	"github.com/jackc/pgx/v5/pgconn"
	"github.com/jackc/pgx/v5/pgxpool"

	"void-nothingness/internal/account"
)

// Validation errors contain only fixed application messages. PostgreSQL and
// connection errors may contain credentials or SQL and must never be retained.
var (
	ErrDatabaseNotFound     = errors.New("postgres: configured database does not exist; verify PGDATABASE or DATABASE_URL")
	ErrAuthenticationFailed = errors.New("postgres: authentication failed; verify configured credentials")
	ErrSchemaMismatch       = errors.New("postgres: public.accounts schema does not match required application structure")
	ErrRuntimeRole          = errors.New("postgres: runtime role is missing required application privileges")
)

type Repository struct {
	pool                        *pgxpool.Pool
	mu                          sync.RWMutex
	idConstraint, keyConstraint string
}

func New(pool *pgxpool.Pool) *Repository {
	return &Repository{pool: pool, idConstraint: "accounts_void_id_key", keyConstraint: "accounts_auth_public_key_key"}
}

func (r *Repository) Insert(ctx context.Context, voidID int64, publicKey []byte) (account.Account, error) {
	result, err := insertAccount(ctx, r.pool, voidID, publicKey)
	if err != nil {
		r.mu.RLock()
		idConstraint, keyConstraint := r.idConstraint, r.keyConstraint
		r.mu.RUnlock()
		return account.Account{}, classifyError(ctx, err, idConstraint, keyConstraint)
	}
	return result, nil
}

type accountWriter interface {
	QueryRow(context.Context, string, ...any) pgx.Row
}

func insertAccount(ctx context.Context, writer accountWriter, voidID int64, publicKey []byte) (account.Account, error) {
	var result account.Account
	var storedID int64
	err := writer.QueryRow(ctx, `
		INSERT INTO public.accounts (void_id, auth_public_key)
		VALUES ($1, $2)
		RETURNING void_id, created_at`, voidID, publicKey).
		Scan(&storedID, &result.CreatedAt)
	if err != nil {
		return account.Account{}, err
	}
	result.VoidID = strconv.FormatInt(storedID, 10)
	return result, nil
}

type accountColumn struct {
	typ, def string
	modifier int32
	notNull  bool
}

var expectedAccountColumns = map[string]accountColumn{
	"id":              {"uuid", "uuidv7()", -1, true},
	"void_id":         {"int8", "", -1, true},
	"username":        {"varchar", "", 132, false},
	"auth_public_key": {"bytea", "", -1, true},
	"created_at":      {"timestamptz", "now()", -1, true},
}

func validAccountColumn(name, typ, def string, modifier int32, notNull, generated bool) bool {
	want, exists := expectedAccountColumns[name]
	return exists && typ == want.typ && modifier == want.modifier && notNull == want.notNull && !generated && def == want.def
}

func classifyError(ctx context.Context, err error, idConstraint, keyConstraint string) error {
	if ctx.Err() != nil {
		return ctx.Err()
	}
	if errors.Is(err, context.Canceled) {
		return context.Canceled
	}
	if errors.Is(err, context.DeadlineExceeded) {
		return context.DeadlineExceeded
	}
	var databaseError *pgconn.PgError
	if errors.As(err, &databaseError) && databaseError.Code == "23505" && databaseError.SchemaName == "public" && databaseError.TableName == "accounts" {
		switch databaseError.ConstraintName {
		case idConstraint:
			return account.ErrIDCollision
		case keyConstraint:
			return account.ErrDuplicateKey
		}
	}
	// PostgreSQL diagnostics can contain public keys, SQL, or connection details.
	// Keep them out of both the returned error and its unwrap chain.
	return account.ErrUnavailable
}

func classifyValidationError(ctx context.Context, err error) error {
	if ctx.Err() != nil {
		return ctx.Err()
	}
	if errors.Is(err, context.Canceled) {
		return context.Canceled
	}
	if errors.Is(err, context.DeadlineExceeded) {
		return context.DeadlineExceeded
	}
	var databaseError *pgconn.PgError
	if errors.As(err, &databaseError) {
		switch databaseError.Code {
		case "3D000":
			return ErrDatabaseNotFound
		case "28P01", "28000":
			return ErrAuthenticationFailed
		case "42501":
			return ErrRuntimeRole
		}
	}
	return account.ErrUnavailable
}

// Ready checks connectivity, the account schema, and the effective runtime
// privileges. Catalog queries are read-only and never modify existing objects.
func (r *Repository) Ready(ctx context.Context) error { return r.Validate(ctx) }

// Validate checks the existing schema and access required by account insertion.
// Call it before accepting requests, and use Ready for subsequent health probes.
func (r *Repository) Validate(ctx context.Context) error {
	conn, err := r.pool.Acquire(ctx)
	if err != nil {
		return classifyValidationError(ctx, err)
	}
	defer conn.Release()
	rows, err := conn.Query(ctx, `
		SELECT a.attname, t.typname, a.atttypmod, a.attnotnull,
		       COALESCE(pg_catalog.pg_get_expr(d.adbin, d.adrelid), ''),
		       a.attidentity <> '' OR a.attgenerated <> '',
		       pg_catalog.has_column_privilege(c.oid, a.attnum, 'SELECT'),
		       pg_catalog.has_column_privilege(c.oid, a.attnum, 'INSERT')
		FROM pg_catalog.pg_attribute a
		JOIN pg_catalog.pg_class c ON c.oid = a.attrelid
		JOIN pg_catalog.pg_type t ON t.oid = a.atttypid
		LEFT JOIN pg_catalog.pg_attrdef d ON d.adrelid = a.attrelid AND d.adnum = a.attnum
		WHERE c.oid = pg_catalog.to_regclass('public.accounts') AND a.attnum > 0 AND NOT a.attisdropped`)
	if err != nil {
		return classifyValidationError(ctx, err)
	}
	var mismatch, privilegeMismatch bool
	count := 0
	for rows.Next() {
		var name, typ, def string
		var modifier int32
		var notNull, generated, selectable, insertable bool
		if err := rows.Scan(&name, &typ, &modifier, &notNull, &def, &generated, &selectable, &insertable); err != nil {
			rows.Close()
			return classifyValidationError(ctx, err)
		}
		_, exists := expectedAccountColumns[name]
		if !validAccountColumn(name, typ, def, modifier, notNull, generated) {
			mismatch = true
		}
		if exists && !hasRequiredColumnAccess(name, selectable, insertable) {
			privilegeMismatch = true
		}
		count++
	}
	err = rows.Err()
	rows.Close()
	if err != nil {
		return classifyValidationError(ctx, err)
	}
	if mismatch || count != len(expectedAccountColumns) {
		return ErrSchemaMismatch
	}
	if privilegeMismatch {
		return ErrRuntimeRole
	}

	rows, err = conn.Query(ctx, `
		SELECT c.conname, c.contype::text, c.convalidated, c.condeferrable,
		       ARRAY(SELECT a.attname::text FROM unnest(c.conkey) WITH ORDINALITY AS k(num, ord)
		             JOIN pg_catalog.pg_attribute a ON a.attrelid = c.conrelid AND a.attnum = k.num ORDER BY k.ord),
		       COALESCE(pg_catalog.pg_get_expr(c.conbin, c.conrelid), '')
		FROM pg_catalog.pg_constraint c
		WHERE c.conrelid = pg_catalog.to_regclass('public.accounts') AND c.contype IN ('p', 'u', 'c')`)
	if err != nil {
		return classifyValidationError(ctx, err)
	}
	checks := map[string]bool{
		"void_id>0": false,
		"char_lengthusername>=1andchar_lengthusername<=32andusername=btrimusername": false,
		"octet_lengthauth_public_key=32":                                            false,
	}
	var idConstraint, keyConstraint string
	var primary bool
	mismatch = false
	for rows.Next() {
		var name, kind, expression string
		var validated, deferrable bool
		var columns []string
		if err := rows.Scan(&name, &kind, &validated, &deferrable, &columns, &expression); err != nil {
			rows.Close()
			return classifyValidationError(ctx, err)
		}
		if !validated || deferrable {
			mismatch = true
		}
		if kind == "c" {
			check := normalizeCheck(expression)
			if seen, exists := checks[check]; !exists || seen {
				mismatch = true
			} else {
				checks[check] = true
			}
			continue
		}
		if len(columns) != 1 {
			mismatch = true
			continue
		}
		switch {
		case kind == "p" && columns[0] == "id" && !primary:
			primary = true
		case kind == "u" && columns[0] == "void_id" && idConstraint == "":
			idConstraint = name
		case kind == "u" && columns[0] == "auth_public_key" && keyConstraint == "":
			keyConstraint = name
		default:
			mismatch = true
		}
	}
	err = rows.Err()
	rows.Close()
	if err != nil {
		return classifyValidationError(ctx, err)
	}
	for _, present := range checks {
		mismatch = mismatch || !present
	}
	if mismatch || !primary || idConstraint == "" || keyConstraint == "" {
		return ErrSchemaMismatch
	}

	var compatibleTable bool
	err = conn.QueryRow(ctx, `
		SELECT c.relkind = 'r' AND c.relpersistence = 'p' AND NOT c.relrowsecurity
		FROM pg_catalog.pg_class c
		WHERE c.oid = pg_catalog.to_regclass('public.accounts')`).Scan(&compatibleTable)
	if err != nil {
		return classifyValidationError(ctx, err)
	}
	if !compatibleTable {
		return ErrSchemaMismatch
	}
	r.mu.Lock()
	r.idConstraint, r.keyConstraint = idConstraint, keyConstraint
	r.mu.Unlock()
	return nil
}

// Registration needs INSERT ... RETURNING, and returning-user authentication
// additionally reads auth_public_key by VOID-ID.
// Additional privileges, including those of the configured postgres login, do
// not prevent the application from using its existing database connection.
func hasRequiredColumnAccess(name string, selectable, insertable bool) bool {
	switch name {
	case "void_id", "auth_public_key":
		return selectable && insertable
	case "created_at":
		return selectable
	default:
		return true
	}
}

// PostgreSQL deparses the manually created checks with implicit text casts and
// extra parentheses. Ignore only that presentation when comparing the required
// expressions, and reject changed or additional checks.
func normalizeCheck(expression string) string {
	return strings.NewReplacer(" ", "", "\n", "", "\t", "", "(", "", ")", "", "::text", "").Replace(strings.ToLower(expression))
}

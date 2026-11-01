package postgres

import (
	"context"
	"crypto/sha256"
	"errors"
	"strconv"

	"github.com/jackc/pgx/v5"
	"github.com/jackc/pgx/v5/pgconn"
	"github.com/jackc/pgx/v5/pgxpool"
	"void-nothingness/internal/auth"
	"void-nothingness/internal/session"
)

type sessionDatabase interface {
	Begin(context.Context) (pgx.Tx, error)
	QueryRow(context.Context, string, ...any) pgx.Row
	Exec(context.Context, string, ...any) (pgconn.CommandTag, error)
}

// SessionStore uses only additive public.void_sessions schema objects. Creation
// and grants are owner-reviewed SQL; constructing this store never migrates.
type SessionStore struct{ db sessionDatabase }

func NewSessionStore(pool *pgxpool.Pool) *SessionStore { return &SessionStore{db: pool} }

// Persist serializes creation for one account, including across processes. It
// keeps at most maxPerAccount stored rows for that account by pruning its expired
// rows in the same transaction. A capacity denial never evicts active sessions.
func (s *SessionStore) Persist(ctx context.Context, record session.Record, maxPerAccount int) error {
	if !auth.ValidVoidID(record.VoidID) || maxPerAccount < 1 || !record.ExpiresAt.After(record.CreatedAt) {
		return session.ErrUnavailable
	}
	voidID, _ := strconv.ParseInt(record.VoidID, 10, 64)
	tx, err := s.db.Begin(ctx)
	if err != nil {
		return session.ErrUnavailable
	}
	defer tx.Rollback(ctx)
	// A namespaced bigint advisory lock avoids modifying or locking accounts.
	if _, err = tx.Exec(ctx, `SELECT pg_advisory_xact_lock(pg_catalog.hashtextextended('void_sessions:' || $1::bigint::text, 0))`, voidID); err != nil {
		return session.ErrUnavailable
	}
	if _, err = tx.Exec(ctx, `DELETE FROM public.void_sessions WHERE void_id = $1 AND expires_at <= $2`, voidID, record.CreatedAt); err != nil {
		return session.ErrUnavailable
	}
	var count int
	if err = tx.QueryRow(ctx, `SELECT count(*) FROM public.void_sessions WHERE void_id = $1`, voidID).Scan(&count); err != nil {
		return session.ErrUnavailable
	}
	if count >= maxPerAccount {
		return session.ErrCapacity
	}
	if _, err = tx.Exec(ctx, `INSERT INTO public.void_sessions (token_hash, void_id, created_at, expires_at) VALUES ($1, $2, $3, $4)`,
		record.TokenHash[:], voidID, record.CreatedAt, record.ExpiresAt); err != nil {
		return session.ErrUnavailable
	}
	if err = tx.Commit(ctx); err != nil {
		return session.ErrUnavailable
	}
	return nil
}

func (s *SessionStore) Find(ctx context.Context, hash [sha256.Size]byte) (session.Record, error) {
	var r session.Record
	var voidID int64
	err := s.db.QueryRow(ctx, `SELECT void_id, created_at, expires_at FROM public.void_sessions WHERE token_hash = $1`, hash[:]).Scan(&voidID, &r.CreatedAt, &r.ExpiresAt)
	if errors.Is(err, pgx.ErrNoRows) {
		return session.Record{}, session.ErrAuthentication
	}
	if err != nil {
		return session.Record{}, session.ErrUnavailable
	}
	r.TokenHash = hash
	r.VoidID = strconv.FormatInt(voidID, 10)
	return r, nil
}

func (s *SessionStore) Revoke(ctx context.Context, hash [sha256.Size]byte) error {
	if _, err := s.db.Exec(ctx, `DELETE FROM public.void_sessions WHERE token_hash = $1`, hash[:]); err != nil {
		return session.ErrUnavailable
	}
	return nil
}

// Validate checks the additive schema, constraints and all runtime privileges.
// Catalog inspection is read-only; the process never repairs or migrates schema.
func (s *SessionStore) Validate(ctx context.Context) error {
	var ready bool
	err := s.db.QueryRow(ctx, `
		SELECT COALESCE((
		  SELECT c.relkind = 'r' AND c.relpersistence = 'p' AND NOT c.relrowsecurity
		    AND pg_catalog.has_table_privilege(c.oid, 'SELECT')
		    AND pg_catalog.has_table_privilege(c.oid, 'INSERT')
		    AND pg_catalog.has_table_privilege(c.oid, 'DELETE')
		    AND (SELECT count(*) = 4 AND bool_and(a.attnotnull AND a.attidentity = '' AND a.attgenerated = '' AND NOT a.atthasdef
		      AND ((a.attname = 'token_hash' AND a.atttypid = 'bytea'::regtype)
		        OR (a.attname = 'void_id' AND a.atttypid = 'bigint'::regtype)
		        OR (a.attname IN ('created_at', 'expires_at') AND a.atttypid = 'timestamptz'::regtype)))
		      FROM pg_catalog.pg_attribute a WHERE a.attrelid = c.oid AND a.attnum > 0 AND NOT a.attisdropped)
		    AND (SELECT count(*) = 4 AND bool_and(k.convalidated AND NOT k.condeferrable)
		      FROM pg_catalog.pg_constraint k WHERE k.conrelid = c.oid AND k.contype IN ('p', 'u', 'c', 'f', 'x'))
		    AND EXISTS (SELECT 1 FROM pg_catalog.pg_constraint k
		      JOIN pg_catalog.pg_attribute a ON a.attrelid = c.oid AND a.attnum = k.conkey[1]
		      WHERE k.conrelid = c.oid AND k.contype = 'p' AND cardinality(k.conkey) = 1 AND a.attname = 'token_hash')
		    AND EXISTS (SELECT 1 FROM pg_catalog.pg_constraint k
		      JOIN pg_catalog.pg_attribute a ON a.attrelid = c.oid AND a.attnum = k.conkey[1]
		      JOIN pg_catalog.pg_attribute f ON f.attrelid = k.confrelid AND f.attnum = k.confkey[1]
		      WHERE k.conrelid = c.oid AND k.contype = 'f' AND cardinality(k.conkey) = 1 AND cardinality(k.confkey) = 1
		        AND a.attname = 'void_id' AND k.confrelid = 'public.accounts'::regclass AND f.attname = 'void_id'
		        AND k.confdeltype = 'a' AND k.confupdtype = 'a')
		    AND (SELECT count(*) = 2 AND count(DISTINCT regexp_replace(pg_catalog.pg_get_expr(k.conbin, k.conrelid), '[()[:space:]]', '', 'g')) = 2 AND bool_and(
		      regexp_replace(pg_catalog.pg_get_expr(k.conbin, k.conrelid), '[()[:space:]]', '', 'g')
		      IN ('octet_lengthtoken_hash=32', 'expires_at>created_at'))
		      FROM pg_catalog.pg_constraint k WHERE k.conrelid = c.oid AND k.contype = 'c')
		  FROM pg_catalog.pg_class c WHERE c.oid = pg_catalog.to_regclass('public.void_sessions')
		), false)`).Scan(&ready)
	if err != nil || !ready {
		return session.ErrUnavailable
	}
	return nil
}

func (s *SessionStore) Ready(ctx context.Context) error { return s.Validate(ctx) }

var _ session.Store = (*SessionStore)(nil)

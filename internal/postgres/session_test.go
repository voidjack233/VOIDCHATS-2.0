package postgres

import (
	"bytes"
	"context"
	"crypto/sha256"
	"errors"
	"strings"
	"testing"
	"time"

	"github.com/jackc/pgx/v5"
	"github.com/jackc/pgx/v5/pgconn"
	"void-nothingness/internal/session"
)

type sessionRow func(...any) error

func (f sessionRow) Scan(dest ...any) error { return f(dest...) }

type sessionTestDB struct {
	tx       *sessionTestTx
	beginErr error
	query    func(context.Context, string, ...any) pgx.Row
	exec     func(context.Context, string, ...any) (pgconn.CommandTag, error)
}

func (d sessionTestDB) Begin(context.Context) (pgx.Tx, error) { return d.tx, d.beginErr }
func (d sessionTestDB) QueryRow(ctx context.Context, q string, args ...any) pgx.Row {
	return d.query(ctx, q, args...)
}
func (d sessionTestDB) Exec(ctx context.Context, q string, args ...any) (pgconn.CommandTag, error) {
	return d.exec(ctx, q, args...)
}

type sessionTestTx struct {
	pgx.Tx
	queries               []string
	args                  [][]any
	count                 int
	execErr               error
	queryErr              error
	commitErr             error
	committed, rolledBack bool
}

func (tx *sessionTestTx) Exec(_ context.Context, q string, args ...any) (pgconn.CommandTag, error) {
	tx.queries = append(tx.queries, q)
	tx.args = append(tx.args, args)
	return pgconn.NewCommandTag("INSERT 0 1"), tx.execErr
}
func (tx *sessionTestTx) QueryRow(_ context.Context, q string, args ...any) pgx.Row {
	tx.queries = append(tx.queries, q)
	tx.args = append(tx.args, args)
	return sessionRow(func(dest ...any) error {
		if tx.queryErr != nil {
			return tx.queryErr
		}
		*dest[0].(*int) = tx.count
		return nil
	})
}
func (tx *sessionTestTx) Commit(context.Context) error {
	tx.committed = tx.commitErr == nil
	return tx.commitErr
}
func (tx *sessionTestTx) Rollback(context.Context) error { tx.rolledBack = true; return nil }

func storedSession() session.Record {
	now := time.Date(2026, 11, 2, 0, 0, 0, 0, time.UTC)
	return session.Record{TokenHash: sha256.Sum256([]byte("synthetic test token")), VoidID: "123", CreatedAt: now, ExpiresAt: now.Add(session.TokenTTL)}
}

func TestSessionPersistSerializesCountPruneAndInsert(t *testing.T) {
	tx := &sessionTestTx{count: 1}
	store := &SessionStore{db: sessionTestDB{tx: tx}}
	r := storedSession()
	if err := store.Persist(context.Background(), r, 8); err != nil {
		t.Fatal(err)
	}
	if !tx.committed || !tx.rolledBack || len(tx.queries) != 4 {
		t.Fatal("session creation was not transactional")
	}
	if !strings.Contains(tx.queries[0], "pg_advisory_xact_lock") || !strings.Contains(tx.queries[0], "void_sessions:") || len(tx.args[0]) != 1 || tx.args[0][0] != int64(123) {
		t.Fatal("per-account lock missing or unbound")
	}
	if !strings.HasPrefix(tx.queries[1], "DELETE") || tx.args[1][0] != int64(123) || tx.args[1][1] != r.CreatedAt {
		t.Fatal("expiry pruning not scoped to account and issuance time")
	}
	if !strings.HasPrefix(tx.queries[2], "SELECT count(*)") || tx.args[2][0] != int64(123) {
		t.Fatal("capacity count not scoped to account")
	}
	if !strings.HasPrefix(tx.queries[3], "INSERT") || !bytes.Equal(tx.args[3][0].([]byte), r.TokenHash[:]) || tx.args[3][1] != int64(123) || tx.args[3][2] != r.CreatedAt || tx.args[3][3] != r.ExpiresAt {
		t.Fatal("session row not bound to hash/account/times")
	}
}

func TestSessionPersistCapacityAndDatabaseFailureRollback(t *testing.T) {
	secret := errors.New("secret database password and SQL")
	for _, tt := range []struct {
		name     string
		tx       *sessionTestTx
		beginErr error
		want     error
	}{
		{"capacity", &sessionTestTx{count: 8}, nil, session.ErrCapacity},
		{"begin", &sessionTestTx{}, secret, session.ErrUnavailable},
		{"exec", &sessionTestTx{execErr: secret}, nil, session.ErrUnavailable},
		{"count", &sessionTestTx{queryErr: secret}, nil, session.ErrUnavailable},
		{"commit", &sessionTestTx{commitErr: secret}, nil, session.ErrUnavailable},
	} {
		t.Run(tt.name, func(t *testing.T) {
			store := &SessionStore{db: sessionTestDB{tx: tt.tx, beginErr: tt.beginErr}}
			err := store.Persist(context.Background(), storedSession(), 8)
			if err != tt.want || errors.Unwrap(err) != nil {
				t.Fatalf("unsafe or wrong error: %v", err)
			}
			if tt.tx.committed {
				t.Fatal("failed session transaction committed")
			}
			if tt.beginErr == nil && !tt.tx.rolledBack {
				t.Fatal("failed session transaction did not rollback")
			}
			if tt.name == "capacity" && len(tt.tx.queries) != 3 {
				t.Fatal("capacity denial inserted a session")
			}
		})
	}
}

func TestSessionFindRevokeAndValidateUseBoundHashesAndSafeErrors(t *testing.T) {
	r := storedSession()
	secret := errors.New("secret connection password")
	for _, fault := range []error{nil, pgx.ErrNoRows, secret} {
		db := sessionTestDB{query: func(_ context.Context, q string, args ...any) pgx.Row {
			if q != `SELECT void_id, created_at, expires_at FROM public.void_sessions WHERE token_hash = $1` || len(args) != 1 || !bytes.Equal(args[0].([]byte), r.TokenHash[:]) {
				t.Fatal("find did not bind token hash")
			}
			return sessionRow(func(dest ...any) error {
				if fault != nil {
					return fault
				}
				*dest[0].(*int64) = 123
				*dest[1].(*time.Time) = r.CreatedAt
				*dest[2].(*time.Time) = r.ExpiresAt
				return nil
			})
		}}
		got, err := (&SessionStore{db: db}).Find(context.Background(), r.TokenHash)
		switch fault {
		case nil:
			if err != nil || got != r {
				t.Fatal("session lookup result mismatch")
			}
		case pgx.ErrNoRows:
			if err != session.ErrAuthentication {
				t.Fatal("unknown token accepted")
			}
		default:
			if err != session.ErrUnavailable || errors.Unwrap(err) != nil {
				t.Fatal("lookup error leaked")
			}
		}
	}
	for _, fault := range []error{nil, secret} {
		db := sessionTestDB{exec: func(_ context.Context, q string, args ...any) (pgconn.CommandTag, error) {
			if q != `DELETE FROM public.void_sessions WHERE token_hash = $1` || len(args) != 1 || !bytes.Equal(args[0].([]byte), r.TokenHash[:]) {
				t.Fatal("revoke did not bind token hash")
			}
			return pgconn.NewCommandTag("DELETE 0"), fault
		}}
		err := (&SessionStore{db: db}).Revoke(context.Background(), r.TokenHash)
		if (fault == nil && err != nil) || (fault != nil && err != session.ErrUnavailable) {
			t.Fatal("revocation error classification mismatch")
		}
	}
	for _, ready := range []bool{false, true} {
		db := sessionTestDB{query: func(_ context.Context, q string, args ...any) pgx.Row {
			for _, priv := range []string{"SELECT", "INSERT", "DELETE"} {
				if !strings.Contains(q, "has_table_privilege(c.oid, '"+priv+"')") {
					t.Fatal("missing runtime privilege validation")
				}
			}
			if !strings.Contains(q, "pg_catalog.pg_constraint") || !strings.Contains(q, "pg_catalog.pg_attribute") {
				t.Fatal("schema validation missing")
			}
			return sessionRow(func(dest ...any) error { *dest[0].(*bool) = ready; return nil })
		}}
		err := (&SessionStore{db: db}).Validate(context.Background())
		if (ready && err != nil) || (!ready && err != session.ErrUnavailable) {
			t.Fatal("schema validation readiness mismatch")
		}
	}
}

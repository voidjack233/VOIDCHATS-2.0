package postgres

import (
	"bytes"
	"context"
	"errors"
	"strings"
	"testing"

	"github.com/jackc/pgx/v5"
	"github.com/jackc/pgx/v5/pgconn"
	"void-nothingness/internal/auth"
)

type authenticationRow struct {
	key []byte
	err error
}

func (r authenticationRow) Scan(dest ...any) error {
	if r.err == nil {
		*dest[0].(*[]byte) = r.key
	}
	return r.err
}

type authenticationQuery func(context.Context, string, ...any) pgx.Row

func (q authenticationQuery) QueryRow(ctx context.Context, sql string, args ...any) pgx.Row {
	return q(ctx, sql, args...)
}

func TestAuthenticationKeyUsesBoundIDAndCopiesResult(t *testing.T) {
	key := bytes.Repeat([]byte{42}, 32)
	ctx := context.WithValue(context.Background(), struct{}{}, "lookup")
	reader := authenticationQuery(func(got context.Context, sql string, args ...any) pgx.Row {
		if got != ctx || sql != `SELECT auth_public_key FROM public.accounts WHERE void_id = $1` || len(args) != 1 || args[0] != int64(123) {
			t.Fatal("authentication lookup did not use the request context and bound VOID-ID")
		}
		return authenticationRow{key: key}
	})
	got, err := authenticationKey(ctx, reader, 123)
	if err != nil || !bytes.Equal(got, key) {
		t.Fatalf("lookup failed: %v", err)
	}
	got[0] = 1
	if key[0] != 42 {
		t.Fatal("returned key aliases query storage")
	}
}

func TestAuthenticationLookupErrorsDoNotExposeDatabaseDiagnostics(t *testing.T) {
	secret := &pgconn.PgError{Code: "42501", Message: "secret SQL and password", Detail: "secret key"}
	for _, tt := range []struct {
		name string
		err  error
		want error
	}{
		{"unknown ID", pgx.ErrNoRows, auth.ErrNotFound},
		{"permission denied", secret, auth.ErrUnavailable},
		{"connection failure", errors.New("secret connection string"), auth.ErrUnavailable},
		{"canceled", context.Canceled, context.Canceled},
		{"deadline", context.DeadlineExceeded, context.DeadlineExceeded},
	} {
		t.Run(tt.name, func(t *testing.T) {
			reader := authenticationQuery(func(context.Context, string, ...any) pgx.Row {
				return authenticationRow{err: tt.err}
			})
			key, err := authenticationKey(context.Background(), reader, 1)
			var databaseError *pgconn.PgError
			if key != nil || err != tt.want || strings.Contains(err.Error(), "secret") || errors.As(err, &databaseError) || errors.Unwrap(err) != nil {
				t.Fatalf("unsafe authentication lookup error: %v", err)
			}
		})
	}
}

func TestInvalidOrCanceledAuthenticationLookupDoesNotQuery(t *testing.T) {
	reader := authenticationQuery(func(context.Context, string, ...any) pgx.Row {
		t.Fatal("invalid/canceled lookup reached PostgreSQL")
		return authenticationRow{}
	})
	for _, id := range []int64{0, -1} {
		if _, err := authenticationKey(context.Background(), reader, id); err != auth.ErrNotFound {
			t.Fatal("invalid ID was accepted")
		}
	}
	ctx, cancel := context.WithCancel(context.Background())
	cancel()
	if _, err := authenticationKey(ctx, reader, 1); err != context.Canceled {
		t.Fatal("lookup ignored cancellation")
	}
}

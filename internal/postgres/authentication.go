package postgres

import (
	"context"
	"errors"

	"github.com/jackc/pgx/v5"
	"void-nothingness/internal/auth"
)

type authenticationReader interface {
	QueryRow(context.Context, string, ...any) pgx.Row
}

// AuthenticationKey reads the existing account's public key. It neither creates
// accounts nor changes database objects or access privileges.
func (r *Repository) AuthenticationKey(ctx context.Context, voidID int64) ([]byte, error) {
	return authenticationKey(ctx, r.pool, voidID)
}

func authenticationKey(ctx context.Context, reader authenticationReader, voidID int64) ([]byte, error) {
	if err := ctx.Err(); err != nil {
		return nil, err
	}
	if voidID <= 0 {
		return nil, auth.ErrNotFound
	}
	var key []byte
	err := reader.QueryRow(ctx, `SELECT auth_public_key FROM public.accounts WHERE void_id = $1`, voidID).Scan(&key)
	if err != nil {
		if ctx.Err() != nil {
			return nil, ctx.Err()
		}
		switch {
		case errors.Is(err, pgx.ErrNoRows):
			return nil, auth.ErrNotFound
		case errors.Is(err, context.Canceled):
			return nil, context.Canceled
		case errors.Is(err, context.DeadlineExceeded):
			return nil, context.DeadlineExceeded
		default:
			// Diagnostics may contain SQL, keys or connection secrets. Never
			// return the database error or retain it in an unwrap chain.
			return nil, auth.ErrUnavailable
		}
	}
	return append([]byte(nil), key...), nil
}

package postgres

import (
	"context"
	"database/sql"
	"errors"
	"time"

	"github.com/voidjack233/VOIDCHATS-2.0/backend/internal/api"
)

type Accounts struct { db *sql.DB }

func New(db *sql.DB) *Accounts { return &Accounts{db: db} }

func (a *Accounts) Ping(ctx context.Context) error {
	return a.db.PingContext(ctx)
}

// A unique index on void_id and auth_public_key enforces ownership and collision
// rejection atomically, even with concurrent registration requests.
func (a *Accounts) CreateAccount(ctx context.Context, input api.Account) (time.Time, error) {
	var created time.Time
	err := a.db.QueryRowContext(ctx, `
		INSERT INTO accounts (void_id, username, auth_public_key)
		VALUES ($1, $2, $3)
		ON CONFLICT DO NOTHING
		RETURNING created_at
	`, input.VoidID, input.Username, input.AuthPublicKey).Scan(&created)
	if errors.Is(err, sql.ErrNoRows) { return time.Time{}, api.ErrConflict }
	return created, err
}

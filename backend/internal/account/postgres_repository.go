package account

import (
	"context"
	"errors"
	"time"

	"github.com/jackc/pgx/v5"
	"github.com/jackc/pgx/v5/pgconn"
	"github.com/jackc/pgx/v5/pgxpool"
)

const postgresOperationTimeout = 5 * time.Second

// Do not return raw PostgreSQL errors: their detail can include submitted IDs
// or key data. Context errors and the domain sentinels remain distinguishable.
var errPostgresOperation = errors.New("account storage operation failed")

// PostgresRepository stores public account metadata in PostgreSQL. The caller
// owns the bounded pool and must keep it open while this repository is in use.
type PostgresRepository struct {
	pool *pgxpool.Pool
}

var _ Repository = (*PostgresRepository)(nil)

// NewPostgresRepository returns a repository using an existing database pool.
func NewPostgresRepository(pool *pgxpool.Pool) *PostgresRepository {
	return &PostgresRepository{pool: pool}
}

// Create inserts without an upsert so conflicting identifiers cannot replace
// an existing account. PostgreSQL enforces uniqueness atomically.
func (r *PostgresRepository) Create(ctx context.Context, account Account) error {
	ctx, cancel := context.WithTimeout(ctx, postgresOperationTimeout)
	defer cancel()
	_, err := r.pool.Exec(ctx, `
		INSERT INTO accounts (void_id, display_name, identity_public_key, created_at)
		VALUES ($1, $2, $3, $4)`,
		account.VOIDID, account.DisplayName, account.IdentityPublicKey, account.CreatedAt.UTC())
	if err != nil {
		return postgresCreateError(err)
	}
	return nil
}

// GetByVOIDID returns the account with an exact, case-sensitive identifier.
func (r *PostgresRepository) GetByVOIDID(ctx context.Context, id string) (Account, error) {
	ctx, cancel := context.WithTimeout(ctx, postgresOperationTimeout)
	defer cancel()
	var account Account
	err := r.pool.QueryRow(ctx, `
		SELECT void_id, display_name, identity_public_key, created_at
		FROM accounts WHERE void_id = $1`, id).
		Scan(&account.VOIDID, &account.DisplayName, &account.IdentityPublicKey, &account.CreatedAt)
	if errors.Is(err, pgx.ErrNoRows) {
		return Account{}, ErrNotFound
	}
	if err != nil {
		return Account{}, postgresOperationError(err)
	}
	account.CreatedAt = account.CreatedAt.UTC()
	return account, nil
}

func postgresCreateError(err error) error {
	var pgError *pgconn.PgError
	if errors.As(err, &pgError) && pgError.Code == "23505" && pgError.ConstraintName == "accounts_void_id_key" {
		return ErrAlreadyExists
	}
	return postgresOperationError(err)
}

func postgresOperationError(err error) error {
	if errors.Is(err, context.Canceled) {
		return context.Canceled
	}
	if errors.Is(err, context.DeadlineExceeded) {
		return context.DeadlineExceeded
	}
	return errPostgresOperation
}

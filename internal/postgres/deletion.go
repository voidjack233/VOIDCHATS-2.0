package postgres

import (
	"context"
	"errors"
	"slices"
	"time"

	"github.com/jackc/pgx/v5"
	"void-nothingness/internal/account"
	"void-nothingness/internal/queue"
)

// All lifecycle and account operations use the queue's injective pair-of-int32
// advisory lock space. Acquire in numeric order, before reading account state.
func lockAccounts(ctx context.Context, tx pgx.Tx, ids ...string) ([]int64, error) {
	numbers := make([]int64, len(ids))
	for i, id := range ids {
		numbers[i] = queueNumber(id)
		if numbers[i] <= 0 {
			return nil, account.ErrInvalidInput
		}
	}
	slices.Sort(numbers)
	numbers = slices.Compact(numbers)
	for _, id := range numbers {
		if _, err := tx.Exec(ctx, `SELECT pg_advisory_xact_lock($1::integer,$2::integer)`, int32(id>>32), int32(id)); err != nil {
			return nil, err
		}
	}
	return numbers, nil
}

func lockActiveAccounts(ctx context.Context, tx pgx.Tx, ids ...string) error {
	numbers, err := lockAccounts(ctx, tx, ids...)
	if err != nil {
		return err
	}
	rows, err := tx.Query(ctx, `SELECT void_id,deleted_at FROM public.accounts WHERE void_id=ANY($1::bigint[]) ORDER BY void_id`, numbers)
	if err != nil {
		return err
	}
	defer rows.Close()
	found := 0
	for rows.Next() {
		var id int64
		var deleted *time.Time
		if err := rows.Scan(&id, &deleted); err != nil {
			return err
		}
		if deleted != nil {
			return account.ErrDeleted
		}
		found++
	}
	if err := rows.Err(); err != nil {
		return err
	}
	if found != len(numbers) {
		return queue.ErrNotFound
	}
	return nil
}

func lifecycleError(ctx context.Context, err error) error {
	if err == nil {
		return nil
	}
	if ctx.Err() != nil {
		return ctx.Err()
	}
	for _, known := range []error{account.ErrDeleted, account.ErrInvalidInput, account.ErrInvalidSignature} {
		if errors.Is(err, known) {
			return known
		}
	}
	if errors.Is(err, queue.ErrNotFound) {
		return queue.ErrNotFound
	}
	return account.ErrUnavailable
}

// DeletionKey is used only for dedicated deletion proofs, including recovery.
// Normal authentication and peer lookup never return a deleted identity's key.
func (r *Repository) DeletionKey(ctx context.Context, id string) ([]byte, error) {
	var key []byte
	err := r.pool.QueryRow(ctx, `SELECT auth_public_key FROM public.accounts WHERE void_id=$1`, queueNumber(id)).Scan(&key)
	if errors.Is(err, pgx.ErrNoRows) {
		return nil, account.ErrInvalidSignature
	}
	return key, lifecycleError(ctx, err)
}

func (r *Repository) Delete(ctx context.Context, id string) (account.Status, error) {
	return deleteAccount(ctx, r.pool, id)
}

func deleteAccount(ctx context.Context, db queueDB, id string) (account.Status, error) {
	tx, err := db.BeginTx(ctx, pgx.TxOptions{IsoLevel: pgx.ReadCommitted})
	if err != nil {
		return account.Status{}, lifecycleError(ctx, err)
	}
	defer rollbackTx(tx)
	if _, err = lockAccounts(ctx, tx, id); err != nil {
		return account.Status{}, lifecycleError(ctx, err)
	}
	var deleted *time.Time
	if err = tx.QueryRow(ctx, `SELECT deleted_at FROM public.accounts WHERE void_id=$1`, queueNumber(id)).Scan(&deleted); err != nil {
		return account.Status{}, lifecycleError(ctx, err)
	}
	if deleted == nil {
		// The server database clock is read after obtaining the lifecycle lock.
		if err = tx.QueryRow(ctx, `UPDATE public.accounts SET deleted_at=clock_timestamp() WHERE void_id=$1 RETURNING deleted_at`, queueNumber(id)).Scan(&deleted); err != nil {
			return account.Status{}, lifecycleError(ctx, err)
		}
		if _, err = tx.Exec(ctx, `DELETE FROM public.messages WHERE sender_void_id=$1 OR recipient_void_id=$1`, queueNumber(id)); err != nil {
			return account.Status{}, lifecycleError(ctx, err)
		}
		var prekeys bool
		if err = tx.QueryRow(ctx, `SELECT to_regclass('public.signal_prekey_inventory') IS NOT NULL AND to_regclass('public.signal_prekey_claims') IS NOT NULL`).Scan(&prekeys); err != nil {
			return account.Status{}, lifecycleError(ctx, err)
		}
		if prekeys {
			for _, sql := range []string{
				`DELETE FROM public.signal_prekey_claims WHERE requester_void_id=$1`,
				`UPDATE public.signal_prekey_claims SET recipient_void_id=NULL,public_bundle=NULL WHERE recipient_void_id=$1`,
				`DELETE FROM public.signal_prekey_inventory WHERE void_id=$1`,
			} {
				if _, err = tx.Exec(ctx, sql, queueNumber(id)); err != nil {
					return account.Status{}, lifecycleError(ctx, err)
				}
			}
		}
	}
	if err = tx.Commit(ctx); err != nil {
		return account.Status{}, lifecycleError(ctx, err)
	}
	stamp := deleted.UTC()
	return account.Status{VoidID: id, State: "deleted", DeletedAt: &stamp}, nil
}

func (r *Repository) WithActive(ctx context.Context, ids []string, fn func() error) error {
	tx, err := r.pool.BeginTx(ctx, pgx.TxOptions{IsoLevel: pgx.ReadCommitted})
	if err != nil {
		return lifecycleError(ctx, err)
	}
	defer rollbackTx(tx)
	if err = lockActiveAccounts(ctx, tx, ids...); err != nil {
		return lifecycleError(ctx, err)
	}
	if err = fn(); err != nil {
		return lifecycleError(ctx, err)
	}
	return lifecycleError(ctx, tx.Commit(ctx))
}

func (r *Repository) Status(ctx context.Context, actor, target string) (account.Status, error) {
	tx, err := r.pool.BeginTx(ctx, pgx.TxOptions{IsoLevel: pgx.ReadCommitted})
	if err != nil {
		return account.Status{}, lifecycleError(ctx, err)
	}
	defer rollbackTx(tx)
	if err = lockActiveAccounts(ctx, tx, actor); err != nil {
		return account.Status{}, lifecycleError(ctx, err)
	}
	var deleted *time.Time
	err = tx.QueryRow(ctx, `SELECT deleted_at FROM public.accounts WHERE void_id=$1`, queueNumber(target)).Scan(&deleted)
	result := account.Status{VoidID: target, State: "active"}
	if errors.Is(err, pgx.ErrNoRows) {
		result.State = "unknown"
	} else if err != nil {
		return account.Status{}, lifecycleError(ctx, err)
	} else if deleted != nil {
		stamp := deleted.UTC()
		result.State, result.DeletedAt = "deleted", &stamp
	}
	return result, lifecycleError(ctx, tx.Commit(ctx))
}

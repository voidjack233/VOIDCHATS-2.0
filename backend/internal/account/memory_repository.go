package account

import (
	"context"
	"sync"
)

// MemoryRepository is temporary process-local account storage. It supports
// concurrent operations and loses its contents when the process ends.
// Its zero value is ready to use; it must not be copied after first use.
type MemoryRepository struct {
	mu       sync.RWMutex
	accounts map[string]Account
}

// NewMemoryRepository returns an empty, concurrency-safe repository.
func NewMemoryRepository() *MemoryRepository {
	return &MemoryRepository{accounts: make(map[string]Account)}
}

// Create stores an account if its VOID-ID is unused. Checking for an existing
// account and inserting the new account happen under the same lock.
func (r *MemoryRepository) Create(ctx context.Context, account Account) error {
	if err := ctx.Err(); err != nil {
		return err
	}
	r.mu.Lock()
	defer r.mu.Unlock()
	if err := ctx.Err(); err != nil {
		return err
	}
	if _, exists := r.accounts[account.VOIDID]; exists {
		return ErrAlreadyExists
	}
	if r.accounts == nil {
		r.accounts = make(map[string]Account)
	}
	r.accounts[account.VOIDID] = account
	return nil
}

// GetByVOIDID returns a copy of the stored account.
func (r *MemoryRepository) GetByVOIDID(ctx context.Context, id string) (Account, error) {
	if err := ctx.Err(); err != nil {
		return Account{}, err
	}
	r.mu.RLock()
	defer r.mu.RUnlock()
	if err := ctx.Err(); err != nil {
		return Account{}, err
	}
	account, exists := r.accounts[id]
	if !exists {
		return Account{}, ErrNotFound
	}
	return account, nil
}

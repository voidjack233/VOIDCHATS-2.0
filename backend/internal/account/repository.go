package account

import (
	"context"
	"errors"
)

var (
	// ErrAlreadyExists reports a duplicate VOID-ID. Existing account data must
	// be preserved when creation returns this error.
	ErrAlreadyExists = errors.New("void_id already exists")

	// ErrNotFound reports that no account has the requested VOID-ID.
	ErrNotFound = errors.New("account not found")
)

// Repository stores and retrieves accounts without exposing storage details.
// Create must enforce unique VOID-IDs atomically and never overwrite a
// previously stored account.
type Repository interface {
	Create(ctx context.Context, account Account) error
	GetByVOIDID(ctx context.Context, id string) (Account, error)
}

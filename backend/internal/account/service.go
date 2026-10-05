package account

import (
	"context"
	"errors"
	"fmt"
	"strings"
	"time"

	"github.com/voidjack233/VOIDCHATS-2.0/backend/internal/voidid"
)

// ErrInvalidAccount reports missing or invalid required account fields.
var ErrInvalidAccount = errors.New("invalid account")

// Service validates account requests independently of the storage mechanism.
type Service struct {
	repository Repository
}

// NewService returns an account service backed by repository.
func NewService(repository Repository) *Service {
	return &Service{repository: repository}
}

// Create registers the public identity submitted by the client. Validation
// currently requires nonblank fields without selecting an ID format or public
// key encoding. Submitted strings are preserved exactly as provided.
func (s *Service) Create(ctx context.Context, voidID, displayName, identityPublicKey string) (Account, error) {
	if err := ctx.Err(); err != nil {
		return Account{}, err
	}
	if err := voidid.ValidateSubmitted(voidID); err != nil {
		return Account{}, fmt.Errorf("%w: void_id is invalid", ErrInvalidAccount)
	}
	if strings.TrimSpace(displayName) == "" {
		return Account{}, fmt.Errorf("%w: display_name is required", ErrInvalidAccount)
	}
	if strings.TrimSpace(identityPublicKey) == "" {
		return Account{}, fmt.Errorf("%w: identity_public_key is required", ErrInvalidAccount)
	}

	account := Account{
		VOIDID:            voidID,
		DisplayName:       displayName,
		IdentityPublicKey: identityPublicKey,
		CreatedAt:         time.Now().UTC(),
	}
	if err := s.repository.Create(ctx, account); err != nil {
		return Account{}, err
	}
	return account, nil
}

// GetByVOIDID returns the account with the exact submitted identifier.
func (s *Service) GetByVOIDID(ctx context.Context, id string) (Account, error) {
	if err := ctx.Err(); err != nil {
		return Account{}, err
	}
	if err := voidid.ValidateSubmitted(id); err != nil {
		return Account{}, fmt.Errorf("%w: void_id is invalid", ErrInvalidAccount)
	}
	return s.repository.GetByVOIDID(ctx, id)
}

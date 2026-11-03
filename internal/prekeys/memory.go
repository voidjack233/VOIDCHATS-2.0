package prekeys

import (
	"context"
	"sync"
	"time"
)

// MemoryStore is a concurrency-safe synthetic implementation for local tests.
// Its registered account list is independent of public prekey availability.
type MemoryStore struct {
	mu          sync.Mutex
	accounts    map[string]bool
	inventories map[string]Inventory
	claims      map[string]memoryClaim
	claimCounts map[string]int
}

type memoryClaim struct {
	recipient string
	bundle    *Bundle
	expiresAt time.Time
}

func NewMemoryStore(accounts []string) *MemoryStore {
	store := &MemoryStore{
		accounts: map[string]bool{}, inventories: map[string]Inventory{},
		claims: map[string]memoryClaim{}, claimCounts: map[string]int{},
	}
	for _, id := range accounts {
		store.accounts[id] = true
	}
	return store
}

func (s *MemoryStore) Validate(ctx context.Context) error { return ctx.Err() }

func (s *MemoryStore) Publish(ctx context.Context, actor, publicationID string, upload Upload, hash [32]byte, now time.Time) (Counts, error) {
	s.mu.Lock()
	defer s.mu.Unlock()
	if err := ctx.Err(); err != nil {
		return Counts{}, err
	}
	if !s.accounts[actor] {
		return Counts{}, ErrInvalid
	}
	var current *Inventory
	if existing, exists := s.inventories[actor]; exists {
		current = &existing
	} else if len(s.inventories) >= MaxInventories {
		return Counts{}, ErrCapacity
	}
	next, counts, err := ApplyPublication(current, publicationID, upload, hash, now)
	if err != nil {
		return Counts{}, err
	}
	s.inventories[actor] = next
	return counts, nil
}

func (s *MemoryStore) Claim(ctx context.Context, actor, requestID, target string, now time.Time) (Bundle, error) {
	s.mu.Lock()
	defer s.mu.Unlock()
	if err := ctx.Err(); err != nil {
		return Bundle{}, err
	}
	if !s.accounts[actor] || actor == target {
		return Bundle{}, ErrInvalid
	}
	key := actor + ":" + requestID
	if claim, exists := s.claims[key]; exists {
		if !now.Before(claim.expiresAt) || claim.bundle == nil {
			return Bundle{}, ErrExpired
		}
		if claim.recipient != target {
			return Bundle{}, ErrConflict
		}
		return cloneBundle(*claim.bundle), nil
	}
	if !s.accounts[target] {
		return Bundle{}, ErrInvalid
	}
	if s.claimCounts[actor] >= MaxClaimsPerAccount || len(s.claims) >= MaxClaimsGlobal {
		return Bundle{}, ErrCapacity
	}
	inventory, exists := s.inventories[target]
	if !exists {
		return Bundle{}, ErrUnavailable
	}
	next, bundle, err := SelectBundle(inventory, now)
	if err != nil {
		return Bundle{}, err
	}
	s.inventories[target] = next
	stored := cloneBundle(bundle)
	s.claims[key] = memoryClaim{recipient: target, bundle: &stored, expiresAt: bundle.ClaimExpiresAt}
	s.claimCounts[actor]++
	return bundle, nil
}

func (s *MemoryStore) Expire(ctx context.Context, now time.Time, limit int) (int64, error) {
	s.mu.Lock()
	defer s.mu.Unlock()
	if err := ctx.Err(); err != nil {
		return 0, err
	}
	var count int64
	for key, claim := range s.claims {
		if count >= int64(limit) {
			break
		}
		if claim.bundle != nil && !now.Before(claim.expiresAt) {
			claim.bundle = nil
			claim.recipient = ""
			s.claims[key] = claim
			count++
		}
	}
	return count, nil
}

func cloneBundle(bundle Bundle) Bundle {
	if bundle.OneTimePrekey != nil {
		key := *bundle.OneTimePrekey
		bundle.OneTimePrekey = &key
	}
	return bundle
}

var _ Store = (*MemoryStore)(nil)

package account

import (
	"context"
	"errors"
	"fmt"
	"sync"
	"testing"
	"time"
)

func TestMemoryRepositoryConcurrentDuplicateCreation(t *testing.T) {
	repository := NewMemoryRepository()
	const attempts = 100
	const id = "same-client-generated-id"
	var wg sync.WaitGroup
	start := make(chan struct{})
	winner := make(chan Account, attempts)
	results := make(chan error, attempts)
	for i := 0; i < attempts; i++ {
		candidate := Account{
			VOIDID:            id,
			DisplayName:       fmt.Sprintf("display-%d", i),
			IdentityPublicKey: fmt.Sprintf("public-key-%d", i),
			CreatedAt:         time.Unix(int64(i), 0).UTC(),
		}
		wg.Add(1)
		go func() {
			defer wg.Done()
			<-start
			err := repository.Create(context.Background(), candidate)
			if err == nil {
				winner <- candidate
			}
			results <- err
		}()
	}
	close(start)
	wg.Wait()
	close(winner)
	close(results)
	successes, duplicates := 0, 0
	for err := range results {
		switch {
		case err == nil:
			successes++
		case errors.Is(err, ErrAlreadyExists):
			duplicates++
		default:
			t.Fatalf("unexpected Create() error: %v", err)
		}
	}
	if successes != 1 || duplicates != attempts-1 {
		t.Fatalf("concurrent results = %d successes, %d duplicates; want 1, %d", successes, duplicates, attempts-1)
	}
	expected := <-winner
	stored, err := repository.GetByVOIDID(context.Background(), id)
	if err != nil || stored != expected {
		t.Fatalf("stored account = %+v, %v; want winning account %+v", stored, err, expected)
	}
}

func TestMemoryRepositoryConcurrentIndependentAccounts(t *testing.T) {
	repository := NewMemoryRepository()
	const accounts = 100
	var wg sync.WaitGroup
	for i := 0; i < accounts; i++ {
		candidate := Account{
			VOIDID:            fmt.Sprintf("client-id-%d", i),
			DisplayName:       "shared display names are allowed",
			IdentityPublicKey: fmt.Sprintf("public-key-%d", i),
		}
		wg.Add(1)
		go func() {
			defer wg.Done()
			if err := repository.Create(context.Background(), candidate); err != nil {
				t.Errorf("Create(%q): %v", candidate.VOIDID, err)
				return
			}
			stored, err := repository.GetByVOIDID(context.Background(), candidate.VOIDID)
			if err != nil || stored != candidate {
				t.Errorf("GetByVOIDID(%q) = %+v, %v; want %+v", candidate.VOIDID, stored, err, candidate)
			}
		}()
	}
	wg.Wait()
}

func TestMemoryRepositoryReturnsCopies(t *testing.T) {
	repository := NewMemoryRepository()
	original := Account{VOIDID: "client-id", DisplayName: "Jack", IdentityPublicKey: "public-key"}
	if err := repository.Create(context.Background(), original); err != nil {
		t.Fatalf("Create(): %v", err)
	}
	original.DisplayName = "changed source"
	found, err := repository.GetByVOIDID(context.Background(), "client-id")
	if err != nil {
		t.Fatalf("GetByVOIDID(): %v", err)
	}
	if found.DisplayName != "Jack" {
		t.Fatal("changing the creation input changed the stored account")
	}
	found.DisplayName = "changed result"
	stored, err := repository.GetByVOIDID(context.Background(), "client-id")
	if err != nil || stored.DisplayName != "Jack" {
		t.Fatalf("changing lookup result changed repository: %+v, %v", stored, err)
	}
}

func TestMemoryRepositoryCancelledContext(t *testing.T) {
	repository := NewMemoryRepository()
	ctx, cancel := context.WithCancel(context.Background())
	cancel()
	if err := repository.Create(ctx, Account{VOIDID: "client-id"}); !errors.Is(err, context.Canceled) {
		t.Fatalf("cancelled Create() error = %v, want context.Canceled", err)
	}
	if _, err := repository.GetByVOIDID(ctx, "client-id"); !errors.Is(err, context.Canceled) {
		t.Fatalf("cancelled GetByVOIDID() error = %v, want context.Canceled", err)
	}
	if _, err := repository.GetByVOIDID(context.Background(), "client-id"); !errors.Is(err, ErrNotFound) {
		t.Fatalf("cancelled creation must not store an account, got %v", err)
	}
}

func TestMemoryRepositoryZeroValue(t *testing.T) {
	var repository MemoryRepository
	if found, err := repository.GetByVOIDID(context.Background(), "missing"); !errors.Is(err, ErrNotFound) || found != (Account{}) {
		t.Fatalf("empty GetByVOIDID() = %+v, %v; want zero Account, ErrNotFound", found, err)
	}
	candidate := Account{VOIDID: "client-id", DisplayName: "Jack", IdentityPublicKey: "public-key"}
	if err := repository.Create(context.Background(), candidate); err != nil {
		t.Fatalf("zero-value Create(): %v", err)
	}
	if found, err := repository.GetByVOIDID(context.Background(), candidate.VOIDID); err != nil || found != candidate {
		t.Fatalf("zero-value GetByVOIDID() = %+v, %v; want %+v, nil", found, err, candidate)
	}
}

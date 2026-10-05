package account

import (
	"context"
	"errors"
	"testing"
	"time"
)

func TestServiceCreateAndRetrievePreservesClientIdentity(t *testing.T) {
	service := NewService(NewMemoryRepository())
	// These deliberately do not select the old generator format or a key
	// encoding. Cosmetic display names and nonblank submitted strings survive.
	const voidID = " client-generated-id "
	const displayName = " Jack 台灣 "
	const publicKey = " encoded-public-key "
	before := time.Now().UTC()
	created, err := service.Create(context.Background(), voidID, displayName, publicKey)
	after := time.Now().UTC()
	if err != nil {
		t.Fatalf("Create(): %v", err)
	}
	if created.VOIDID != voidID || created.DisplayName != displayName || created.IdentityPublicKey != publicKey {
		t.Fatalf("Create() changed submitted fields: %+v", created)
	}
	if created.CreatedAt.IsZero() || created.CreatedAt.Before(before) || created.CreatedAt.After(after) {
		t.Fatalf("CreatedAt = %v, want server time between %v and %v", created.CreatedAt, before, after)
	}
	if created.CreatedAt.Location() != time.UTC {
		t.Fatalf("CreatedAt location = %v, want UTC", created.CreatedAt.Location())
	}
	stored, err := service.GetByVOIDID(context.Background(), voidID)
	if err != nil || stored != created {
		t.Fatalf("GetByVOIDID() = %+v, %v; want %+v, nil", stored, err, created)
	}
	if _, err := service.GetByVOIDID(context.Background(), "client-generated-id"); !errors.Is(err, ErrNotFound) {
		t.Fatalf("lookup must use exact submitted ID, got %v", err)
	}
}

func TestServiceDuplicateDoesNotOverwrite(t *testing.T) {
	service := NewService(NewMemoryRepository())
	first, err := service.Create(context.Background(), "client-id", "Jack", "first-public-key")
	if err != nil {
		t.Fatalf("first Create(): %v", err)
	}
	duplicate, err := service.Create(context.Background(), "client-id", "Changed", "replacement-public-key")
	if !errors.Is(err, ErrAlreadyExists) || duplicate != (Account{}) {
		t.Fatalf("duplicate Create() = %+v, %v; want zero Account, ErrAlreadyExists", duplicate, err)
	}
	stored, err := service.GetByVOIDID(context.Background(), "client-id")
	if err != nil || stored != first {
		t.Fatalf("duplicate changed stored account: %+v, %v", stored, err)
	}
}

func TestServiceRejectsBlankRequiredFields(t *testing.T) {
	for _, blank := range []string{"", " \t\r\n", "\u3000\u00a0"} {
		for _, field := range []string{"void_id", "display_name", "identity_public_key"} {
			t.Run(field+"/"+blank, func(t *testing.T) {
				repository := &stubRepository{}
				service := NewService(repository)
				id, displayName, publicKey := "client-id", "Jack", "public-key"
				switch field {
				case "void_id":
					id = blank
				case "display_name":
					displayName = blank
				case "identity_public_key":
					publicKey = blank
				}
				created, err := service.Create(context.Background(), id, displayName, publicKey)
				if !errors.Is(err, ErrInvalidAccount) || created != (Account{}) {
					t.Fatalf("Create() = %+v, %v; want zero Account, ErrInvalidAccount", created, err)
				}
				if repository.createCalls != 0 {
					t.Fatal("invalid creation reached repository")
				}
			})
		}
	}
}

func TestServiceLookupValidationAndNotFound(t *testing.T) {
	for _, blank := range []string{"", " \t\n", "\u3000"} {
		repository := &stubRepository{}
		found, err := NewService(repository).GetByVOIDID(context.Background(), blank)
		if !errors.Is(err, ErrInvalidAccount) || found != (Account{}) {
			t.Fatalf("GetByVOIDID(%q) = %+v, %v; want zero Account, ErrInvalidAccount", blank, found, err)
		}
		if repository.getCalls != 0 {
			t.Fatal("invalid lookup reached repository")
		}
	}
	if _, err := NewService(NewMemoryRepository()).GetByVOIDID(context.Background(), "missing-id"); !errors.Is(err, ErrNotFound) {
		t.Fatalf("missing GetByVOIDID() error = %v, want ErrNotFound", err)
	}
}

func TestServicePropagatesRepositoryErrors(t *testing.T) {
	failure := errors.New("repository unavailable")
	repository := &stubRepository{err: failure}
	service := NewService(repository)
	if created, err := service.Create(context.Background(), "client-id", "Jack", "public-key"); err != failure || created != (Account{}) {
		t.Fatalf("Create() = %+v, %v; want zero Account, repository error", created, err)
	}
	if found, err := service.GetByVOIDID(context.Background(), "client-id"); err != failure || found != (Account{}) {
		t.Fatalf("GetByVOIDID() = %+v, %v; want zero Account, repository error", found, err)
	}
}

func TestServiceCancelledContextDoesNotReachRepository(t *testing.T) {
	ctx, cancel := context.WithCancel(context.Background())
	cancel()
	repository := &stubRepository{}
	service := NewService(repository)
	if _, err := service.Create(ctx, "client-id", "Jack", "public-key"); !errors.Is(err, context.Canceled) {
		t.Fatalf("Create() error = %v, want context.Canceled", err)
	}
	if _, err := service.GetByVOIDID(ctx, "client-id"); !errors.Is(err, context.Canceled) {
		t.Fatalf("GetByVOIDID() error = %v, want context.Canceled", err)
	}
	if repository.createCalls != 0 || repository.getCalls != 0 {
		t.Fatal("cancelled operations reached repository")
	}
}

type stubRepository struct {
	err         error
	createCalls int
	getCalls    int
}

func (r *stubRepository) Create(context.Context, Account) error {
	r.createCalls++
	return r.err
}

func (r *stubRepository) GetByVOIDID(context.Context, string) (Account, error) {
	r.getCalls++
	return Account{}, r.err
}

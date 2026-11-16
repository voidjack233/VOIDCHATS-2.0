package account

import (
	"context"
	"crypto/ed25519"
	"crypto/rand"
	"encoding/base64"
	"errors"
	"strings"
	"sync"
	"testing"
	"time"
)

type deletionFixture struct {
	mu      sync.Mutex
	key     []byte
	deleted *time.Time
	calls   int
}

func (f *deletionFixture) DeletionKey(context.Context, string) ([]byte, error) { return f.key, nil }
func (f *deletionFixture) Delete(ctx context.Context, id string) (Status, error) {
	f.mu.Lock()
	defer f.mu.Unlock()
	if ctx.Err() != nil {
		return Status{}, ctx.Err()
	}
	f.calls++
	if f.deleted == nil {
		now := time.Now().UTC()
		f.deleted = &now
	}
	return Status{VoidID: id, State: "deleted", DeletedAt: f.deleted}, nil
}
func (*deletionFixture) Status(context.Context, string, string) (Status, error)          { return Status{}, nil }
func (*deletionFixture) WithActive(_ context.Context, _ []string, fn func() error) error { return fn() }

func deletionTestService(t *testing.T) (*DeletionService, *deletionFixture, ed25519.PrivateKey) {
	t.Helper()
	key, private, err := ed25519.GenerateKey(rand.Reader)
	if err != nil {
		t.Fatal(err)
	}
	store := &deletionFixture{key: key}
	s, err := NewDeletionService(store, "https://api.example.test/backend")
	if err != nil {
		t.Fatal(err)
	}
	return s, store, private
}

func deletionProof(t *testing.T, c *DeletionChallenge, key ed25519.PrivateKey) []byte {
	t.Helper()
	message, err := base64.StdEncoding.Strict().DecodeString(c.Message)
	if err != nil {
		t.Fatal(err)
	}
	return ed25519.Sign(key, message)
}

func TestDeletionAuthorizationTranscriptAndRecovery(t *testing.T) {
	s, store, key := deletionTestService(t)
	ctx := context.Background()
	c, err := s.Begin(ctx, "123")
	if err != nil {
		t.Fatal(err)
	}
	raw, _ := base64.StdEncoding.DecodeString(c.Message)
	parts := strings.Split(string(raw), "\n")
	if len(parts) != 9 || parts[0] != "VOID:delete-account:v1" || parts[1] != c.ChallengeID || parts[2] != "123" ||
		parts[3] != base64.StdEncoding.EncodeToString(store.key) || parts[4] != c.ExpiresAt.Format(time.RFC3339Nano) ||
		parts[5] != base64.StdEncoding.EncodeToString([]byte(s.audience)) || parts[6] != "DELETE" || len(parts[7]) != 43 || parts[8] != "" {
		t.Fatalf("invalid transcript: %q", raw)
	}
	first, err := s.Confirm(ctx, c, c.ChallengeID, "DELETE", deletionProof(t, c, key))
	if err != nil || first.State != "deleted" || first.DeletedAt == nil {
		t.Fatal(first, err)
	}
	if _, err = s.Confirm(ctx, c, c.ChallengeID, "DELETE", deletionProof(t, c, key)); !errors.Is(err, ErrInvalidChallenge) {
		t.Fatal("replay accepted", err)
	}
	// Model a lost ACK followed by a service restart; only the store persists.
	restarted, _ := NewDeletionService(store, s.audience)
	retry, err := restarted.Begin(ctx, "123")
	if err != nil {
		t.Fatal(err)
	}
	again, err := restarted.Confirm(ctx, retry, retry.ChallengeID, "DELETE", deletionProof(t, retry, key))
	if err != nil || !again.DeletedAt.Equal(*first.DeletedAt) {
		t.Fatal("recovery changed tombstone", again, err)
	}
}

func TestDeletionRejectsInvalidExpiredAndCrossOperationProofs(t *testing.T) {
	for _, name := range []string{"wrong_key", "wrong_challenge", "confirmation", "expired", "authentication_proof", "other_audience", "other_id", "copied_replay"} {
		t.Run(name, func(t *testing.T) {
			s, store, key := deletionTestService(t)
			c, err := s.Begin(context.Background(), "123")
			if err != nil {
				t.Fatal(err)
			}
			id, confirm := c.ChallengeID, "DELETE"
			signature := deletionProof(t, c, key)
			switch name {
			case "wrong_key":
				_, other, _ := ed25519.GenerateKey(rand.Reader)
				signature = deletionProof(t, c, other)
			case "wrong_challenge":
				id = "another"
			case "confirmation":
				confirm = "delete"
			case "expired":
				s.now = func() time.Time { return c.ExpiresAt }
			case "authentication_proof":
				signature = ed25519.Sign(key, []byte("VOID-NOTHINGNESS:authenticate:v2\n"))
			case "other_audience", "other_id":
				raw, _ := base64.StdEncoding.DecodeString(c.Message)
				if name == "other_id" {
					raw = []byte(strings.Replace(string(raw), "\n123\n", "\n124\n", 1))
				} else {
					raw = []byte(strings.Replace(string(raw), base64.StdEncoding.EncodeToString([]byte(s.audience)), base64.StdEncoding.EncodeToString([]byte("https://other.example")), 1))
				}
				signature = ed25519.Sign(key, raw)
			case "copied_replay":
				copy := *c
				if _, err = s.Confirm(context.Background(), &copy, id, confirm, signature); err != nil {
					t.Fatal(err)
				}
			}
			if _, err = s.Confirm(context.Background(), c, id, confirm, signature); err == nil {
				t.Fatal("invalid authorization accepted")
			}
			if name != "copied_replay" && store.calls != 0 {
				t.Fatal("invalid proof reached storage")
			}
			if _, err = s.Confirm(context.Background(), c, c.ChallengeID, "DELETE", deletionProof(t, c, key)); !errors.Is(err, ErrInvalidChallenge) {
				t.Fatal("failed attempt was reusable", err)
			}
		})
	}
}

func TestDeletionChallengeConcurrentConsumption(t *testing.T) {
	s, store, key := deletionTestService(t)
	c, _ := s.Begin(context.Background(), "123")
	sig := deletionProof(t, c, key)
	var group sync.WaitGroup
	for range 12 {
		group.Go(func() { _, _ = s.Confirm(context.Background(), c, c.ChallengeID, "DELETE", sig) })
	}
	group.Wait()
	if store.calls != 1 {
		t.Fatal("challenge consumed more than once", store.calls)
	}
}

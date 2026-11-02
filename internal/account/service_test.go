package account

import (
	"context"
	"crypto/ed25519"
	"crypto/rand"
	"encoding/base64"
	"encoding/hex"
	"encoding/json"
	"errors"
	"strconv"
	"strings"
	"sync"
	"sync/atomic"
	"testing"
	"time"

	"filippo.io/edwards25519"
)

type fakeIDs struct{ n atomic.Int64 }

type idFunc func(context.Context) (int64, error)

func (f idFunc) Next(ctx context.Context) (int64, error) { return f(ctx) }

func (g *fakeIDs) Next(context.Context) (int64, error) { return g.n.Add(1), nil }

type repositoryFunc func(context.Context, int64, []byte) (Account, error)

func (f repositoryFunc) Insert(ctx context.Context, id int64, key []byte) (Account, error) {
	return f(ctx, id, key)
}

func testService(t *testing.T, repo Repository) (*Service, ed25519.PublicKey, ed25519.PrivateKey) {
	t.Helper()
	if repo == nil {
		repo = repositoryFunc(func(_ context.Context, id int64, _ []byte) (Account, error) {
			return Account{VoidID: strconv.FormatInt(id, 10), CreatedAt: time.Now().UTC()}, nil
		})
	}
	s, err := NewService(repo, &fakeIDs{}, Options{ChallengeTTL: time.Minute, MaxChallenges: 32})
	if err != nil {
		t.Fatal(err)
	}
	pub, priv, err := ed25519.GenerateKey(rand.Reader)
	if err != nil {
		t.Fatal(err)
	}
	return s, pub, priv
}

func signedChallenge(t *testing.T, s *Service, pub ed25519.PublicKey, priv ed25519.PrivateKey) (Challenge, []byte) {
	t.Helper()
	c, err := s.Challenge(pub)
	if err != nil {
		t.Fatal(err)
	}
	message, err := base64.StdEncoding.DecodeString(c.Message)
	if err != nil {
		t.Fatal(err)
	}
	return c, ed25519.Sign(priv, message)
}

func TestRegistrationAndReplay(t *testing.T) {
	s, pub, priv := testService(t, nil)
	c, sig := signedChallenge(t, s, pub, priv)
	a, err := s.Register(context.Background(), c.ChallengeID, sig)
	if err != nil || a.VoidID != "1" || a.CreatedAt.IsZero() {
		t.Fatalf("registration: %+v %v", a, err)
	}
	if _, err = s.Register(context.Background(), c.ChallengeID, sig); !errors.Is(err, ErrInvalidChallenge) {
		t.Fatalf("replay: %v", err)
	}
}

func TestRegistrationIdentityProof(t *testing.T) {
	var storedKey []byte
	s, pub, priv := testService(t, repositoryFunc(func(_ context.Context, id int64, key []byte) (Account, error) {
		storedKey = append([]byte(nil), key...)
		return Account{VoidID: strconv.FormatInt(id, 10), CreatedAt: time.Now().UTC()}, nil
	}))
	c, err := s.Challenge(pub)
	if err != nil {
		t.Fatal(err)
	}
	message, err := base64.StdEncoding.DecodeString(c.Message)
	if err != nil {
		t.Fatal(err)
	}
	parts := strings.Split(string(message), "\n")
	if len(parts) != 7 || parts[0] != "VOID-NOTHINGNESS:register:v1" || parts[1] != c.ChallengeID || parts[3] != base64.StdEncoding.EncodeToString(pub) || parts[4] != "" || parts[5] == "" || parts[6] != "" {
		t.Fatalf("reserved line changed proof framing: %q", message)
	}
	a, err := s.Register(context.Background(), c.ChallengeID, ed25519.Sign(priv, message))
	if err != nil || a.VoidID != "1" || string(storedKey) != string(pub) {
		t.Fatalf("registration: %+v %v", a, err)
	}
	encoded, err := json.Marshal(a)
	if err != nil || strings.Contains(string(encoded), "username") {
		t.Fatalf("account exposed a username: %s %v", encoded, err)
	}
	if _, err = s.Register(context.Background(), c.ChallengeID, ed25519.Sign(priv, message)); !errors.Is(err, ErrInvalidChallenge) {
		t.Fatalf("proof replay: %v", err)
	}
	wrong, err := s.Challenge(pub)
	if err != nil {
		t.Fatal(err)
	}
	wrongMessage, _ := base64.StdEncoding.DecodeString(wrong.Message)
	_, wrongPrivate, err := ed25519.GenerateKey(rand.Reader)
	if err != nil {
		t.Fatal(err)
	}
	if _, err = s.Register(context.Background(), wrong.ChallengeID, ed25519.Sign(wrongPrivate, wrongMessage)); !errors.Is(err, ErrInvalidSignature) {
		t.Fatalf("registration bypassed key ownership: %v", err)
	}
}

func TestValidation(t *testing.T) {
	s, _, _ := testService(t, nil)
	for _, n := range []int{0, 31, 33} {
		if _, err := s.Challenge(make([]byte, n)); !errors.Is(err, ErrInvalidInput) {
			t.Fatalf("key size %d: %v", n, err)
		}
	}
}

func TestSignatureBindingAndConsumption(t *testing.T) {
	s, pub, priv := testService(t, nil)
	c, sig := signedChallenge(t, s, pub, priv)
	sig[0] ^= 1
	if _, err := s.Register(context.Background(), c.ChallengeID, sig); !errors.Is(err, ErrInvalidSignature) {
		t.Fatal(err)
	}
	if _, err := s.Register(context.Background(), c.ChallengeID, sig); !errors.Is(err, ErrInvalidChallenge) {
		t.Fatal(err)
	}
	c1, _ := signedChallenge(t, s, pub, priv)
	c2, sig2 := signedChallenge(t, s, pub, priv)
	if _, err := s.Register(context.Background(), c1.ChallengeID, sig2); !errors.Is(err, ErrInvalidSignature) {
		t.Fatal("signature reused across challenges", err)
	}
	if _, err := s.Register(context.Background(), c2.ChallengeID, sig2); err != nil {
		t.Fatal(err)
	}
	c, err := s.Challenge(pub)
	if err != nil {
		t.Fatal(err)
	}
	message, _ := base64.StdEncoding.DecodeString(c.Message)
	changed := strings.Replace(string(message), "\n\n", "\n"+base64.StdEncoding.EncodeToString([]byte("modified"))+"\n", 1)
	if _, err = s.Register(context.Background(), c.ChallengeID, ed25519.Sign(priv, []byte(changed))); !errors.Is(err, ErrInvalidSignature) {
		t.Fatal("modified reserved line accepted", err)
	}
}

func TestExpiryAndCapacity(t *testing.T) {
	s, pub, priv := testService(t, nil)
	s.opts.MaxChallenges = 1
	now := time.Date(2026, 1, 1, 0, 0, 0, 0, time.UTC)
	s.now = func() time.Time { return now }
	c, sig := signedChallenge(t, s, pub, priv)
	if _, err := s.Challenge(pub); !errors.Is(err, ErrCapacity) {
		t.Fatal(err)
	}
	now = now.Add(time.Minute)
	if _, err := s.Register(context.Background(), c.ChallengeID, sig); !errors.Is(err, ErrInvalidChallenge) {
		t.Fatal(err)
	}
	if _, err := s.Challenge(pub); err != nil {
		t.Fatal("capacity not reclaimed", err)
	}
	now = now.Add(time.Minute)
	if _, err := s.Challenge(pub); err != nil {
		t.Fatal("expired challenges not pruned", err)
	}
}

func TestConcurrentChallengeConsumption(t *testing.T) {
	var inserts atomic.Int64
	s, pub, priv := testService(t, repositoryFunc(func(_ context.Context, id int64, _ []byte) (Account, error) {
		inserts.Add(1)
		return Account{VoidID: strconv.FormatInt(id, 10)}, nil
	}))
	c, sig := signedChallenge(t, s, pub, priv)
	var success atomic.Int64
	var wg sync.WaitGroup
	for range 64 {
		wg.Go(func() {
			_, err := s.Register(context.Background(), c.ChallengeID, sig)
			if err == nil {
				success.Add(1)
			} else if !errors.Is(err, ErrInvalidChallenge) {
				t.Errorf("unexpected: %v", err)
			}
		})
	}
	wg.Wait()
	if success.Load() != 1 || inserts.Load() != 1 {
		t.Fatalf("success=%d inserts=%d", success.Load(), inserts.Load())
	}
}

func TestConflictsFailuresAndCancellation(t *testing.T) {
	for _, test := range []struct {
		name  string
		fail  error
		want  error
		calls int
	}{
		{"duplicate", ErrDuplicateKey, ErrDuplicateKey, 1},
		{"unavailable", errors.New("password=must-not-escape"), ErrUnavailable, 1},
		{"collision limit", ErrIDCollision, ErrUnavailable, 5},
	} {
		t.Run(test.name, func(t *testing.T) {
			calls := 0
			s, pub, priv := testService(t, repositoryFunc(func(context.Context, int64, []byte) (Account, error) { calls++; return Account{}, test.fail }))
			c, sig := signedChallenge(t, s, pub, priv)
			_, err := s.Register(context.Background(), c.ChallengeID, sig)
			if !errors.Is(err, test.want) || calls != test.calls {
				t.Fatalf("err=%v calls=%d", err, calls)
			}
			if strings.Contains(err.Error(), "must-not-escape") {
				t.Fatal("repository details leaked")
			}
			if _, err = s.Register(context.Background(), c.ChallengeID, sig); !errors.Is(err, ErrInvalidChallenge) {
				t.Fatal(err)
			}
		})
	}
	calls := 0
	s, pub, priv := testService(t, repositoryFunc(func(_ context.Context, id int64, _ []byte) (Account, error) {
		calls++
		if calls < 3 {
			return Account{}, ErrIDCollision
		}
		return Account{VoidID: strconv.FormatInt(id, 10)}, nil
	}))
	c, sig := signedChallenge(t, s, pub, priv)
	if a, err := s.Register(context.Background(), c.ChallengeID, sig); err != nil || a.VoidID != "3" {
		t.Fatalf("retry: %+v %v", a, err)
	}
	c, sig = signedChallenge(t, s, pub, priv)
	ctx, cancel := context.WithCancel(context.Background())
	cancel()
	if _, err := s.Register(ctx, c.ChallengeID, sig); !errors.Is(err, context.Canceled) {
		t.Fatal(err)
	}
	if _, err := s.Register(context.Background(), c.ChallengeID, sig); err != nil {
		t.Fatal("already-cancelled request consumed challenge", err)
	}
}

func TestPublicKeyCopied(t *testing.T) {
	s, pub, priv := testService(t, nil)
	c, sig := signedChallenge(t, s, pub, priv)
	for i := range pub {
		pub[i] = 0
	}
	if _, err := s.Register(context.Background(), c.ChallengeID, sig); err != nil {
		t.Fatal(err)
	}
}

func TestRejectDegenerateAndNoncanonicalPublicKeys(t *testing.T) {
	s, pub, _ := testService(t, nil)
	identity := make([]byte, 32)
	identity[0] = 1
	// This signature verifies for arbitrary messages under the identity point;
	// no standard Ed25519 private key is needed to construct it.
	signature := make([]byte, 64)
	basepoint, err := hex.DecodeString("5866666666666666666666666666666666666666666666666666666666666666")
	if err != nil {
		t.Fatal(err)
	}
	copy(signature, basepoint)
	signature[32] = 1
	if !ed25519.Verify(identity, []byte("any registration payload"), signature) {
		t.Fatal("degenerate-key regression fixture changed")
	}
	noncanonical := append([]byte(nil), identity...)
	noncanonical[31] = 0x80
	orderTwo := make([]byte, 32)
	for i := range orderTwo {
		orderTwo[i] = 0xff
	}
	orderTwo[0], orderTwo[31] = 0xec, 0x7f
	torsion, err := new(edwards25519.Point).SetBytes(orderTwo)
	if err != nil {
		t.Fatal(err)
	}
	point, err := new(edwards25519.Point).SetBytes(pub)
	if err != nil {
		t.Fatal(err)
	}
	mixedOrder := new(edwards25519.Point).Add(point, torsion).Bytes()
	for _, key := range [][]byte{identity, make([]byte, 32), noncanonical, orderTwo, mixedOrder} {
		if _, err := s.Challenge(key); !errors.Is(err, ErrInvalidInput) {
			t.Fatal("accepted unsafe public key", err)
		}
	}
}

func TestCancellationDuringIDGeneration(t *testing.T) {
	s, pub, priv := testService(t, nil)
	ctx, cancel := context.WithCancel(context.Background())
	s.ids = idFunc(func(ctx context.Context) (int64, error) { cancel(); <-ctx.Done(); return 0, ctx.Err() })
	c, sig := signedChallenge(t, s, pub, priv)
	if _, err := s.Register(ctx, c.ChallengeID, sig); !errors.Is(err, context.Canceled) {
		t.Fatal(err)
	}
	if _, err := s.Register(context.Background(), c.ChallengeID, sig); !errors.Is(err, ErrInvalidChallenge) {
		t.Fatal("canceled in-flight registration replayed", err)
	}
}

package auth

import (
	"bytes"
	"context"
	"crypto/ed25519"
	"crypto/rand"
	"encoding/base64"
	"encoding/json"
	"errors"
	"fmt"
	"io"
	"strconv"
	"strings"
	"sync"
	"sync/atomic"
	"testing"
	"time"

	"filippo.io/edwards25519"
)

type lookupFunc func(context.Context, int64) ([]byte, error)

func (f lookupFunc) AuthenticationKey(ctx context.Context, id int64) ([]byte, error) {
	return f(ctx, id)
}

func testService(t *testing.T) (*Service, ed25519.PublicKey, ed25519.PrivateKey) {
	t.Helper()
	pub, priv, err := ed25519.GenerateKey(rand.Reader)
	if err != nil {
		t.Fatal(err)
	}
	s, err := NewService(lookupFunc(func(_ context.Context, id int64) ([]byte, error) {
		if id != 42 {
			return nil, ErrNotFound
		}
		return pub, nil
	}), 15*time.Second)
	if err != nil {
		t.Fatal(err)
	}
	return s, pub, priv
}

func signedChallenge(t *testing.T, s *Service, id string, private ed25519.PrivateKey) (*Challenge, []byte) {
	t.Helper()
	c, err := s.Begin(context.Background(), id)
	if err != nil {
		t.Fatal(err)
	}
	message, err := base64.StdEncoding.Strict().DecodeString(c.Message)
	if err != nil {
		t.Fatal(err)
	}
	return c, ed25519.Sign(private, message)
}

func TestValidAuthenticationAndReplay(t *testing.T) {
	s, _, private := testService(t)
	c, signature := signedChallenge(t, s, "42", private)
	id, err := c.Verify(c.ChallengeID, signature)
	if err != nil || id != "42" {
		t.Fatalf("authenticated identity = %q, error = %v", id, err)
	}
	if id, err = c.Verify(c.ChallengeID, signature); id != "" || !errors.Is(err, ErrAuthentication) {
		t.Fatalf("replay = %q, %v", id, err)
	}
}

func TestChallengeMessageAndJSON(t *testing.T) {
	s, _, _ := testService(t)
	now := time.Date(2026, 11, 1, 14, 23, 56, 123456789, time.FixedZone("Manila", 8*60*60))
	s.now = func() time.Time { return now }
	c, err := s.Begin(context.Background(), "42")
	if err != nil {
		t.Fatal(err)
	}
	message, err := base64.StdEncoding.Strict().DecodeString(c.Message)
	if err != nil {
		t.Fatal(err)
	}
	parts := strings.Split(string(message), "\n")
	if len(parts) != 6 || parts[0] != "VOID-NOTHINGNESS:authenticate:v1" || parts[1] != c.ChallengeID || parts[2] != "42" || parts[5] != "" {
		t.Fatal("invalid authentication message framing")
	}
	if parts[3] != now.Add(s.ttl).UTC().Format(time.RFC3339Nano) || !c.ExpiresAt.Equal(now.Add(s.ttl)) || c.ExpiresAt.Location() != time.UTC {
		t.Fatal("expiration must bind the exact UTC expiry")
	}
	for _, token := range []string{c.ChallengeID, parts[4]} {
		decoded, err := base64.RawURLEncoding.Strict().DecodeString(token)
		if err != nil || len(decoded) != 32 || len(token) != 43 {
			t.Fatal("challenge ID and nonce require 256 bits of raw URL-safe base64")
		}
	}
	if c.ChallengeID == parts[4] {
		t.Fatal("nonce and challenge ID unexpectedly match")
	}
	encoded, err := json.Marshal(c)
	if err != nil {
		t.Fatal(err)
	}
	var fields map[string]json.RawMessage
	if err := json.Unmarshal(encoded, &fields); err != nil {
		t.Fatal(err)
	}
	if len(fields) != 3 || fields["challenge_id"] == nil || fields["message"] == nil || fields["expires_at"] == nil {
		t.Fatalf("unexpected challenge envelope fields: %v", fields)
	}
}

func TestAudienceBoundAuthenticationMessageAndProof(t *testing.T) {
	legacy, public, private := testService(t)
	const audience = "https://api.voidchats.xyz/dm9pZG5vdGhpbmduZXNz"
	s, err := NewServiceForAudience(legacy.lookup, 15*time.Second, audience)
	if err != nil {
		t.Fatal(err)
	}
	now := time.Date(2026, 11, 2, 0, 0, 0, 123, time.UTC)
	s.now = func() time.Time { return now }
	c, signature := signedChallenge(t, s, "42", private)
	message, _ := base64.StdEncoding.Strict().DecodeString(c.Message)
	parts := strings.Split(string(message), "\n")
	if len(parts) != 8 || parts[0] != "VOID-NOTHINGNESS:authenticate:v2" || parts[1] != c.ChallengeID || parts[2] != "42" ||
		parts[3] != base64.StdEncoding.EncodeToString(public) || parts[4] != now.Add(s.ttl).Format(time.RFC3339Nano) ||
		parts[5] != base64.StdEncoding.EncodeToString([]byte(audience)) || parts[7] != "" {
		t.Fatalf("invalid audience-bound framing: %q", message)
	}
	if nonce, err := base64.RawURLEncoding.Strict().DecodeString(parts[6]); err != nil || len(nonce) != 32 {
		t.Fatal("authentication nonce must retain 256 random bits")
	}
	if id, err := c.Verify(c.ChallengeID, signature); err != nil || id != "42" {
		t.Fatalf("audience-bound proof: %q %v", id, err)
	}
	if _, err = c.Verify(c.ChallengeID, signature); err != ErrAuthentication {
		t.Fatalf("v2 proof replay: %v", err)
	}
}

func TestAudienceAndProtocolSubstitutionAreRejected(t *testing.T) {
	legacy, _, private := testService(t)
	now := time.Date(2026, 11, 2, 0, 0, 0, 0, time.UTC)
	for _, otherAudience := range []string{"https://other.example/relay", ""} {
		t.Run(otherAudience, func(t *testing.T) {
			production, err := NewServiceForAudience(legacy.lookup, 15*time.Second, "https://api.example/relay")
			if err != nil {
				t.Fatal(err)
			}
			var other *Service
			if otherAudience == "" {
				other, err = NewService(legacy.lookup, 15*time.Second)
			} else {
				other, err = NewServiceForAudience(legacy.lookup, 15*time.Second, otherAudience)
			}
			if err != nil {
				t.Fatal(err)
			}
			// Keep every random byte and timestamp equal so only the bound
			// audience/protocol differs between these two connection challenges.
			for _, s := range []*Service{production, other} {
				s.now = func() time.Time { return now }
				s.rand = bytes.NewReader(bytes.Repeat([]byte{42}, 64))
			}
			challenge, err := production.Begin(context.Background(), "42")
			if err != nil {
				t.Fatal(err)
			}
			foreign, signature := signedChallenge(t, other, "42", private)
			if foreign.ChallengeID != challenge.ChallengeID {
				t.Fatal("substitution fixture failed to hold challenge ID constant")
			}
			if _, err = challenge.Verify(challenge.ChallengeID, signature); err != ErrAuthentication {
				t.Fatalf("cross-audience/protocol proof accepted: %v", err)
			}
		})
	}
}

func TestAudienceConstructorRequiresCanonicalHTTPS(t *testing.T) {
	legacy, _, _ := testService(t)
	for _, audience := range []string{"", "http://api.example/relay", "https:///relay", "https://user:pass@api.example/relay", "https://api.example/relay/", "https://api.example/relay?", "https://api.example/relay?q=1", "https://api.example/relay#fragment", "https://api.example/relay\n", " https://api.example/relay", strings.Repeat("x", 2049)} {
		if _, err := NewServiceForAudience(legacy.lookup, time.Second, audience); err == nil {
			t.Errorf("invalid audience accepted: %q", audience)
		}
	}
	if _, err := NewServiceForAudience(nil, time.Second, "https://api.example/relay"); err == nil {
		t.Fatal("nil identity lookup accepted")
	}
	if _, err := NewServiceForAudience(legacy.lookup, 31*time.Second, "https://api.example/relay"); err == nil {
		t.Fatal("unbounded authentication lifetime accepted")
	}
}

func TestInvalidProofConsumesChallenge(t *testing.T) {
	for _, test := range []string{"wrong key", "modified signature", "empty signature", "short signature", "long signature", "empty ID", "wrong ID"} {
		t.Run(test, func(t *testing.T) {
			s, _, private := testService(t)
			c, validSignature := signedChallenge(t, s, "42", private)
			id := c.ChallengeID
			signature := append([]byte(nil), validSignature...)
			switch test {
			case "wrong key":
				_, wrongPrivate, err := ed25519.GenerateKey(rand.Reader)
				if err != nil {
					t.Fatal(err)
				}
				message, _ := base64.StdEncoding.DecodeString(c.Message)
				signature = ed25519.Sign(wrongPrivate, message)
			case "modified signature":
				signature[0] ^= 1
			case "empty signature":
				signature = nil
			case "short signature":
				signature = signature[:63]
			case "long signature":
				signature = append(signature, 0)
			case "empty ID":
				id = ""
			case "wrong ID":
				id = strings.Repeat("!", 43)
			}
			if id, err := c.Verify(id, signature); id != "" || !errors.Is(err, ErrAuthentication) {
				t.Fatalf("invalid proof accepted: %q, %v", id, err)
			}
			if id, err := c.Verify(c.ChallengeID, validSignature); id != "" || !errors.Is(err, ErrAuthentication) {
				t.Fatalf("failed proof did not consume challenge: %q, %v", id, err)
			}
		})
	}
}

func TestIdentityAndConnectionBinding(t *testing.T) {
	s, public, private := testService(t)
	s.lookup = lookupFunc(func(context.Context, int64) ([]byte, error) { return public, nil })
	first, _ := signedChallenge(t, s, "42", private)
	second, secondSignature := signedChallenge(t, s, "43", private)
	if id, err := first.Verify(first.ChallengeID, secondSignature); id != "" || !errors.Is(err, ErrAuthentication) {
		t.Fatalf("another connection's proof accepted: %q, %v", id, err)
	}
	if id, err := second.Verify(second.ChallengeID, secondSignature); id != "43" || err != nil {
		t.Fatalf("unrelated connection consumed: %q, %v", id, err)
	}
	third, _ := signedChallenge(t, s, "42", private)
	message, _ := base64.StdEncoding.DecodeString(third.Message)
	modified := bytes.Replace(message, []byte("\n42\n"), []byte("\n43\n"), 1)
	if id, err := third.Verify(third.ChallengeID, ed25519.Sign(private, modified)); id != "" || !errors.Is(err, ErrAuthentication) {
		t.Fatalf("identity substitution accepted: %q, %v", id, err)
	}
}

func TestExpiryBoundary(t *testing.T) {
	for _, offset := range []time.Duration{-time.Nanosecond, 0, time.Nanosecond} {
		t.Run(offset.String(), func(t *testing.T) {
			s, _, private := testService(t)
			now := time.Date(2026, 11, 1, 0, 0, 0, 0, time.UTC)
			s.now = func() time.Time { return now }
			c, signature := signedChallenge(t, s, "42", private)
			now = now.Add(s.ttl + offset)
			id, err := c.Verify(c.ChallengeID, signature)
			if offset < 0 {
				if err != nil || id != "42" {
					t.Fatalf("unexpired proof rejected: %q, %v", id, err)
				}
			} else if id != "" || !errors.Is(err, ErrAuthentication) {
				t.Fatalf("expired proof accepted: %q, %v", id, err)
			}
			// Expiry and failed proofs permanently consume the challenge.
			now = now.Add(-s.ttl)
			if _, err := c.Verify(c.ChallengeID, signature); !errors.Is(err, ErrAuthentication) {
				t.Fatalf("consumed challenge became usable: %v", err)
			}
		})
	}
}

func TestConcurrentConsumptionAndCopies(t *testing.T) {
	s, _, private := testService(t)
	c, signature := signedChallenge(t, s, "42", private)
	var successes atomic.Int64
	var group sync.WaitGroup
	for range 64 {
		// Even value copies retain the original connection's consumption state.
		copyOfChallenge := *c
		group.Go(func() {
			id, err := copyOfChallenge.Verify(c.ChallengeID, signature)
			if err == nil {
				if id != "42" {
					t.Errorf("wrong identity %q", id)
				}
				successes.Add(1)
			} else if !errors.Is(err, ErrAuthentication) {
				t.Errorf("unexpected error: %v", err)
			}
		})
	}
	group.Wait()
	if successes.Load() != 1 {
		t.Fatalf("successful verifications = %d, want 1", successes.Load())
	}
}

func TestCanonicalVoidIDs(t *testing.T) {
	for _, id := range []string{"1", "9", "10", "42", "9223372036854775807"} {
		if !ValidVoidID(id) {
			t.Errorf("canonical ID rejected: %q", id)
		}
	}
	for _, id := range []string{"", "0", "00", "01", "00042", "-1", "+1", " 42", "42 ", "42\n", "1.0", "1e2", "４２", "٤٢", "9223372036854775808", "18446744073709551615", strings.Repeat("9", 10000), string([]byte{0xff}), "4\x002"} {
		t.Run(strconv.Quote(id[:min(len(id), 32)]), func(t *testing.T) {
			if ValidVoidID(id) {
				t.Fatal("noncanonical ID accepted")
			}
			calls := 0
			s, err := NewService(lookupFunc(func(context.Context, int64) ([]byte, error) {
				calls++
				return nil, nil
			}), time.Second)
			if err != nil {
				t.Fatal(err)
			}
			if c, err := s.Begin(context.Background(), id); c != nil || !errors.Is(err, ErrInvalidInput) || calls != 0 {
				t.Fatalf("invalid Begin: challenge=%v error=%v lookup calls=%d", c, err, calls)
			}
		})
	}
}

func TestUnknownUnavailableAndCancellation(t *testing.T) {
	for _, test := range []struct {
		name string
		err  error
		want error
	}{
		{"unknown account", ErrNotFound, ErrAuthentication},
		{"wrapped unknown account", fmt.Errorf("private storage detail: %w", ErrNotFound), ErrAuthentication},
		{"storage failure", errors.New("private storage detail"), ErrUnavailable},
		{"context cancellation", context.Canceled, ErrUnavailable},
		{"context deadline", context.DeadlineExceeded, ErrUnavailable},
	} {
		t.Run(test.name, func(t *testing.T) {
			s, _, _ := testService(t)
			s.lookup = lookupFunc(func(context.Context, int64) ([]byte, error) { return nil, test.err })
			if c, err := s.Begin(context.Background(), "42"); c != nil || err != test.want {
				t.Fatalf("lookup failure = %v, %v", c, err)
			}
		})
	}
	for _, cancelBeforeLookup := range []bool{true, false} {
		t.Run(fmt.Sprintf("canceled_before_lookup=%t", cancelBeforeLookup), func(t *testing.T) {
			s, public, _ := testService(t)
			ctx, cancel := context.WithCancel(context.Background())
			defer cancel()
			calls := 0
			s.lookup = lookupFunc(func(context.Context, int64) ([]byte, error) {
				calls++
				cancel()
				return public, nil
			})
			if cancelBeforeLookup {
				cancel()
			}
			if c, err := s.Begin(ctx, "42"); c != nil || err != ErrUnavailable {
				t.Fatalf("canceled Begin = %v, %v", c, err)
			}
			if cancelBeforeLookup && calls != 0 || !cancelBeforeLookup && calls != 1 {
				t.Fatalf("unexpected lookup calls: %d", calls)
			}
		})
	}
}

func TestRejectInvalidStoredPublicKeys(t *testing.T) {
	s, public, _ := testService(t)
	identity := make([]byte, ed25519.PublicKeySize)
	identity[0] = 1
	noncanonical := append([]byte(nil), identity...)
	noncanonical[31] = 0x80
	orderTwo := bytes.Repeat([]byte{0xff}, 32)
	orderTwo[0], orderTwo[31] = 0xec, 0x7f
	torsion, err := new(edwards25519.Point).SetBytes(orderTwo)
	if err != nil {
		t.Fatal(err)
	}
	point, err := new(edwards25519.Point).SetBytes(public)
	if err != nil {
		t.Fatal(err)
	}
	mixedOrder := new(edwards25519.Point).Add(point, torsion).Bytes()
	for _, key := range [][]byte{nil, make([]byte, 31), make([]byte, 33), make([]byte, 32), identity, noncanonical, orderTwo, mixedOrder} {
		s.lookup = lookupFunc(func(context.Context, int64) ([]byte, error) { return key, nil })
		if c, err := s.Begin(context.Background(), "42"); c != nil || err != ErrUnavailable {
			t.Fatalf("invalid stored key accepted: %v, %v", c, err)
		}
	}
}

func TestStoredKeyAndPublicEnvelopeCannotAlterChallenge(t *testing.T) {
	s, public, private := testService(t)
	c, signature := signedChallenge(t, s, "42", private)
	id := c.ChallengeID
	clear(public)
	c.ChallengeID = "modified"
	c.Message = "modified"
	c.ExpiresAt = time.Time{}
	if authenticated, err := c.Verify(id, signature); authenticated != "42" || err != nil {
		t.Fatalf("mutable envelope or repository key altered challenge: %q, %v", authenticated, err)
	}
}

func TestRandomnessFailureAndIndependentChallenges(t *testing.T) {
	s, _, _ := testService(t)
	s.rand = bytes.NewReader(make([]byte, 63))
	if c, err := s.Begin(context.Background(), "42"); c != nil || err != ErrUnavailable {
		t.Fatalf("partial randomness accepted: %v, %v", c, err)
	}
	s.rand = rand.Reader
	seen := make(map[string]bool)
	for range 128 {
		c, err := s.Begin(context.Background(), "42")
		if err != nil {
			t.Fatal(err)
		}
		if seen[c.ChallengeID] {
			t.Fatal("duplicate challenge ID")
		}
		seen[c.ChallengeID] = true
	}
}

func TestCancellationDuringRandomness(t *testing.T) {
	s, _, _ := testService(t)
	ctx, cancel := context.WithCancel(context.Background())
	defer cancel()
	s.rand = readerFunc(func(buf []byte) (int, error) {
		cancel()
		return len(buf), nil
	})
	if c, err := s.Begin(ctx, "42"); c != nil || err != ErrUnavailable {
		t.Fatalf("canceled randomness accepted: %v, %v", c, err)
	}
}

type readerFunc func([]byte) (int, error)

func (f readerFunc) Read(buf []byte) (int, error) { return f(buf) }

var _ io.Reader = readerFunc(nil)

func TestServiceConfigurationAndEmptyChallenge(t *testing.T) {
	lookup := lookupFunc(func(context.Context, int64) ([]byte, error) { return nil, nil })
	for _, ttl := range []time.Duration{0, -time.Second, 30*time.Second + time.Nanosecond, time.Minute} {
		if s, err := NewService(lookup, ttl); s != nil || err == nil {
			t.Fatalf("invalid TTL %s accepted", ttl)
		}
	}
	for _, ttl := range []time.Duration{time.Nanosecond, time.Second, 30 * time.Second} {
		if _, err := NewService(lookup, ttl); err != nil {
			t.Fatalf("valid TTL %s rejected: %v", ttl, err)
		}
	}
	if s, err := NewService(nil, time.Second); s != nil || err == nil {
		t.Fatal("nil lookup accepted")
	}
	for _, c := range []*Challenge{nil, {}} {
		if id, err := c.Verify("", nil); id != "" || err != ErrAuthentication {
			t.Fatalf("empty challenge accepted: %q, %v", id, err)
		}
	}
}

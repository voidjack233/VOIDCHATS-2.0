package relay

import (
	"bytes"
	"context"
	"crypto/ed25519"
	"encoding/base64"
	"encoding/json"
	"errors"
	"strconv"
	"sync/atomic"
	"testing"
	"time"

	"void-nothingness/internal/prekeys"
)

const prekeyAudience = "https://relay.example.test/api"

func enablePrekeys(t *testing.T, f *fixture, store prekeys.Store) *prekeys.Service {
	t.Helper()
	keys := make(keyLookup, len(f.keys))
	for id, private := range f.keys {
		number, err := strconv.ParseInt(id, 10, 64)
		if err != nil {
			t.Fatal(err)
		}
		keys[number] = private.Public().(ed25519.PublicKey)
	}
	service, err := prekeys.New(store, keys, prekeyAudience)
	if err != nil {
		t.Fatal(err)
	}
	f.server.opts.Prekeys = service
	return service
}

// Signal key objects remain opaque here. These synthetic bytes verify relay
// framing and the real Ed25519 account binding, not Signal cryptography.
func signedPrekeyUpload(t *testing.T, f *fixture, actor string) string {
	t.Helper()
	ec := append([]byte{5}, bytes.Repeat([]byte{13}, 32)...)
	signature := base64.StdEncoding.EncodeToString(make([]byte, 64))
	public := base64.StdEncoding.EncodeToString(ec)
	kyber := base64.StdEncoding.EncodeToString(bytes.Repeat([]byte{19}, 128))
	upload := prekeys.Upload{
		V: 1, DeviceID: 1, RegistrationID: 123, IdentityKey: public,
		SignedPrekey:    prekeys.SignedKey{KeyID: 1, PublicKey: public, Signature: signature},
		KyberLastResort: prekeys.SignedKey{KeyID: 99, PublicKey: kyber, Signature: signature},
		OneTimePrekeys:  []prekeys.PublicKey{{KeyID: 10, PublicKey: public}, {KeyID: 11, PublicKey: public}},
		OneTimeKyberPrekeys: []prekeys.SignedKey{
			{KeyID: 20, PublicKey: kyber, Signature: signature},
			{KeyID: 21, PublicKey: kyber, Signature: signature},
		},
	}
	upload.AccountSignature = base64.StdEncoding.EncodeToString(ed25519.Sign(f.keys[actor], prekeys.IdentityMessage(prekeyAudience, actor, upload)))
	raw, err := json.Marshal(upload)
	if err != nil {
		t.Fatal(err)
	}
	return base64.StdEncoding.EncodeToString(raw)
}

func publishPrekeys(id, payload string) map[string]string {
	return map[string]string{"type": "prekeys_publish", "publication_id": id, "payload_base64": payload}
}

func claimPrekeys(id, target string) map[string]string {
	return map[string]string{"type": "prekeys_claim", "request_id": id, "void_id": target}
}

func expectPrekeyError(t *testing.T, got map[string]string, id, code string) {
	t.Helper()
	assertFields(t, got, map[string]string{"type": "prekeys_error", "request_id": id, "code": code})
}

func TestPrekeysDisabledAndMalformedCommandsAreCorrelated(t *testing.T) {
	for _, tc := range []struct {
		name, code string
		frame      map[string]string
	}{
		{"disabled publication", "prekeys_unavailable", publishPrekeys(firstEventID, "AQ==")},
		{"disabled claim", "prekeys_unavailable", claimPrekeys(firstEventID, "2")},
		{"publication base64", "prekeys_invalid", publishPrekeys(firstEventID, "AQ")},
		{"publication extra field", "prekeys_invalid", map[string]string{"type": "prekeys_publish", "publication_id": firstEventID, "payload_base64": "AQ==", "void_id": "2"}},
		{"claim recipient", "prekeys_invalid", claimPrekeys(firstEventID, "02")},
		{"claim extra field", "prekeys_invalid", map[string]string{"type": "prekeys_claim", "request_id": firstEventID, "void_id": "2", "sender_void_id": "2"}},
	} {
		t.Run(tc.name, func(t *testing.T) {
			f := newFixture(t, Options{}, 2)
			alice := f.authenticate(t, "1")
			write(t, alice, tc.frame)
			expectPrekeyError(t, read(t, alice), firstEventID, tc.code)
			write(t, alice, map[string]string{"type": "lookup", "void_id": "2"})
			if frame := read(t, alice); frame["type"] != "person" {
				t.Fatalf("correlated prekey error closed the connection: %#v", frame)
			}
		})
	}
}

func TestPrekeysUncorrelatableIDsUseOrdinaryInvalidFrame(t *testing.T) {
	for _, id := range []string{"", "not-a-uuid", "00000000-0000-0000-0000-000000000000", "AAAAAAAA-0000-4000-8000-000000000001"} {
		for _, frame := range []map[string]string{publishPrekeys(id, "AQ=="), claimPrekeys(id, "2")} {
			f := newFixture(t, Options{}, 2)
			alice := f.authenticate(t, "1")
			write(t, alice, frame)
			assertFields(t, read(t, alice), map[string]string{"type": "error", "code": "invalid_frame"})
		}
	}
}

func TestPrekeysRequireAuthentication(t *testing.T) {
	for _, frame := range []map[string]string{publishPrekeys(firstEventID, "AQ=="), claimPrekeys(firstEventID, "2")} {
		f := newFixture(t, Options{}, 2)
		store := &failingPrekeys{Store: prekeys.NewMemoryStore([]string{"1", "2"}), err: prekeys.ErrUnavailable}
		enablePrekeys(t, f, store)
		conn := f.dial(t)
		write(t, conn, frame)
		assertFields(t, read(t, conn), map[string]string{"type": "error", "code": "unauthorized"})
		if store.calls.Load() != 0 {
			t.Fatal("unauthenticated command reached public-prekey storage")
		}
	}
}

func TestPrekeysPublishBindingOfflineClaimAndDuplicateSafeRetry(t *testing.T) {
	f := newFixture(t, Options{}, 3)
	service := enablePrekeys(t, f, prekeys.NewMemoryStore([]string{"1", "2", "3"}))
	bob := f.authenticate(t, "2")
	payload := signedPrekeyUpload(t, f, "2")
	wantPublished := map[string]string{"type": "prekeys_published", "publication_id": firstEventID, "ec_remaining": "2", "kyber_remaining": "2"}
	write(t, bob, publishPrekeys(firstEventID, payload))
	assertFields(t, read(t, bob), wantPublished)
	write(t, bob, publishPrekeys(firstEventID, payload))
	assertFields(t, read(t, bob), wantPublished)
	alice := f.authenticate(t, "1")
	write(t, alice, publishPrekeys(firstEventID, payload))
	expectPrekeyError(t, read(t, alice), firstEventID, "prekeys_invalid")
	_ = bob.CloseNow()
	eventually(t, "offline prekey owner remained connected", func() bool { return f.server.ActiveCount() == 1 })
	write(t, alice, claimPrekeys(firstEventID, "2"))
	claimed := read(t, alice)
	if claimed["type"] != "prekey_bundle" || claimed["request_id"] != firstEventID || claimed["void_id"] != "2" || len(claimed) != 4 {
		t.Fatalf("native prekey bundle envelope = %#v", claimed)
	}
	raw, err := base64.StdEncoding.Strict().DecodeString(claimed["payload_base64"])
	if err != nil || len(raw) > prekeys.MaxBundleBytes {
		t.Fatalf("invalid public bundle: %v", err)
	}
	var bundle prekeys.Bundle
	if err := json.Unmarshal(raw, &bundle); err != nil {
		t.Fatal(err)
	}
	if bundle.V != 1 || bundle.DeviceID != 1 || bundle.OneTimePrekey == nil || bundle.OneTimePrekey.KeyID != 10 || bundle.KyberPrekey.KeyID != 20 || bundle.KyberPrekey.LastResort || bundle.ClaimExpiresAt.Before(time.Now()) {
		t.Fatalf("claimed public bundle = %+v", bundle)
	}
	write(t, alice, claimPrekeys(firstEventID, "2"))
	assertFields(t, read(t, alice), claimed)
	write(t, alice, claimPrekeys(firstEventID, "3"))
	expectPrekeyError(t, read(t, alice), firstEventID, "prekeys_conflict")
	counts, err := service.Publish(context.Background(), "2", firstEventID, payload)
	if err != nil || counts.ECRemaining != 1 || counts.KyberRemaining != 1 {
		t.Fatalf("publication/claim retries consumed more public keys: %+v %v", counts, err)
	}
	// An established opaque conversation continues through the same queue;
	// exhausted public-prekey rate tokens never gate ciphertext or ACK traffic.
	bob = f.authenticate(t, "2")
	write(t, alice, queuedSend(secondEventID, "2", "AAH+/w=="))
	expectAccepted(t, read(t, alice), secondEventID)
	expectEvent(t, read(t, bob), secondEventID, "1", "AAH+/w==")
	write(t, bob, map[string]string{"type": "ack", "sender_void_id": "1", "event_id": secondEventID})
	assertFields(t, read(t, bob), map[string]string{"type": "ack_ok", "sender_void_id": "1", "event_id": secondEventID})
	queued, err := f.server.opts.Queue.Pending(context.Background(), "2", 100)
	if err != nil || len(queued) != 0 {
		t.Fatalf("ACK did not delete opaque ciphertext: %+v %v", queued, err)
	}
}

type failingPrekeys struct {
	prekeys.Store
	err   error
	wait  bool
	calls atomic.Int32
}

func (s *failingPrekeys) Publish(ctx context.Context, actor, id string, upload prekeys.Upload, hash [32]byte, now time.Time) (prekeys.Counts, error) {
	s.calls.Add(1)
	return prekeys.Counts{}, s.err
}

func (s *failingPrekeys) Claim(ctx context.Context, actor, id, target string, now time.Time) (prekeys.Bundle, error) {
	s.calls.Add(1)
	if s.wait {
		<-ctx.Done()
		return prekeys.Bundle{}, ctx.Err()
	}
	return prekeys.Bundle{}, s.err
}

func TestPrekeysServiceErrorsAreCorrelatedAndPreserveConnection(t *testing.T) {
	for _, tc := range []struct {
		err  error
		code string
	}{
		{prekeys.ErrInvalid, "prekeys_invalid"},
		{prekeys.ErrConflict, "prekeys_conflict"},
		{prekeys.ErrCapacity, "prekeys_capacity"},
		{prekeys.ErrExpired, "prekey_claim_expired"},
		{prekeys.ErrUnavailable, "prekeys_unavailable"},
		{errors.New("synthetic internal storage failure"), "prekeys_unavailable"},
	} {
		t.Run(tc.code+":"+tc.err.Error(), func(t *testing.T) {
			f := newFixture(t, Options{}, 2)
			enablePrekeys(t, f, &failingPrekeys{Store: prekeys.NewMemoryStore([]string{"1", "2"}), err: tc.err})
			alice := f.authenticate(t, "1")
			write(t, alice, claimPrekeys(firstEventID, "2"))
			expectPrekeyError(t, read(t, alice), firstEventID, tc.code)
			write(t, alice, map[string]string{"type": "lookup", "void_id": "2"})
			if frame := read(t, alice); frame["type"] != "person" {
				t.Fatalf("service error closed the connection: %#v", frame)
			}
		})
	}
	t.Run("store timeout", func(t *testing.T) {
		f := newFixture(t, Options{ForwardTimeout: 10 * time.Millisecond}, 2)
		enablePrekeys(t, f, &failingPrekeys{Store: prekeys.NewMemoryStore([]string{"1", "2"}), wait: true})
		alice := f.authenticate(t, "1")
		write(t, alice, claimPrekeys(firstEventID, "2"))
		expectPrekeyError(t, read(t, alice), firstEventID, "prekeys_unavailable")
		write(t, alice, map[string]string{"type": "lookup", "void_id": "2"})
		if frame := read(t, alice); frame["type"] != "person" {
			t.Fatalf("store timeout closed the connection: %#v", frame)
		}
	})
}

func TestPrekeysRateLimitSurvivesConnectionReplacement(t *testing.T) {
	f := newFixture(t, Options{}, 2)
	alice := f.authenticate(t, "1")
	for range 4 {
		write(t, alice, claimPrekeys(firstEventID, "2"))
		expectPrekeyError(t, read(t, alice), firstEventID, "prekeys_unavailable")
	}
	write(t, alice, claimPrekeys(firstEventID, "2"))
	expectPrekeyError(t, read(t, alice), firstEventID, "prekeys_capacity")
	_ = alice.CloseNow()
	alice = f.authenticate(t, "1")
	write(t, alice, claimPrekeys(firstEventID, "2"))
	expectPrekeyError(t, read(t, alice), firstEventID, "prekeys_capacity")
	bob := f.authenticate(t, "2")
	write(t, bob, claimPrekeys(secondEventID, "1"))
	expectPrekeyError(t, read(t, bob), secondEventID, "prekeys_unavailable")
	// The smaller budget applies only to prekey work, not established messages.
	write(t, alice, queuedSend(firstEventID, "2", "AQ=="))
	expectAccepted(t, read(t, alice), firstEventID)
	expectEvent(t, read(t, bob), firstEventID, "1", "AQ==")
	write(t, bob, map[string]string{"type": "ack", "sender_void_id": "1", "event_id": firstEventID})
	assertFields(t, read(t, bob), map[string]string{"type": "ack_ok", "sender_void_id": "1", "event_id": firstEventID})
}

func TestPrekeysGenericCapacityErrorsAreCorrelated(t *testing.T) {
	t.Run("account budget", func(t *testing.T) {
		f := newFixture(t, Options{}, 2)
		for range 40 {
			if !f.server.allowCommand("1") {
				t.Fatal("test exhausted the account budget early")
			}
		}
		alice := f.authenticate(t, "1")
		write(t, alice, claimPrekeys(firstEventID, "2"))
		expectPrekeyError(t, read(t, alice), firstEventID, "prekeys_capacity")
	})
	t.Run("worker slots", func(t *testing.T) {
		f := newFixture(t, Options{}, 2)
		alice := f.authenticate(t, "1")
		for range cap(f.server.slots) {
			f.server.slots <- struct{}{}
		}
		write(t, alice, claimPrekeys(firstEventID, "2"))
		expectPrekeyError(t, read(t, alice), firstEventID, "prekeys_capacity")
		for range cap(f.server.slots) {
			<-f.server.slots
		}
		write(t, alice, claimPrekeys(firstEventID, "2"))
		expectPrekeyError(t, read(t, alice), firstEventID, "prekeys_unavailable")
	})
}

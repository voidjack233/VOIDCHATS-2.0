package prekeys

import (
	"context"
	"crypto/ed25519"
	"crypto/rand"
	"crypto/sha256"
	"encoding/base64"
	"encoding/json"
	"errors"
	"fmt"
	"reflect"
	"strings"
	"sync"
	"testing"
	"time"

	"void-nothingness/internal/auth"
)

const testAudience = "https://example.test/api"

type testKeys map[int64]ed25519.PublicKey

func (keys testKeys) AuthenticationKey(ctx context.Context, id int64) ([]byte, error) {
	if err := ctx.Err(); err != nil {
		return nil, err
	}
	key, ok := keys[id]
	if !ok {
		return nil, auth.ErrNotFound
	}
	return append([]byte(nil), key...), nil
}

func testUUID(n int) string { return fmt.Sprintf("11111111-1111-1111-1111-%012x", n) }

func testUpload(t *testing.T, actor string, private ed25519.PrivateKey, count int) Upload {
	t.Helper()
	ec := make([]byte, 33)
	ec[0] = 5
	for i := 1; i < len(ec); i++ {
		ec[i] = byte(i)
	}
	signature := base64.StdEncoding.EncodeToString(make([]byte, 64))
	encodedEC := base64.StdEncoding.EncodeToString(ec)
	kyber := base64.StdEncoding.EncodeToString([]byte{1, 2, 3})
	upload := Upload{
		V: 1, DeviceID: 1, RegistrationID: 123,
		IdentityKey:     encodedEC,
		SignedPrekey:    SignedKey{KeyID: 7, PublicKey: encodedEC, Signature: signature},
		KyberLastResort: SignedKey{KeyID: MaxKeyID, PublicKey: kyber, Signature: signature},
		OneTimePrekeys:  []PublicKey{}, OneTimeKyberPrekeys: []SignedKey{},
	}
	for id := 1; id <= count; id++ {
		upload.OneTimePrekeys = append(upload.OneTimePrekeys, PublicKey{KeyID: id, PublicKey: encodedEC})
		upload.OneTimeKyberPrekeys = append(upload.OneTimeKyberPrekeys, SignedKey{KeyID: id, PublicKey: kyber, Signature: signature})
	}
	upload.AccountSignature = base64.StdEncoding.EncodeToString(ed25519.Sign(private, IdentityMessage(testAudience, actor, upload)))
	return upload
}

func encodeUpload(t *testing.T, upload Upload) string {
	t.Helper()
	raw, err := json.Marshal(upload)
	if err != nil {
		t.Fatal(err)
	}
	return base64.StdEncoding.EncodeToString(raw)
}

func decodeBundle(t *testing.T, payload string) Bundle {
	t.Helper()
	raw, err := base64.StdEncoding.DecodeString(payload)
	if err != nil {
		t.Fatal(err)
	}
	var bundle Bundle
	if err := json.Unmarshal(raw, &bundle); err != nil {
		t.Fatal(err)
	}
	return bundle
}

func setup(t *testing.T, count int) (*Service, *MemoryStore, Upload) {
	t.Helper()
	pub1, _, err := ed25519.GenerateKey(rand.Reader)
	if err != nil {
		t.Fatal(err)
	}
	pub2, private, err := ed25519.GenerateKey(rand.Reader)
	if err != nil {
		t.Fatal(err)
	}
	pub3, _, err := ed25519.GenerateKey(rand.Reader)
	if err != nil {
		t.Fatal(err)
	}
	store := NewMemoryStore([]string{"1", "2", "3"})
	service, err := New(store, testKeys{1: pub1, 2: pub2, 3: pub3}, testAudience)
	if err != nil {
		t.Fatal(err)
	}
	return service, store, testUpload(t, "2", private, count)
}

func TestStrictPublicUpload(t *testing.T) {
	_, _, upload := setup(t, 1)
	valid, _ := base64.StdEncoding.DecodeString(encodeUpload(t, upload))
	mutations := map[string]string{
		"unknown":             strings.Replace(string(valid), `"v":1`, `"private_key":"secret","v":1`, 1),
		"nested private":      strings.Replace(string(valid), `"key_id":7`, `"private_key":"secret","key_id":7`, 1),
		"duplicate":           strings.Replace(string(valid), `"v":1`, `"v":1,"v":1`, 1),
		"escaped duplicate":   strings.Replace(string(valid), `"v":1`, `"v":1,"\u0076":1`, 1),
		"case variant":        strings.Replace(string(valid), `"device_id":1`, `"Device_ID":1`, 1),
		"nested case variant": strings.Replace(string(valid), `"key_id":7`, `"KEY_ID":7`, 1),
		"null":                strings.Replace(string(valid), `"one_time_prekeys":[`, `"one_time_prekeys":null,"one_time_prekeys":[`, 1),
		"missing array":       strings.Replace(string(valid), `"one_time_prekeys":`, `"missing":`, 1),
		"float":               strings.Replace(string(valid), `"v":1`, `"v":1.0`, 1),
		"exponent":            strings.Replace(string(valid), `"v":1`, `"v":1e0`, 1),
		"overflow":            strings.Replace(string(valid), `"v":1`, `"v":18446744073709551616`, 1),
		"negative":            strings.Replace(string(valid), `"v":1`, `"v":-1`, 1),
		"boolean":             strings.Replace(string(valid), `"v":1`, `"v":true`, 1),
		"trailing":            string(valid) + `{}`,
	}
	for name, raw := range mutations {
		t.Run(name, func(t *testing.T) {
			if _, _, err := ParseUpload(base64.StdEncoding.EncodeToString([]byte(raw))); !errors.Is(err, ErrInvalid) {
				t.Fatalf("got %v", err)
			}
		})
	}
	if parsed, hash, err := ParseUpload(encodeUpload(t, upload)); err != nil || !reflect.DeepEqual(parsed, upload) || hash != sha256.Sum256(valid) {
		t.Fatalf("valid upload: parsed=%+v hash=%x err=%v", parsed, hash, err)
	}
	for name, mutate := range map[string]func(*Upload){
		"EC prefix":           func(u *Upload) { u.IdentityKey = base64.StdEncoding.EncodeToString(make([]byte, 33)) },
		"EC length":           func(u *Upload) { u.SignedPrekey.PublicKey = base64.StdEncoding.EncodeToString([]byte{5}) },
		"signature length":    func(u *Upload) { u.AccountSignature = base64.StdEncoding.EncodeToString([]byte{1}) },
		"kyber size":          func(u *Upload) { u.KyberLastResort.PublicKey = base64.StdEncoding.EncodeToString(make([]byte, 2049)) },
		"key id":              func(u *Upload) { u.SignedPrekey.KeyID = MaxKeyID + 1 },
		"registration id":     func(u *Upload) { u.RegistrationID = 16381 },
		"device id":           func(u *Upload) { u.DeviceID = 2 },
		"duplicate id":        func(u *Upload) { u.OneTimePrekeys = append(u.OneTimePrekeys, u.OneTimePrekeys[0]) },
		"fallback id reuse":   func(u *Upload) { u.OneTimeKyberPrekeys[0].KeyID = u.KyberLastResort.KeyID },
		"null array":          func(u *Upload) { u.OneTimeKyberPrekeys = nil },
		"noncanonical base64": func(u *Upload) { u.IdentityKey += "\n" },
	} {
		t.Run(name, func(t *testing.T) {
			copy := upload
			copy.OneTimePrekeys = append([]PublicKey{}, upload.OneTimePrekeys...)
			copy.OneTimeKyberPrekeys = append([]SignedKey{}, upload.OneTimeKyberPrekeys...)
			mutate(&copy)
			if _, _, err := ParseUpload(encodeUpload(t, copy)); !errors.Is(err, ErrInvalid) {
				t.Fatalf("got %v", err)
			}
		})
	}
	if _, _, err := ParseUpload(base64.StdEncoding.EncodeToString(make([]byte, MaxUploadBytes+1))); !errors.Is(err, ErrInvalid) {
		t.Fatal("oversized upload accepted")
	}
	if _, _, err := ParseUpload(encodeUpload(t, upload) + "\n"); !errors.Is(err, ErrInvalid) {
		t.Fatal("noncanonical outer base64 accepted")
	}
}

func TestAccountBindingAndSafeFailures(t *testing.T) {
	service, store, upload := setup(t, 1)
	ctx := context.Background()
	if _, err := service.Publish(ctx, "1", testUUID(1), encodeUpload(t, upload)); !errors.Is(err, ErrInvalid) {
		t.Fatalf("cross-account signature accepted: %v", err)
	}
	service.audience = "https://other.example/api"
	if _, err := service.Publish(ctx, "2", testUUID(1), encodeUpload(t, upload)); !errors.Is(err, ErrInvalid) {
		t.Fatalf("cross-audience signature accepted: %v", err)
	}
	service.audience = testAudience
	changed := upload
	changed.RegistrationID++
	if _, err := service.Publish(ctx, "2", testUUID(1), encodeUpload(t, changed)); !errors.Is(err, ErrInvalid) {
		t.Fatalf("changed registration accepted: %v", err)
	}
	changed = upload
	changed.IdentityKey = upload.SignedPrekey.PublicKey[:len(upload.SignedPrekey.PublicKey)-4] + "AAAA"
	if _, err := service.Publish(ctx, "2", testUUID(1), encodeUpload(t, changed)); !errors.Is(err, ErrInvalid) {
		t.Fatalf("changed identity accepted: %v", err)
	}
	if len(store.inventories) != 0 {
		t.Fatal("invalid signature mutated inventory")
	}
	if _, err := service.Claim(ctx, "1", testUUID(1), "99"); !errors.Is(err, ErrInvalid) {
		t.Fatalf("unknown recipient: %v", err)
	}
	if _, err := service.Claim(ctx, "1", testUUID(1), "2"); !errors.Is(err, ErrUnavailable) {
		t.Fatalf("registered recipient without publication: %v", err)
	}
	if _, err := service.Claim(ctx, "1", testUUID(1), "1"); !errors.Is(err, ErrInvalid) {
		t.Fatal("self claim allowed")
	}
	canceled, cancel := context.WithCancel(ctx)
	cancel()
	if _, err := service.Publish(canceled, "2", testUUID(1), encodeUpload(t, upload)); !errors.Is(err, context.Canceled) {
		t.Fatalf("cancellation: %v", err)
	}
	if _, err := service.Publish(ctx, "2", testUUID(1), encodeUpload(t, upload)); err != nil {
		t.Fatal(err)
	}
}

func TestPublicationRetriesAndWatermarks(t *testing.T) {
	service, store, upload := setup(t, 2)
	ctx := context.Background()
	counts, err := service.Publish(ctx, "2", testUUID(1), encodeUpload(t, upload))
	if err != nil || counts != (Counts{ECRemaining: 2, KyberRemaining: 2}) {
		t.Fatalf("publish: %+v %v", counts, err)
	}
	if _, err := service.Claim(ctx, "1", testUUID(100), "2"); err != nil {
		t.Fatal(err)
	}
	counts, err = service.Publish(ctx, "2", testUUID(1), encodeUpload(t, upload))
	if err != nil || counts != (Counts{ECRemaining: 1, KyberRemaining: 1}) {
		t.Fatalf("retry replenished inventory: %+v %v", counts, err)
	}
	changed := upload
	changed.OneTimePrekeys = []PublicKey{}
	changed.OneTimeKyberPrekeys = []SignedKey{}
	if _, err := service.Publish(ctx, "2", testUUID(1), encodeUpload(t, changed)); !errors.Is(err, ErrConflict) {
		t.Fatalf("reused UUID content: %v", err)
	}
	if _, err := service.Publish(ctx, "2", testUUID(2), encodeUpload(t, upload)); !errors.Is(err, ErrConflict) {
		t.Fatalf("consumed ID reuse: %v", err)
	}
	changed.OneTimePrekeys = []PublicKey{{KeyID: 3, PublicKey: upload.IdentityKey}}
	changed.OneTimeKyberPrekeys = []SignedKey{{KeyID: 3, PublicKey: upload.KyberLastResort.PublicKey, Signature: upload.KyberLastResort.Signature}}
	if _, err := service.Publish(ctx, "2", testUUID(2), encodeUpload(t, changed)); err != nil {
		t.Fatal(err)
	}
	if store.inventories["2"].MaxECID != 3 || store.inventories["2"].MaxKyberID != 3 {
		t.Fatal("watermarks not advanced")
	}
	before := cloneInventory(store.inventories["2"])
	changed.SignedPrekey.KeyID++
	if _, err := service.Publish(ctx, "2", testUUID(3), encodeUpload(t, changed)); !errors.Is(err, ErrConflict) {
		t.Fatalf("signed prekey mutation: %v", err)
	}
	if !reflect.DeepEqual(before, store.inventories["2"]) {
		t.Fatal("failed publication changed inventory")
	}
}

func TestClaimRetryExpiryAndScrubbing(t *testing.T) {
	service, store, upload := setup(t, 1)
	ctx := context.Background()
	now := time.Date(2026, 11, 3, 1, 2, 3, 123456789, time.FixedZone("test", 3600))
	service.now = func() time.Time { return now }
	if _, err := service.Publish(ctx, "2", testUUID(1), encodeUpload(t, upload)); err != nil {
		t.Fatal(err)
	}
	payload, err := service.Claim(ctx, "1", testUUID(1), "2")
	if err != nil {
		t.Fatal(err)
	}
	bundle := decodeBundle(t, payload)
	expectedExpiry := now.UTC().Truncate(time.Microsecond).Add(ClaimTTL)
	if !bundle.ClaimExpiresAt.Equal(expectedExpiry) || bundle.OneTimePrekey == nil || bundle.KyberPrekey.LastResort {
		t.Fatalf("unexpected first bundle: %+v", bundle)
	}
	now = now.Add(time.Hour)
	if retry, err := service.Claim(ctx, "1", testUUID(1), "2"); err != nil || retry != payload {
		t.Fatalf("retry changed bundle: %v", err)
	}
	if _, err := service.Claim(ctx, "1", testUUID(1), "3"); !errors.Is(err, ErrConflict) {
		t.Fatalf("UUID rebound to recipient: %v", err)
	}
	if _, err := service.Claim(ctx, "1", testUUID(1), "999999"); !errors.Is(err, ErrConflict) {
		t.Fatalf("UUID rebound to unknown recipient: %v", err)
	}
	if _, err := service.Claim(ctx, "1", testUUID(99), "999999"); !errors.Is(err, ErrInvalid) {
		t.Fatalf("new claim to unknown recipient: %v", err)
	}
	fallback, err := service.Claim(ctx, "1", testUUID(2), "2")
	if err != nil {
		t.Fatal(err)
	}
	if b := decodeBundle(t, fallback); !b.KyberPrekey.LastResort || b.OneTimePrekey != nil || b.KyberPrekey.KeyID != upload.KyberLastResort.KeyID {
		t.Fatalf("fallback bundle: %+v", b)
	}
	now = expectedExpiry
	if _, err := service.Claim(ctx, "1", testUUID(1), "2"); !errors.Is(err, ErrExpired) {
		t.Fatalf("expiry boundary: %v", err)
	}
	if n, err := service.Cleanup(ctx, 1); err != nil || n != 1 {
		t.Fatalf("cleanup: %d %v", n, err)
	}
	claim := store.claims["1:"+testUUID(1)]
	if claim.bundle != nil || claim.recipient != "" || len(store.claims) != 2 || store.claimCounts["1"] != 2 {
		t.Fatal("cleanup did not scrub public bundle/relationship while retaining quota marker")
	}
	if _, err := service.Claim(ctx, "1", testUUID(1), "3"); !errors.Is(err, ErrExpired) {
		t.Fatalf("expired marker reused: %v", err)
	}
	if _, err := service.Claim(ctx, "1", testUUID(1), "999999"); !errors.Is(err, ErrExpired) {
		t.Fatalf("expired marker rebound to unknown recipient: %v", err)
	}
	if n, err := service.Cleanup(ctx, 1000); err != nil || n != 0 {
		t.Fatalf("active bundle scrubbed: %d %v", n, err)
	}
}

func TestConcurrentClaimsAreAtomic(t *testing.T) {
	service, store, upload := setup(t, 64)
	ctx := context.Background()
	if _, err := service.Publish(ctx, "2", testUUID(1), encodeUpload(t, upload)); err != nil {
		t.Fatal(err)
	}
	var wg sync.WaitGroup
	results := make(chan Bundle, 64)
	errorsCh := make(chan error, 64)
	for i := 1; i <= 64; i++ {
		wg.Add(1)
		go func(id int) {
			defer wg.Done()
			payload, err := service.Claim(ctx, "1", testUUID(id), "2")
			if err != nil {
				errorsCh <- err
				return
			}
			raw, _ := base64.StdEncoding.DecodeString(payload)
			var bundle Bundle
			if err := json.Unmarshal(raw, &bundle); err != nil {
				errorsCh <- err
				return
			}
			results <- bundle
		}(i)
	}
	wg.Wait()
	close(results)
	close(errorsCh)
	for err := range errorsCh {
		t.Fatal(err)
	}
	ec, kyber := map[int]bool{}, map[int]bool{}
	for bundle := range results {
		if bundle.OneTimePrekey == nil || bundle.KyberPrekey.LastResort || ec[bundle.OneTimePrekey.KeyID] || kyber[bundle.KyberPrekey.KeyID] {
			t.Fatalf("one-time key allocated twice: %+v", bundle)
		}
		ec[bundle.OneTimePrekey.KeyID], kyber[bundle.KyberPrekey.KeyID] = true, true
	}
	if len(ec) != 64 || len(kyber) != 64 || len(store.inventories["2"].Upload.OneTimePrekeys) != 0 {
		t.Fatal("lost or unconsumed one-time keys")
	}
	// Concurrent retries of one committed UUID produce identical bytes and keep
	// the original claim count, even after all one-time keys have been consumed.
	payloads := make(chan string, 16)
	for range 16 {
		wg.Add(1)
		go func() {
			defer wg.Done()
			payload, err := service.Claim(ctx, "1", testUUID(1), "2")
			if err != nil {
				t.Error(err)
			}
			payloads <- payload
		}()
	}
	wg.Wait()
	close(payloads)
	first := ""
	for payload := range payloads {
		if first == "" {
			first = payload
		}
		if first != payload {
			t.Fatal("concurrent retry bytes differ")
		}
	}
	if store.claimCounts["1"] != 64 {
		t.Fatal("retry consumed claim quota")
	}
}

func TestLifetimeLimits(t *testing.T) {
	ctx := context.Background()
	service, store, upload := setup(t, 0)
	for id := 1; id <= MaxPublications; id++ {
		if _, err := service.Publish(ctx, "2", testUUID(id), encodeUpload(t, upload)); err != nil {
			t.Fatalf("publication %d: %v", id, err)
		}
	}
	if _, err := service.Publish(ctx, "2", testUUID(MaxPublications+1), encodeUpload(t, upload)); !errors.Is(err, ErrCapacity) {
		t.Fatalf("publication cap: %v", err)
	}
	if _, err := service.Publish(ctx, "2", testUUID(1), encodeUpload(t, upload)); err != nil {
		t.Fatalf("publication retry at cap: %v", err)
	}
	for id := 1; id <= MaxClaimsPerAccount; id++ {
		if _, err := service.Claim(ctx, "1", testUUID(id), "2"); err != nil {
			t.Fatalf("claim %d: %v", id, err)
		}
	}
	if _, err := service.Claim(ctx, "1", testUUID(MaxClaimsPerAccount+1), "2"); !errors.Is(err, ErrCapacity) {
		t.Fatalf("claim cap: %v", err)
	}
	if _, err := service.Claim(ctx, "1", testUUID(1), "2"); err != nil {
		t.Fatalf("claim retry at cap: %v", err)
	}
	now := time.Now().Add(2 * ClaimTTL)
	if _, err := store.Expire(ctx, now, 1000); err != nil {
		t.Fatal(err)
	}
	if _, err := service.Claim(ctx, "1", testUUID(MaxClaimsPerAccount+1), "2"); !errors.Is(err, ErrCapacity) {
		t.Fatalf("cleanup reset lifetime quota: %v", err)
	}
}

func TestHelpersDoNotMutateInput(t *testing.T) {
	_, _, upload := setup(t, 2)
	// Upload ordering is arbitrary; selection is by the lowest key ID.
	upload.OneTimePrekeys[0], upload.OneTimePrekeys[1] = upload.OneTimePrekeys[1], upload.OneTimePrekeys[0]
	upload.OneTimeKyberPrekeys[0], upload.OneTimeKyberPrekeys[1] = upload.OneTimeKyberPrekeys[1], upload.OneTimeKyberPrekeys[0]
	original := encodeUpload(t, upload)
	inventory, _, err := ApplyPublication(nil, testUUID(1), upload, sha256.Sum256([]byte(original)), time.Now())
	if err != nil {
		t.Fatal(err)
	}
	if encodeUpload(t, upload) != original {
		t.Fatal("publication helper mutated upload slices")
	}
	before := cloneInventory(inventory)
	next, bundle, err := SelectBundle(inventory, time.Now())
	if err != nil || bundle.OneTimePrekey.KeyID != 1 || bundle.KyberPrekey.KeyID != 1 {
		t.Fatalf("selection: %+v %v", bundle, err)
	}
	if !reflect.DeepEqual(inventory, before) || len(next.Upload.OneTimePrekeys) != 1 {
		t.Fatal("selection helper mutated current inventory")
	}
}

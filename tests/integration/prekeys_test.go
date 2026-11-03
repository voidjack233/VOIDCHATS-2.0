package integration

import (
	"bytes"
	"context"
	"crypto/ed25519"
	"crypto/rand"
	"encoding/base64"
	"encoding/json"
	"errors"
	"fmt"
	"strconv"
	"sync"
	"testing"
	"time"

	"github.com/jackc/pgx/v5/pgxpool"
	"void-nothingness/internal/postgres"
	"void-nothingness/internal/prekeys"
)

const prekeyAudience = "https://api.voidchats.xyz/dm9pZG5vdGhpbmduZXNz"

type prekeyAccount struct {
	id      string
	private ed25519.PrivateKey
}

func registerPrekeyAccount(t *testing.T, pool *pgxpool.Pool) prekeyAccount {
	t.Helper()
	public, private, err := ed25519.GenerateKey(rand.Reader)
	if err != nil {
		t.Fatal(err)
	}
	row, err := postgres.New(pool).Insert(context.Background(), freshID(t), public)
	if err != nil {
		t.Fatal(err)
	}
	return prekeyAccount{id: row.VoidID, private: private}
}

// Public test fixtures exercise transport/storage; their placeholder Signal
// signatures are not evidence of native libsignal interoperability.
func prekeyUpload(t *testing.T, account prekeyAccount, count int) (prekeys.Upload, string) {
	t.Helper()
	ec := append([]byte{5}, bytes.Repeat([]byte{42}, 32)...)
	signature := base64.StdEncoding.EncodeToString(bytes.Repeat([]byte{7}, 64))
	kyber := base64.StdEncoding.EncodeToString(bytes.Repeat([]byte{9}, 1568))
	upload := prekeys.Upload{V: 1, DeviceID: 1, RegistrationID: 123,
		IdentityKey:     base64.StdEncoding.EncodeToString(ec),
		SignedPrekey:    prekeys.SignedKey{KeyID: 1, PublicKey: base64.StdEncoding.EncodeToString(ec), Signature: signature},
		KyberLastResort: prekeys.SignedKey{KeyID: 2, PublicKey: kyber, Signature: signature},
		OneTimePrekeys:  []prekeys.PublicKey{}, OneTimeKyberPrekeys: []prekeys.SignedKey{}}
	for i := 0; i < count; i++ {
		upload.OneTimePrekeys = append(upload.OneTimePrekeys, prekeys.PublicKey{KeyID: 3 + i*2, PublicKey: upload.IdentityKey})
		upload.OneTimeKyberPrekeys = append(upload.OneTimeKyberPrekeys, prekeys.SignedKey{KeyID: 4 + i*2, PublicKey: kyber, Signature: signature})
	}
	upload.AccountSignature = base64.StdEncoding.EncodeToString(ed25519.Sign(account.private, prekeys.IdentityMessage(prekeyAudience, account.id, upload)))
	raw, err := json.Marshal(upload)
	if err != nil {
		t.Fatal(err)
	}
	return upload, base64.StdEncoding.EncodeToString(raw)
}

func prekeyID(t *testing.T) string {
	t.Helper()
	var raw [16]byte
	if _, err := rand.Read(raw[:]); err != nil {
		t.Fatal(err)
	}
	raw[6] = (raw[6] & 15) | 64
	raw[8] = (raw[8] & 63) | 128
	return fmt.Sprintf("%x-%x-%x-%x-%x", raw[:4], raw[4:6], raw[6:8], raw[8:10], raw[10:])
}

func decodePrekeyBundle(t *testing.T, encoded string) prekeys.Bundle {
	t.Helper()
	raw, err := base64.StdEncoding.Strict().DecodeString(encoded)
	if err != nil {
		t.Fatal(err)
	}
	var bundle prekeys.Bundle
	if err = json.Unmarshal(raw, &bundle); err != nil {
		t.Fatal(err)
	}
	return bundle
}

func TestPostgresPrekeyPublicationConcurrentClaimsAndFallback(t *testing.T) {
	app, _ := testPools(t)
	ctx, cancel := context.WithTimeout(context.Background(), 20*time.Second)
	defer cancel()
	store := postgres.NewPrekeyStore(app)
	if err := store.Validate(ctx); err != nil {
		t.Fatal(err)
	}
	if err := postgres.NewQueueStore(app).Validate(ctx); err != nil {
		t.Fatal(err)
	}
	alice, bob, other := registerPrekeyAccount(t, app), registerPrekeyAccount(t, app), registerPrekeyAccount(t, app)
	service, err := prekeys.New(store, postgres.New(app), prekeyAudience)
	if err != nil {
		t.Fatal(err)
	}
	_, payload := prekeyUpload(t, bob, 2)
	publication := prekeyID(t)
	if _, err = service.Publish(ctx, alice.id, publication, payload); !errors.Is(err, prekeys.ErrInvalid) {
		t.Fatalf("ownership: %v", err)
	}
	counts, err := service.Publish(ctx, bob.id, publication, payload)
	if err != nil || counts.ECRemaining != 2 || counts.KyberRemaining != 2 {
		t.Fatalf("publication: %+v %v", counts, err)
	}
	request := prekeyID(t)
	first, err := service.Claim(ctx, alice.id, request, bob.id)
	if err != nil {
		t.Fatal(err)
	}
	bundle := decodePrekeyBundle(t, first)
	if bundle.OneTimePrekey == nil || bundle.OneTimePrekey.KeyID != 3 || bundle.KyberPrekey.KeyID != 4 || bundle.KyberPrekey.LastResort || bundle.ClaimExpiresAt.Before(time.Now().Add(23*time.Hour)) {
		t.Fatalf("first bundle: %+v", bundle)
	}
	replacement, err := prekeys.New(postgres.NewPrekeyStore(app), postgres.New(app), prekeyAudience)
	if err != nil {
		t.Fatal(err)
	}
	retry, err := replacement.Claim(ctx, alice.id, request, bob.id)
	if err != nil || retry != first {
		t.Fatalf("persisted retry differed: %v", err)
	}
	if _, err = service.Claim(ctx, alice.id, request, other.id); !errors.Is(err, prekeys.ErrConflict) {
		t.Fatalf("recipient conflict: %v", err)
	}
	counts, err = service.Publish(ctx, bob.id, publication, payload)
	if err != nil || counts.ECRemaining != 1 || counts.KyberRemaining != 1 {
		t.Fatalf("publication retry republished consumed keys: %+v %v", counts, err)
	}
	concurrentID := prekeyID(t)
	results := make(chan string, 8)
	failures := make(chan error, 8)
	var workers sync.WaitGroup
	for range 8 {
		workers.Go(func() {
			value, err := service.Claim(ctx, alice.id, concurrentID, bob.id)
			results <- value
			failures <- err
		})
	}
	workers.Wait()
	close(results)
	close(failures)
	for err := range failures {
		if err != nil {
			t.Fatal(err)
		}
	}
	var same string
	for value := range results {
		if same == "" {
			same = value
		}
		if value != same {
			t.Fatal("concurrent retry selected different bundles")
		}
	}
	selected := decodePrekeyBundle(t, same)
	if selected.OneTimePrekey == nil || selected.OneTimePrekey.KeyID != 5 || selected.KyberPrekey.KeyID != 6 {
		t.Fatal("concurrent claim consumed incorrect key")
	}
	counts, err = service.Publish(ctx, bob.id, publication, payload)
	if err != nil || counts.ECRemaining != 0 || counts.KyberRemaining != 0 {
		t.Fatalf("concurrent inventory: %+v %v", counts, err)
	}
	fallback, err := service.Claim(ctx, alice.id, prekeyID(t), bob.id)
	if err != nil {
		t.Fatal(err)
	}
	last := decodePrekeyBundle(t, fallback)
	if last.OneTimePrekey != nil || !last.KyberPrekey.LastResort || last.KyberPrekey.KeyID != 2 {
		t.Fatal("fallback must stay available")
	}
	if _, err = service.Publish(ctx, bob.id, prekeyID(t), payload); !errors.Is(err, prekeys.ErrConflict) {
		t.Fatalf("old IDs republished: %v", err)
	}
}

func TestPostgresPrekeyExpiryScrubsPublicBundleAndRecipient(t *testing.T) {
	app, _ := testPools(t)
	ctx, cancel := context.WithTimeout(context.Background(), 15*time.Second)
	defer cancel()
	alice, bob := registerPrekeyAccount(t, app), registerPrekeyAccount(t, app)
	_, payload := prekeyUpload(t, bob, 1)
	upload, hash, err := prekeys.ParseUpload(payload)
	if err != nil {
		t.Fatal(err)
	}
	store := postgres.NewPrekeyStore(app)
	old := time.Now().UTC().Truncate(time.Microsecond).Add(-25 * time.Hour)
	if _, err = store.Publish(ctx, bob.id, prekeyID(t), upload, hash, old); err != nil {
		t.Fatal(err)
	}
	request := prekeyID(t)
	if _, err = store.Claim(ctx, alice.id, request, bob.id, old); err != nil {
		t.Fatal(err)
	}
	if _, err = store.Claim(ctx, alice.id, request, bob.id, time.Now()); !errors.Is(err, prekeys.ErrExpired) {
		t.Fatalf("expired retry: %v", err)
	}
	count, err := store.Expire(ctx, time.Now(), 1000)
	if err != nil || count < 1 {
		t.Fatalf("scrub: %d %v", count, err)
	}
	var scrubbed bool
	number, _ := strconv.ParseInt(alice.id, 10, 64)
	if err = app.QueryRow(ctx, `SELECT recipient_void_id IS NULL AND public_bundle IS NULL FROM public.signal_prekey_claims WHERE requester_void_id=$1 AND request_id=$2::uuid`, number, request).Scan(&scrubbed); err != nil || !scrubbed {
		t.Fatalf("retained claim relationship/bundle: %v", err)
	}
	if _, err = store.Claim(ctx, alice.id, request, bob.id, time.Now()); !errors.Is(err, prekeys.ErrExpired) {
		t.Fatalf("tombstone lost: %v", err)
	}
	service, err := prekeys.New(store, postgres.New(app), prekeyAudience)
	if err != nil {
		t.Fatal(err)
	}
	if _, err = service.Claim(ctx, alice.id, request, "9223372036854775807"); !errors.Is(err, prekeys.ErrExpired) {
		t.Fatalf("expired UUID was rebound to an unknown recipient: %v", err)
	}
}

func TestPostgresPrekeyInsertFailureRollsBackConsumption(t *testing.T) {
	app, admin := testPools(t)
	if admin == nil {
		t.Skip("requires disposable administration URL")
	}
	ctx, cancel := context.WithTimeout(context.Background(), 15*time.Second)
	defer cancel()
	alice, bob := registerPrekeyAccount(t, app), registerPrekeyAccount(t, app)
	_, payload := prekeyUpload(t, bob, 1)
	service, err := prekeys.New(postgres.NewPrekeyStore(app), postgres.New(app), prekeyAudience)
	if err != nil {
		t.Fatal(err)
	}
	publication := prekeyID(t)
	if _, err = service.Publish(ctx, bob.id, publication, payload); err != nil {
		t.Fatal(err)
	}
	request := prekeyID(t)
	suffix := strconv.FormatInt(freshID(t), 10)
	function, trigger := "prekey_reject_"+suffix, "prekey_reject_"+suffix
	if _, err = admin.Exec(ctx, `CREATE FUNCTION public.`+function+`() RETURNS trigger LANGUAGE plpgsql AS $$ BEGIN RAISE EXCEPTION 'synthetic claim rejection'; END $$`); err != nil {
		t.Fatal("could not create isolated rollback fixture")
	}
	defer func() { _, _ = admin.Exec(context.Background(), `DROP FUNCTION public.`+function+`() CASCADE`) }()
	if _, err = admin.Exec(ctx, `CREATE TRIGGER `+trigger+` BEFORE INSERT ON public.signal_prekey_claims FOR EACH ROW WHEN (NEW.request_id='`+request+`'::uuid) EXECUTE FUNCTION public.`+function+`() `); err != nil {
		t.Fatal("could not create isolated rollback trigger")
	}
	if _, err = service.Claim(ctx, alice.id, request, bob.id); !errors.Is(err, prekeys.ErrUnavailable) {
		t.Fatalf("failed insert accepted: %v", err)
	}
	counts, err := service.Publish(ctx, bob.id, publication, payload)
	if err != nil || counts.ECRemaining != 1 || counts.KyberRemaining != 1 {
		t.Fatalf("failed insert consumed keys: %+v %v", counts, err)
	}
	if _, err = admin.Exec(ctx, `DROP TRIGGER `+trigger+` ON public.signal_prekey_claims`); err != nil {
		t.Fatal("could not remove isolated rollback trigger")
	}
	if _, err = service.Claim(ctx, alice.id, request, bob.id); err != nil {
		t.Fatal(err)
	}
}

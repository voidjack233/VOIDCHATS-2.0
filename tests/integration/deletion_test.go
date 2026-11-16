package integration

import (
	"context"
	"crypto/ed25519"
	"encoding/base64"
	"errors"
	"strconv"
	"sync"
	"testing"
	"time"

	"github.com/jackc/pgx/v5/pgxpool"
	"void-nothingness/internal/account"
	"void-nothingness/internal/auth"
	"void-nothingness/internal/postgres"
	"void-nothingness/internal/prekeys"
	"void-nothingness/internal/queue"
)

func TestPostgresDeletionLifecycle(t *testing.T) {
	app, _ := testPools(t)
	ctx, cancel := context.WithTimeout(context.Background(), 20*time.Second)
	defer cancel()
	repo := postgres.New(app)
	alice, bob, carol := registerPrekeyAccount(t, app), registerPrekeyAccount(t, app), registerPrekeyAccount(t, app)
	service, _ := account.NewDeletionService(repo, prekeyAudience)
	keys, _ := prekeys.New(postgres.NewPrekeyStore(app), repo, prekeyAudience)
	q, _ := queue.New(postgres.NewQueueStore(app))
	_, payload := prekeyUpload(t, bob, 2)
	if _, err := keys.Publish(ctx, bob.id, prekeyID(t), payload); err != nil {
		t.Fatal(err)
	}
	claimID := prekeyID(t)
	if _, err := keys.Claim(ctx, alice.id, claimID, bob.id); err != nil {
		t.Fatal(err)
	}
	for _, pair := range [][2]string{{alice.id, bob.id}, {bob.id, alice.id}, {alice.id, carol.id}} {
		if _, err := q.Send(ctx, pair[0], prekeyID(t), pair[1], []byte("synthetic ciphertext")); err != nil {
			t.Fatal(err)
		}
	}
	challenge, err := service.Begin(ctx, bob.id)
	if err != nil {
		t.Fatal(err)
	}
	message, _ := base64.StdEncoding.DecodeString(challenge.Message)
	if _, err = service.Confirm(ctx, challenge, challenge.ChallengeID, "DELETE", ed25519.Sign(alice.private, message)); !errors.Is(err, account.ErrInvalidSignature) {
		t.Fatal("unauthorized deletion", err)
	}
	challenge, err = service.Begin(ctx, bob.id)
	if err != nil {
		t.Fatal(err)
	}
	message, _ = base64.StdEncoding.DecodeString(challenge.Message)
	before := time.Now().Add(-time.Second)
	deleted, err := service.Confirm(ctx, challenge, challenge.ChallengeID, "DELETE", ed25519.Sign(bob.private, message))
	if err != nil || deleted.State != "deleted" || deleted.DeletedAt == nil || deleted.DeletedAt.Before(before) || deleted.DeletedAt.After(time.Now().Add(time.Second)) {
		t.Fatal(deleted, err)
	}
	// Re-create every application service over the persisted database. The old
	// deletion ACK can be lost; a new proof must return exactly the same result.
	reconnected, err := pgxpool.NewWithConfig(ctx, app.Config().Copy())
	if err != nil {
		t.Fatal("could not reconnect to disposable fixture")
	}
	defer reconnected.Close()
	restarted := postgres.New(reconnected)
	recovery, _ := account.NewDeletionService(restarted, prekeyAudience)
	c, err := recovery.Begin(ctx, bob.id)
	if err != nil {
		t.Fatal(err)
	}
	message, _ = base64.StdEncoding.DecodeString(c.Message)
	again, err := recovery.Confirm(ctx, c, c.ChallengeID, "DELETE", ed25519.Sign(bob.private, message))
	if err != nil || !again.DeletedAt.Equal(*deleted.DeletedAt) {
		t.Fatal("lost-ACK recovery", again, err)
	}
	if _, err = recovery.Confirm(ctx, c, c.ChallengeID, "DELETE", ed25519.Sign(bob.private, message)); !errors.Is(err, account.ErrInvalidChallenge) {
		t.Fatal("replayed deletion", err)
	}
	for target, want := range map[string]string{alice.id: "active", bob.id: "deleted", strconv.FormatInt(freshID(t), 10): "unknown"} {
		status, err := recovery.Status(ctx, alice.id, target)
		if err != nil || status.State != want {
			t.Fatal(status, err)
		}
	}
	if _, err = q.Lookup(ctx, alice.id, bob.id); !errors.Is(err, account.ErrDeleted) {
		t.Fatal("deleted lookup", err)
	}
	if _, err = q.Send(ctx, alice.id, prekeyID(t), bob.id, []byte{1}); !errors.Is(err, account.ErrDeleted) {
		t.Fatal("deleted recipient", err)
	}
	if _, err = q.Send(ctx, bob.id, prekeyID(t), alice.id, []byte{1}); !errors.Is(err, account.ErrDeleted) {
		t.Fatal("deleted sender", err)
	}
	if _, err = keys.Claim(ctx, alice.id, claimID, bob.id); !errors.Is(err, account.ErrDeleted) {
		t.Fatal("cached claim leaked", err)
	}
	if _, err = postgres.NewPrekeyStore(app).Publish(ctx, bob.id, prekeyID(t), prekeys.Upload{}, [32]byte{}, time.Now()); !errors.Is(err, account.ErrDeleted) {
		t.Fatal("deleted publication", err)
	}
	number, _ := strconv.ParseInt(bob.id, 10, 64)
	if _, err = restarted.AuthenticationKey(ctx, number); !errors.Is(err, auth.ErrNotFound) {
		t.Fatal("deleted auth key returned", err)
	}
	authentication, _ := auth.NewServiceForAudience(restarted, time.Second, prekeyAudience)
	if _, err = authentication.Begin(ctx, bob.id); !errors.Is(err, auth.ErrAuthentication) {
		t.Fatal("deleted authentication", err)
	}
	if _, err = restarted.Insert(ctx, number, freshKey(t)); !errors.Is(err, account.ErrIDCollision) {
		t.Fatal("VOID-ID reused", err)
	}
	if _, err = restarted.Insert(ctx, freshID(t), bob.private.Public().(ed25519.PublicKey)); !errors.Is(err, account.ErrDuplicateKey) {
		t.Fatal("authentication key reused", err)
	}
	var remaining int
	if err = app.QueryRow(ctx, `SELECT (SELECT count(*) FROM public.messages WHERE sender_void_id=$1 OR recipient_void_id=$1)+(SELECT count(*) FROM public.signal_prekey_inventory WHERE void_id=$1)+(SELECT count(*) FROM public.signal_prekey_claims WHERE requester_void_id=$1 OR recipient_void_id=$1)`, number).Scan(&remaining); err != nil || remaining != 0 {
		t.Fatal("deletion did not clean storage", remaining, err)
	}
	var scrubbed bool
	aliceNumber, _ := strconv.ParseInt(alice.id, 10, 64)
	if err = app.QueryRow(ctx, `SELECT recipient_void_id IS NULL AND public_bundle IS NULL FROM public.signal_prekey_claims WHERE requester_void_id=$1 AND request_id=$2::uuid`, aliceNumber, claimID).Scan(&scrubbed); err != nil || !scrubbed {
		t.Fatal("peer claim retry marker was not scrubbed", err)
	}
	rows, err := q.Pending(ctx, carol.id, 10)
	if err != nil || len(rows) != 1 || rows[0].SenderVoidID != alice.id {
		t.Fatal("unrelated queue was changed", rows, err)
	}
	if _, err = q.Lookup(ctx, alice.id, carol.id); err != nil {
		t.Fatal("other account was changed", err)
	}
}

func TestPostgresConcurrentDeletionSendClaimAndPublication(t *testing.T) {
	app, _ := testPools(t)
	ctx, cancel := context.WithTimeout(context.Background(), 30*time.Second)
	defer cancel()
	for range 8 {
		repo := postgres.New(app)
		alice, bob := registerPrekeyAccount(t, app), registerPrekeyAccount(t, app)
		keys, _ := prekeys.New(postgres.NewPrekeyStore(app), repo, prekeyAudience)
		q, _ := queue.New(postgres.NewQueueStore(app))
		_, payload := prekeyUpload(t, bob, 2)
		publication, claim, event := prekeyID(t), prekeyID(t), prekeyID(t)
		if _, err := keys.Publish(ctx, bob.id, publication, payload); err != nil {
			t.Fatal(err)
		}
		start := make(chan struct{})
		results := make(chan error, 5)
		var workers sync.WaitGroup
		for _, operation := range []func() error{
			func() error { _, err := repo.Delete(ctx, bob.id); return err },
			func() error { _, err := repo.Delete(ctx, bob.id); return err },
			func() error { _, err := q.Send(ctx, alice.id, event, bob.id, []byte{1}); return err },
			func() error { _, err := keys.Claim(ctx, alice.id, claim, bob.id); return err },
			func() error { _, err := keys.Publish(ctx, bob.id, publication, payload); return err },
		} {
			workers.Go(func() { <-start; results <- operation() })
		}
		close(start)
		workers.Wait()
		close(results)
		for err := range results {
			if err != nil && !errors.Is(err, account.ErrDeleted) && !errors.Is(err, prekeys.ErrInvalid) {
				t.Fatal("unexpected concurrent result", err)
			}
		}
		status, err := repo.Status(ctx, alice.id, bob.id)
		if err != nil || status.State != "deleted" {
			t.Fatal(status, err)
		}
		if _, err = q.Send(ctx, alice.id, prekeyID(t), bob.id, []byte{1}); !errors.Is(err, account.ErrDeleted) {
			t.Fatal("post-commit send", err)
		}
		if _, err = keys.Claim(ctx, alice.id, claim, bob.id); !errors.Is(err, account.ErrDeleted) {
			t.Fatal("post-commit claim", err)
		}
		var count int
		number, _ := strconv.ParseInt(bob.id, 10, 64)
		if err = app.QueryRow(ctx, `SELECT (SELECT count(*) FROM public.messages WHERE recipient_void_id=$1 OR sender_void_id=$1)+(SELECT count(*) FROM public.signal_prekey_inventory WHERE void_id=$1)+(SELECT count(*) FROM public.signal_prekey_claims WHERE requester_void_id=$1 OR recipient_void_id=$1)`, number).Scan(&count); err != nil || count != 0 {
			t.Fatal("concurrent cleanup", count, err)
		}
	}
}

func TestPostgresDeletionRollbackAndTombstonePermanence(t *testing.T) {
	app, admin := testPools(t)
	if admin == nil {
		t.Skip("disposable schema connection required")
	}
	ctx, cancel := context.WithTimeout(context.Background(), 15*time.Second)
	defer cancel()
	bob, alice := registerPrekeyAccount(t, app), registerPrekeyAccount(t, app)
	repo := postgres.New(app)
	q, _ := queue.New(postgres.NewQueueStore(app))
	if _, err := q.Send(ctx, alice.id, prekeyID(t), bob.id, []byte{1}); err != nil {
		t.Fatal(err)
	}
	number, _ := strconv.ParseInt(bob.id, 10, 64)
	name := "deletion_reject_" + strconv.FormatInt(freshID(t), 10)
	if _, err := admin.Exec(ctx, `CREATE FUNCTION public.`+name+`() RETURNS trigger LANGUAGE plpgsql AS $$ BEGIN RAISE EXCEPTION 'synthetic cleanup failure'; END $$`); err != nil {
		t.Fatal("rollback fixture unavailable")
	}
	defer func() { _, _ = admin.Exec(context.Background(), `DROP FUNCTION public.`+name+`() CASCADE`) }()
	if _, err := admin.Exec(ctx, `CREATE TRIGGER `+name+` BEFORE DELETE ON public.messages FOR EACH ROW WHEN (OLD.recipient_void_id=`+bob.id+`) EXECUTE FUNCTION public.`+name+`() `); err != nil {
		t.Fatal("rollback trigger unavailable")
	}
	if _, err := repo.Delete(ctx, bob.id); !errors.Is(err, account.ErrUnavailable) {
		t.Fatal("failed cleanup acknowledged", err)
	}
	status, err := repo.Status(ctx, alice.id, bob.id)
	if err != nil || status.State != "active" {
		t.Fatal("partial tombstone committed", status, err)
	}
	if _, err = admin.Exec(ctx, `DROP TRIGGER `+name+` ON public.messages`); err != nil {
		t.Fatal("could not remove synthetic trigger")
	}
	if _, err = repo.Delete(ctx, bob.id); err != nil {
		t.Fatal(err)
	}
	for _, query := range []string{
		`UPDATE public.accounts SET deleted_at=NULL WHERE void_id=$1`,
		`UPDATE public.accounts SET deleted_at=clock_timestamp() WHERE void_id=$1`,
		`UPDATE public.accounts SET void_id=void_id+1 WHERE void_id=$1`,
		`DELETE FROM public.accounts WHERE void_id=$1`,
	} {
		if _, err = admin.Exec(ctx, query, number); err == nil {
			t.Fatal("tombstone mutation succeeded", query)
		}
	}
	// TRUNCATE is checked inside a transaction so even a broken trigger cannot
	// erase another fixture. No test ever targets an existing production table.
	tx, err := admin.Begin(ctx)
	if err != nil {
		t.Fatal(err)
	}
	_, err = tx.Exec(ctx, `TRUNCATE public.accounts CASCADE`)
	_ = tx.Rollback(ctx)
	if err == nil {
		t.Fatal("tombstones could be truncated")
	}
}

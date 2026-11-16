package relay

import (
	"context"
	"crypto/ed25519"
	"encoding/base64"
	"strconv"
	"sync"
	"testing"
	"time"

	"github.com/coder/websocket"
	"void-nothingness/internal/account"
	"void-nothingness/internal/auth"
	"void-nothingness/internal/queue"
)

// Models durable lifecycle state independently of connections and services.
// Transactional storage/cleanup is verified against PostgreSQL in integration.
type lifecycleFixture struct {
	mu      sync.Mutex
	keys    map[string]ed25519.PublicKey
	deleted map[string]*time.Time
}

func (f *lifecycleFixture) DeletionKey(_ context.Context, id string) ([]byte, error) {
	f.mu.Lock()
	defer f.mu.Unlock()
	if key, ok := f.keys[id]; ok {
		return key, nil
	}
	return nil, account.ErrInvalidSignature
}
func (f *lifecycleFixture) AuthenticationKey(_ context.Context, id int64) ([]byte, error) {
	f.mu.Lock()
	defer f.mu.Unlock()
	s := strconv.FormatInt(id, 10)
	if f.deleted[s] != nil {
		return nil, auth.ErrNotFound
	}
	if key, ok := f.keys[s]; ok {
		return key, nil
	}
	return nil, auth.ErrNotFound
}
func (f *lifecycleFixture) Delete(ctx context.Context, id string) (account.Status, error) {
	f.mu.Lock()
	defer f.mu.Unlock()
	if ctx.Err() != nil {
		return account.Status{}, ctx.Err()
	}
	if f.deleted[id] == nil {
		now := time.Now().UTC()
		f.deleted[id] = &now
	}
	return account.Status{VoidID: id, State: "deleted", DeletedAt: f.deleted[id]}, nil
}
func (f *lifecycleFixture) Status(_ context.Context, actor, target string) (account.Status, error) {
	f.mu.Lock()
	defer f.mu.Unlock()
	if f.deleted[actor] != nil {
		return account.Status{}, account.ErrDeleted
	}
	status := account.Status{VoidID: target, State: "active"}
	if f.keys[target] == nil {
		status.State = "unknown"
	}
	if f.deleted[target] != nil {
		status.State = "deleted"
		status.DeletedAt = f.deleted[target]
	}
	return status, nil
}
func (f *lifecycleFixture) WithActive(ctx context.Context, ids []string, fn func() error) error {
	f.mu.Lock()
	defer f.mu.Unlock()
	if ctx.Err() != nil {
		return ctx.Err()
	}
	for _, id := range ids {
		if f.deleted[id] != nil {
			return account.ErrDeleted
		}
		if f.keys[id] == nil {
			return queue.ErrNotFound
		}
	}
	return fn()
}

func lifecycleRelay(t *testing.T) (*fixture, *lifecycleFixture) {
	t.Helper()
	f := newFixture(t, Options{}, 3)
	store := &lifecycleFixture{keys: map[string]ed25519.PublicKey{}, deleted: map[string]*time.Time{}}
	for id, key := range f.keys {
		store.keys[id] = key.Public().(ed25519.PublicKey)
	}
	service, err := account.NewDeletionService(store, "https://api.example.test/backend")
	if err != nil {
		t.Fatal(err)
	}
	f.server.opts.Accounts = service
	f.server.auth, err = auth.NewServiceForAudience(store, time.Second, "https://api.example.test/backend")
	if err != nil {
		t.Fatal(err)
	}
	return f, store
}
func beginDelete(t *testing.T, conn *websocket.Conn, id string) map[string]string {
	t.Helper()
	write(t, conn, map[string]string{"type": "account_delete_begin", "void_id": id})
	c := read(t, conn)
	if c["type"] != "account_delete_challenge" || len(c) != 4 {
		t.Fatal(c)
	}
	return c
}
func confirmDelete(t *testing.T, conn *websocket.Conn, c map[string]string, key ed25519.PrivateKey, confirmation string) {
	t.Helper()
	raw, err := base64.StdEncoding.Strict().DecodeString(c["message"])
	if err != nil {
		t.Fatal(err)
	}
	write(t, conn, map[string]string{"type": "account_delete_confirm", "challenge_id": c["challenge_id"], "confirmation": confirmation, "signature": base64.StdEncoding.EncodeToString(ed25519.Sign(key, raw))})
}

func TestDeletionFramesStatusAndSessionRevocation(t *testing.T) {
	f, _ := lifecycleRelay(t)
	alice, bob := f.authenticate(t, "1"), f.authenticate(t, "2")
	write(t, alice, map[string]string{"type": "account_status", "request_id": "status:1", "void_id": "2"})
	assertFields(t, read(t, alice), map[string]string{"type": "account_status", "request_id": "status:1", "void_id": "2", "state": "active"})
	// A deletion-only proof authenticates the owner and revokes the existing
	// relay connection, without authenticating the new socket for relay use.
	deletion := f.dial(t)
	c := beginDelete(t, deletion, "2")
	confirmDelete(t, deletion, c, f.keys["2"], "DELETE")
	ack := read(t, deletion)
	if ack["type"] != "account_deleted" || ack["void_id"] != "2" || ack["deleted_at"] == "" || len(ack) != 3 {
		t.Fatal(ack)
	}
	ctx, cancel := context.WithTimeout(context.Background(), time.Second)
	defer cancel()
	if _, _, err := bob.Read(ctx); err == nil {
		t.Fatal("deleted relay session survived")
	}
	write(t, alice, map[string]string{"type": "account_status", "request_id": "status:2", "void_id": "2"})
	assertFields(t, read(t, alice), map[string]string{"type": "account_status", "request_id": "status:2", "void_id": "2", "state": "deleted", "deleted_at": ack["deleted_at"]})
	write(t, alice, map[string]string{"type": "account_status", "request_id": "status:3", "void_id": "999"})
	assertFields(t, read(t, alice), map[string]string{"type": "account_status", "request_id": "status:3", "void_id": "999", "state": "unknown"})
	failed := f.dial(t)
	write(t, failed, map[string]string{"type": "auth_begin", "void_id": "2"})
	assertFields(t, read(t, failed), map[string]string{"type": "error", "code": "authentication_failed"})
	retry := f.dial(t)
	c = beginDelete(t, retry, "2")
	confirmDelete(t, retry, c, f.keys["2"], "DELETE")
	assertFields(t, read(t, retry), ack)
}

func TestDeletionRejectsOtherOwnerWrongConfirmationAndCrossSocketReplay(t *testing.T) {
	f, store := lifecycleRelay(t)
	alice := f.authenticate(t, "1")
	write(t, alice, map[string]string{"type": "account_delete_begin", "void_id": "2"})
	assertFields(t, read(t, alice), map[string]string{"type": "error", "code": "unauthorized"})
	for _, confirmation := range []string{"delete", "DELETE"} {
		conn := f.dial(t)
		c := beginDelete(t, conn, "2")
		key := f.keys["2"]
		if confirmation == "DELETE" {
			key = f.keys["1"]
		}
		confirmDelete(t, conn, c, key, confirmation)
		assertFields(t, read(t, conn), map[string]string{"type": "error", "code": "deletion_authorization_failed"})
	}
	first, second := f.dial(t), f.dial(t)
	original := beginDelete(t, first, "2")
	_ = beginDelete(t, second, "2")
	confirmDelete(t, second, original, f.keys["2"], "DELETE")
	assertFields(t, read(t, second), map[string]string{"type": "error", "code": "deletion_authorization_failed"})
	status, err := store.Status(context.Background(), "1", "2")
	if err != nil || status.State != "active" {
		t.Fatal(status, err)
	}
	confirmDelete(t, first, original, f.keys["2"], "DELETE")
	if ack := read(t, first); ack["type"] != "account_deleted" {
		t.Fatal(ack)
	}
}

func TestAuthenticatedSelfDeletionAndDisconnectPreservesAccount(t *testing.T) {
	f, store := lifecycleRelay(t)
	conn := f.authenticate(t, "1")
	_ = conn.CloseNow()
	status, err := store.Status(context.Background(), "2", "1")
	if err != nil || status.State != "active" {
		t.Fatal("disconnect deleted account", status, err)
	}
	conn = f.authenticate(t, "1")
	c := beginDelete(t, conn, "1")
	confirmDelete(t, conn, c, f.keys["1"], "DELETE")
	if ack := read(t, conn); ack["type"] != "account_deleted" {
		t.Fatal(ack)
	}
	eventually(t, "deletion session remained active", func() bool { return f.server.ActiveCount() == 0 })
}

func TestLostDeletionAcknowledgmentCanBeRecovered(t *testing.T) {
	f, store := lifecycleRelay(t)
	conn := f.dial(t)
	c := beginDelete(t, conn, "2")
	confirmDelete(t, conn, c, f.keys["2"], "DELETE")
	// Never read the acknowledgment. Observe the independent server fixture,
	// drop the transport, and recover with a fresh operation-bound proof.
	eventually(t, "deletion did not commit", func() bool {
		status, err := store.Status(context.Background(), "1", "2")
		return err == nil && status.State == "deleted"
	})
	_ = conn.CloseNow()
	retry := f.dial(t)
	c = beginDelete(t, retry, "2")
	confirmDelete(t, retry, c, f.keys["2"], "DELETE")
	ack := read(t, retry)
	status, err := store.Status(context.Background(), "1", "2")
	if err != nil || ack["type"] != "account_deleted" || ack["deleted_at"] != status.DeletedAt.Format(time.RFC3339Nano) {
		t.Fatal("uncertain acknowledgment could not be reconciled", ack, err)
	}
}

func TestDeletedSenderDoesNotEmitUnsolicitedRecipientError(t *testing.T) {
	f, store := lifecycleRelay(t)
	if _, err := f.server.opts.Queue.Send(context.Background(), "1", firstEventID, "2", []byte{1}); err != nil {
		t.Fatal(err)
	}
	if _, err := store.Delete(context.Background(), "1"); err != nil {
		t.Fatal(err)
	}
	ctx, cancel := context.WithCancel(context.Background())
	defer cancel()
	client := &session{server: f.server, ctx: ctx, cancel: cancel, voidID: "2", out: make(chan outbound, 2)}
	if !client.deliverOne() || client.inFlightID != "" || len(client.out) != 0 || ctx.Err() != nil {
		t.Fatal("stale delivery interrupted an unrelated recipient operation")
	}
}

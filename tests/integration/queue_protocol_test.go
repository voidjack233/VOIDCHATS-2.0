package integration

import (
	"bytes"
	"context"
	"errors"
	"strconv"
	"testing"
	"time"

	"github.com/jackc/pgx/v5/pgconn"
	"void-nothingness/internal/postgres"
	"void-nothingness/internal/queue"
)

func TestPostgresQueueProtocolMetadataAndRecovery(t *testing.T) {
	app, _ := testPools(t)
	ctx, cancel := context.WithTimeout(context.Background(), 20*time.Second)
	defer cancel()
	store := postgres.NewQueueStore(app)
	if err := store.Validate(ctx); err != nil {
		t.Fatal(err)
	}
	alice, bob, other := registerPrekeyAccount(t, app), registerPrekeyAccount(t, app), registerPrekeyAccount(t, app)
	service, err := queue.New(store)
	if err != nil {
		t.Fatal(err)
	}
	payload := []byte{0, 255, 128, 1, 2}
	first, err := service.Send(ctx, alice.id, prekeyID(t), bob.id, payload)
	if err != nil || first.MinProtocolVersion != 1 {
		t.Fatalf("legacy send: %+v %v", first, err)
	}
	second, err := service.SendVersioned(ctx, alice.id, prekeyID(t), bob.id, payload, 2)
	if err != nil {
		t.Fatal(err)
	}
	third, err := service.Send(ctx, alice.id, prekeyID(t), bob.id, []byte{3})
	if err != nil {
		t.Fatal(err)
	}
	if _, err = service.Send(ctx, alice.id, second.EventID, bob.id, []byte{9}); !errors.Is(err, queue.ErrDuplicate) {
		t.Fatalf("duplicate overwrote versioned event: %v", err)
	}
	if err = service.Ack(ctx, other.id, alice.id, second.EventID); err != nil {
		t.Fatal(err)
	}
	// Recreate both the service and adapter without changing persisted rows.
	restarted, err := queue.New(postgres.NewQueueStore(app))
	if err != nil {
		t.Fatal(err)
	}
	rows, err := restarted.Pending(ctx, bob.id, queue.MaxBatch)
	if err != nil || len(rows) != 3 {
		t.Fatalf("persisted queue: %+v %v", rows, err)
	}
	for i, want := range []queue.Event{first, second, third} {
		if rows[i].EventID != want.EventID || rows[i].MinProtocolVersion != want.MinProtocolVersion ||
			!bytes.Equal(rows[i].Payload, want.Payload) || !rows[i].CreatedAt.Equal(want.CreatedAt) ||
			!rows[i].ExpiresAt.Equal(want.ExpiresAt) {
			t.Fatalf("queue order or immutable data changed at %d: %+v", i, rows[i])
		}
	}
	for range 2 {
		if err = restarted.Ack(ctx, bob.id, alice.id, first.EventID); err != nil {
			t.Fatal(err)
		}
	}
	rows, err = restarted.Pending(ctx, bob.id, 1)
	if err != nil || len(rows) != 1 || rows[0].EventID != second.EventID || rows[0].MinProtocolVersion != 2 {
		t.Fatalf("head must retain incompatible event ahead of later legacy ciphertext: %+v %v", rows, err)
	}
	if err = restarted.Ack(ctx, bob.id, alice.id, second.EventID); err != nil {
		t.Fatal(err)
	}
	rows, err = restarted.Pending(ctx, bob.id, 1)
	if err != nil || len(rows) != 1 || rows[0].EventID != third.EventID {
		t.Fatalf("ACK did not release next event: %+v %v", rows, err)
	}

	created := time.Now().UTC().Truncate(time.Microsecond).Add(-31 * 24 * time.Hour)
	expired := queue.Event{EventID: prekeyID(t), SenderVoidID: alice.id, RecipientVoidID: bob.id,
		Payload: payload, CreatedAt: created, ExpiresAt: created.Add(queue.Retention), MinProtocolVersion: 2}
	if err = store.Enqueue(ctx, expired); err != nil {
		t.Fatal(err)
	}
	if count, err := restarted.Cleanup(ctx, 1000); err != nil || count < 1 {
		t.Fatalf("versioned expiration: %d %v", count, err)
	}
	var remaining int
	sender, _ := strconv.ParseInt(alice.id, 10, 64)
	if err = app.QueryRow(ctx, `SELECT count(*) FROM public.messages WHERE sender_void_id=$1 AND event_id=$2::uuid`, sender, expired.EventID).Scan(&remaining); err != nil || remaining != 0 {
		t.Fatalf("expired versioned ciphertext remains: %d %v", remaining, err)
	}
}

func TestPostgresQueueProtocolDefaultAndConstraints(t *testing.T) {
	app, _ := testPools(t)
	ctx, cancel := context.WithTimeout(context.Background(), 15*time.Second)
	defer cancel()
	alice, bob := registerPrekeyAccount(t, app), registerPrekeyAccount(t, app)
	sender, _ := strconv.ParseInt(alice.id, 10, 64)
	recipient, _ := strconv.ParseInt(bob.id, 10, 64)
	created := time.Now().UTC().Truncate(time.Microsecond)
	legacyID := prekeyID(t)
	if _, err := app.Exec(ctx, `INSERT INTO public.messages(event_id,sender_void_id,recipient_void_id,payload,created_at,expires_at) VALUES($1::uuid,$2,$3,$4,$5,$6)`,
		legacyID, sender, recipient, []byte{0, 255, 128}, created, created.Add(queue.Retention)); err != nil {
		t.Fatal(err)
	}
	rows, err := postgres.NewQueueStore(app).Pending(ctx, bob.id, time.Now(), 10)
	if err != nil || len(rows) != 1 || rows[0].EventID != legacyID || rows[0].MinProtocolVersion != 1 || !bytes.Equal(rows[0].Payload, []byte{0, 255, 128}) {
		t.Fatalf("SQL legacy insert default: %+v %v", rows, err)
	}
	for _, test := range []struct {
		name    string
		version any
		code    string
	}{
		{"zero", int64(0), "23514"},
		{"negative", int64(-1), "23514"},
		{"null", nil, "23502"},
		{"overflow", int64(2147483648), "22003"},
	} {
		t.Run(test.name, func(t *testing.T) {
			_, err := app.Exec(ctx, `INSERT INTO public.messages(event_id,sender_void_id,recipient_void_id,payload,created_at,expires_at,min_protocol_version) VALUES($1::uuid,$2,$3,$4,$5,$6,$7::bigint)`,
				prekeyID(t), sender, recipient, []byte{1}, created, created.Add(queue.Retention), test.version)
			var databaseError *pgconn.PgError
			if !errors.As(err, &databaseError) || databaseError.Code != test.code {
				t.Fatalf("invalid protocol version accepted or unexpected constraint: %v", err)
			}
		})
	}
}

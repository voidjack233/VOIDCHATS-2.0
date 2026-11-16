package postgres

import (
	"context"
	"errors"
	"strings"
	"testing"
	"time"

	"github.com/jackc/pgx/v5"
	"void-nothingness/internal/account"
)

func TestDeletionCommitsTombstoneAndCleanupTogether(t *testing.T) {
	stamp := time.Now().UTC()
	tx := &queueTestTx{queueTestDB: queueTestDB{rowResults: []pgx.Row{queueRow((*time.Time)(nil)), queueRow(&stamp), queueRow(true)}}}
	result, err := deleteAccount(context.Background(), &queueTestDB{tx: tx}, "101")
	if err != nil || !tx.committed || !tx.rolledBack || result.State != "deleted" || !result.DeletedAt.Equal(stamp) {
		t.Fatal(result, err)
	}
	queries := []string{}
	for _, call := range tx.calls {
		queries = append(queries, call.sql)
	}
	joined := strings.Join(queries, "\n")
	for _, required := range []string{"pg_advisory_xact_lock", "SET deleted_at=clock_timestamp()", "DELETE FROM public.messages WHERE sender_void_id=$1 OR recipient_void_id=$1", "DELETE FROM public.signal_prekey_inventory", "DELETE FROM public.signal_prekey_claims WHERE requester_void_id=$1", "SET recipient_void_id=NULL,public_bundle=NULL WHERE recipient_void_id=$1"} {
		if !strings.Contains(joined, required) {
			t.Fatal("missing atomic cleanup", required)
		}
	}
}

func TestDuplicateDeletionReturnsOriginalTombstone(t *testing.T) {
	stamp := time.Now().UTC().Add(-time.Hour)
	tx := &queueTestTx{queueTestDB: queueTestDB{rowResults: []pgx.Row{queueRow(&stamp)}}}
	result, err := deleteAccount(context.Background(), &queueTestDB{tx: tx}, "101")
	if err != nil || !tx.committed || !result.DeletedAt.Equal(stamp) || len(tx.calls) != 2 {
		t.Fatal(result, err, tx.calls)
	}
}

func TestDeletionCommitFailureNeverAcknowledges(t *testing.T) {
	stamp := time.Now().UTC()
	secret := errors.New("private SQL credentials")
	tx := &queueTestTx{queueTestDB: queueTestDB{rowResults: []pgx.Row{queueRow((*time.Time)(nil)), queueRow(&stamp), queueRow(false)}}, commitErr: secret}
	result, err := deleteAccount(context.Background(), &queueTestDB{tx: tx}, "101")
	if !errors.Is(err, account.ErrUnavailable) || errors.Is(err, secret) || result.DeletedAt != nil || tx.committed || !tx.rolledBack {
		t.Fatal(result, err)
	}
}

func TestDeletedAccountsRejectQueueAndPrekeysBeforeMutation(t *testing.T) {
	stamp := time.Now()
	t.Run("send", func(t *testing.T) {
		s, tx := queueEnqueueStore()
		tx.rowsResults = []pgx.Rows{queueRows([]any{int64(101), &stamp}, []any{int64(9223372036854775807), (*time.Time)(nil)})}
		if err := s.Enqueue(context.Background(), queueSampleEvent()); !errors.Is(err, account.ErrDeleted) || tx.committed {
			t.Fatal(err)
		}
	})
	t.Run("lookup", func(t *testing.T) {
		s := &QueueStore{db: &queueTestDB{rowResults: []pgx.Row{queueRow(int64(101), []byte("must not escape"), &stamp)}}}
		person, err := s.Lookup(context.Background(), "101")
		if !errors.Is(err, account.ErrDeleted) || person.AuthPublicKey != "" {
			t.Fatal(person, err)
		}
	})
	for _, op := range []string{"publish", "claim", "cached_claim", "ack", "pending"} {
		t.Run(op, func(t *testing.T) {
			prekeys, tx := prekeyTestStore()
			tx.accounts[101] = &stamp
			var err error
			switch op {
			case "publish":
				_, err = prekeys.Publish(context.Background(), "101", prekeyTestPublication, prekeySampleUpload(), [32]byte{}, time.Now())
			case "claim", "cached_claim":
				_, err = prekeys.Claim(context.Background(), "102", prekeyTestRequest, "101", time.Now())
			case "ack":
				err = (&QueueStore{db: &queueTestDB{tx: tx}}).Ack(context.Background(), "101", "102", prekeyTestRequest)
			case "pending":
				_, err = (&QueueStore{db: &queueTestDB{tx: tx}}).Pending(context.Background(), "101", time.Now(), 1)
			}
			if !errors.Is(err, account.ErrDeleted) || tx.committed {
				t.Fatal(err)
			}
			for _, call := range tx.calls {
				if strings.HasPrefix(call.sql, "INSERT") || strings.HasPrefix(call.sql, "UPDATE") || strings.HasPrefix(call.sql, "DELETE") {
					t.Fatal("deleted operation mutated storage", call)
				}
			}
		})
	}
}

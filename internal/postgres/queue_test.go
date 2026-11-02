package postgres

import (
	"bytes"
	"context"
	"errors"
	"reflect"
	"strings"
	"testing"
	"time"

	"github.com/jackc/pgx/v5"
	"github.com/jackc/pgx/v5/pgconn"
	"github.com/jackc/pgx/v5/pgtype"
	"void-nothingness/internal/queue"
)

type queueTestCall struct {
	sql  string
	args []any
}
type queueTestRow func(...any) error

func (r queueTestRow) Scan(dest ...any) error { return r(dest...) }
func queueRow(values ...any) pgx.Row {
	return queueTestRow(func(dest ...any) error {
		if len(dest) != len(values) {
			return errors.New("wrong mock column count")
		}
		for i, value := range values {
			reflect.ValueOf(dest[i]).Elem().Set(reflect.ValueOf(value))
		}
		return nil
	})
}

type queueTestRows struct {
	pgx.Rows
	values [][]any
	index  int
	closed bool
	err    error
}

func queueRows(values ...[]any) *queueTestRows  { return &queueTestRows{values: values, index: -1} }
func (r *queueTestRows) Close()                 { r.closed = true }
func (r *queueTestRows) Err() error             { return r.err }
func (r *queueTestRows) Next() bool             { r.index++; return !r.closed && r.index < len(r.values) }
func (r *queueTestRows) Scan(dest ...any) error { return queueRow(r.values[r.index]...).Scan(dest...) }

type queueTestDB struct {
	rowResults                  []pgx.Row
	rowsResults                 []pgx.Rows
	calls                       []queueTestCall
	tx                          *queueTestTx
	beginErr, execErr, queryErr error
	tag                         pgconn.CommandTag
	beginCount                  int
}

func (db *queueTestDB) BeginTx(_ context.Context, opts pgx.TxOptions) (pgx.Tx, error) {
	db.beginCount++
	if opts.IsoLevel != pgx.ReadCommitted {
		panic("unexpected isolation")
	}
	return db.tx, db.beginErr
}
func (db *queueTestDB) Exec(_ context.Context, sql string, args ...any) (pgconn.CommandTag, error) {
	db.calls = append(db.calls, queueTestCall{sql, args})
	return db.tag, db.execErr
}
func (db *queueTestDB) QueryRow(_ context.Context, sql string, args ...any) pgx.Row {
	db.calls = append(db.calls, queueTestCall{sql, args})
	if len(db.rowResults) == 0 {
		return queueTestRow(func(...any) error { return errors.New("unexpected mock query") })
	}
	row := db.rowResults[0]
	db.rowResults = db.rowResults[1:]
	return row
}
func (db *queueTestDB) Query(_ context.Context, sql string, args ...any) (pgx.Rows, error) {
	db.calls = append(db.calls, queueTestCall{sql, args})
	if db.queryErr != nil {
		return nil, db.queryErr
	}
	if len(db.rowsResults) == 0 {
		return nil, errors.New("unexpected mock query")
	}
	rows := db.rowsResults[0]
	db.rowsResults = db.rowsResults[1:]
	return rows, nil
}

type queueTestTx struct {
	pgx.Tx
	queueTestDB
	committed, rolledBack bool
	commitErr             error
	insertErr             error
}

func (tx *queueTestTx) Query(ctx context.Context, sql string, args ...any) (pgx.Rows, error) {
	return tx.queueTestDB.Query(ctx, sql, args...)
}
func (tx *queueTestTx) QueryRow(ctx context.Context, sql string, args ...any) pgx.Row {
	return tx.queueTestDB.QueryRow(ctx, sql, args...)
}

func (tx *queueTestTx) Exec(ctx context.Context, sql string, args ...any) (pgconn.CommandTag, error) {
	if strings.HasPrefix(sql, "INSERT ") && tx.insertErr != nil {
		tx.calls = append(tx.calls, queueTestCall{sql, args})
		return pgconn.CommandTag{}, tx.insertErr
	}
	return tx.queueTestDB.Exec(ctx, sql, args...)
}
func (tx *queueTestTx) Commit(context.Context) error {
	tx.committed = tx.commitErr == nil
	return tx.commitErr
}
func (tx *queueTestTx) Rollback(context.Context) error { tx.rolledBack = true; return nil }

func queueSampleEvent() queue.Event {
	now := time.Date(2026, 11, 2, 1, 0, 0, 123456000, time.UTC)
	return queue.Event{EventID: "00000000-0000-4000-8000-000000000001", SenderVoidID: "9223372036854775807", RecipientVoidID: "101", Payload: []byte{0, 255, 1, 2}, CreatedAt: now, ExpiresAt: now.Add(queue.Retention)}
}
func queueEnqueueStore() (*QueueStore, *queueTestTx) {
	tx := &queueTestTx{queueTestDB: queueTestDB{
		rowsResults: []pgx.Rows{queueRows([]any{int64(101)}, []any{int64(9223372036854775807)})},
		rowResults:  []pgx.Row{queueRow(false), queueRow(int64(0), int64(0)), queueRow(int64(0), int64(0))},
		tag:         pgconn.NewCommandTag("INSERT 0 1"),
	}}
	return &QueueStore{db: &queueTestDB{tx: tx}}, tx
}

func TestQueueEnqueueLocksAffectedAccountsAndCommitsBeforeAcceptance(t *testing.T) {
	s, tx := queueEnqueueStore()
	event := queueSampleEvent()
	if err := s.Enqueue(context.Background(), event); err != nil {
		t.Fatal(err)
	}
	if !tx.committed || !tx.rolledBack {
		t.Fatal("enqueue did not commit/finish transaction")
	}
	if len(tx.calls) != 9 {
		t.Fatalf("unexpected calls: %#v", tx.calls)
	}
	for i, keys := range [][]any{{int32(0), int32(101)}, {int32(2147483647), int32(-1)}} {
		lock := tx.calls[2+i]
		if lock.sql != "SELECT pg_advisory_xact_lock($1::integer,$2::integer)" || !reflect.DeepEqual(lock.args, keys) {
			t.Fatal("account lock is not sorted, collision-free or isolated from bigint lock space", lock)
		}
	}
	accounts := tx.calls[4]
	if !strings.Contains(accounts.sql, "ORDER BY void_id") || strings.Contains(accounts.sql, "FOR ") || !reflect.DeepEqual(accounts.args[0], []int64{101, 9223372036854775807}) {
		t.Fatal("account existence check requires mutation privileges or changed IDs", accounts)
	}
	if !strings.Contains(tx.calls[6].sql, "WHERE sender_void_id=$1 AND expires_at>$2") || tx.calls[6].args[0] != int64(9223372036854775807) || tx.calls[6].args[1] != event.CreatedAt {
		t.Fatal("sender quota is not scoped to pending events")
	}
	if !strings.Contains(tx.calls[7].sql, "WHERE recipient_void_id=$1 AND expires_at>$2") || tx.calls[7].args[0] != int64(101) {
		t.Fatal("recipient quota is not scoped")
	}
	insert := tx.calls[8]
	if !strings.HasPrefix(insert.sql, "INSERT INTO public.messages(event_id,sender_void_id,recipient_void_id,payload,created_at,expires_at)") || !strings.Contains(insert.sql, "ON CONFLICT(sender_void_id,event_id) DO NOTHING") {
		t.Fatal(insert.sql)
	}
	if insert.args[0] != queueUUID(event.EventID) || insert.args[1] != int64(9223372036854775807) || insert.args[2] != int64(101) || !bytes.Equal(insert.args[3].([]byte), event.Payload) || insert.args[4] != event.CreatedAt || insert.args[5] != event.ExpiresAt {
		t.Fatal("event data or typed parameters changed")
	}
	encoded, err := pgtype.NewMap().Encode(pgtype.UUIDOID, pgtype.BinaryFormatCode, insert.args[0], nil)
	if err != nil || len(encoded) != 16 {
		t.Fatal("UUID not binary-safe", err)
	}
}

func TestQueueEnqueueFailuresNeverAcknowledgePersistence(t *testing.T) {
	secret := errors.New("private database URL and ciphertext")
	for _, tc := range []struct {
		name   string
		change func(*queueTestTx)
		want   error
	}{
		{"missing_recipient", func(tx *queueTestTx) { tx.rowsResults = []pgx.Rows{queueRows([]any{int64(101)})} }, queue.ErrNotFound},
		{"duplicate", func(tx *queueTestTx) { tx.rowResults[0] = queueRow(true) }, queue.ErrDuplicate},
		{"sender_count_limit", func(tx *queueTestTx) { tx.rowResults[1] = queueRow(int64(queue.MaxPendingPerAccount), int64(0)) }, queue.ErrCapacity},
		{"recipient_byte_limit", func(tx *queueTestTx) { tx.rowResults[2] = queueRow(int64(1), int64(queue.MaxPendingBytesPerAccount)) }, queue.ErrCapacity},
		{"insert_failure", func(tx *queueTestTx) { tx.insertErr = secret }, queue.ErrUnavailable},
		{"insert_conflict", func(tx *queueTestTx) { tx.tag = pgconn.NewCommandTag("INSERT 0 0") }, queue.ErrDuplicate},
		{"commit_failure", func(tx *queueTestTx) { tx.commitErr = secret }, queue.ErrUnavailable},
	} {
		t.Run(tc.name, func(t *testing.T) {
			s, tx := queueEnqueueStore()
			tc.change(tx)
			err := s.Enqueue(context.Background(), queueSampleEvent())
			if !errors.Is(err, tc.want) || errors.Is(err, secret) || tx.committed || !tx.rolledBack {
				t.Fatal(err, tx.committed, tx.rolledBack)
			}
		})
	}
}

func TestQueueSelfDeliveryLocksOneAccount(t *testing.T) {
	s, tx := queueEnqueueStore()
	event := queueSampleEvent()
	event.SenderVoidID = "101"
	tx.rowsResults = []pgx.Rows{queueRows([]any{int64(101)})}
	if err := s.Enqueue(context.Background(), event); err != nil {
		t.Fatal(err)
	}
	if len(tx.calls) != 8 || tx.calls[2].sql != "SELECT pg_advisory_xact_lock($1::integer,$2::integer)" || !reflect.DeepEqual(tx.calls[2].args, []any{int32(0), int32(101)}) || !reflect.DeepEqual(tx.calls[3].args[0], []int64{101}) {
		t.Fatal("self delivery duplicated account lock")
	}
}

func TestQueueLookupUsesRegisteredKeyWithoutUsernameRequirement(t *testing.T) {
	key := bytes.Repeat([]byte{17}, 32)
	db := &queueTestDB{rowResults: []pgx.Row{queueRow(int64(101), key)}}
	store := &QueueStore{db: db}
	person, err := store.Lookup(context.Background(), "101")
	if err != nil || person.VoidID != "101" || person.AuthPublicKey == "" {
		t.Fatal(person, err)
	}
	if db.calls[0].sql != "SELECT void_id,auth_public_key FROM public.accounts WHERE void_id=$1" || db.calls[0].args[0] != int64(101) {
		t.Fatal("lookup changed account identity or depends on username")
	}
	db.rowResults = []pgx.Row{queueTestRow(func(...any) error { return pgx.ErrNoRows })}
	if _, err := store.Lookup(context.Background(), "102"); !errors.Is(err, queue.ErrNotFound) {
		t.Fatal("missing account lookup:", err)
	}
}

func TestQueueFailedBeginDoesNotAcknowledge(t *testing.T) {
	db := &queueTestDB{beginErr: errors.New("private database detail")}
	store := &QueueStore{db: db}
	if err := store.Enqueue(context.Background(), queueSampleEvent()); !errors.Is(err, queue.ErrUnavailable) || len(db.calls) != 0 {
		t.Fatal(err)
	}
}

func TestQueueACKDeletesOnlyAuthenticatedRecipientRecord(t *testing.T) {
	event := queueSampleEvent()
	for _, tag := range []string{"DELETE 1", "DELETE 0"} {
		db := &queueTestDB{tag: pgconn.NewCommandTag(tag)}
		s := &QueueStore{db: db}
		if err := s.Ack(context.Background(), event.RecipientVoidID, event.SenderVoidID, event.EventID); err != nil {
			t.Fatal(err)
		}
		call := db.calls[0]
		if call.sql != "DELETE FROM public.messages WHERE recipient_void_id=$1 AND sender_void_id=$2 AND event_id=$3" || !reflect.DeepEqual(call.args, []any{int64(101), int64(9223372036854775807), queueUUID(event.EventID)}) {
			t.Fatal("ACK ownership is not enforced", call)
		}
	}
}

func TestQueuePendingFiltersExpiredAndCopiesOpaqueBytes(t *testing.T) {
	event := queueSampleEvent()
	original := append([]byte(nil), event.Payload...)
	db := &queueTestDB{rowsResults: []pgx.Rows{queueRows([]any{event.EventID, event.SenderVoidID, event.RecipientVoidID, original, event.CreatedAt.In(time.FixedZone("synthetic", 8*3600)), event.ExpiresAt.In(time.FixedZone("synthetic", 8*3600))})}}
	s := &QueueStore{db: db}
	events, err := s.Pending(context.Background(), "101", event.CreatedAt, 100)
	if err != nil || len(events) != 1 || !reflect.DeepEqual(events[0], event) {
		t.Fatal(events, err)
	}
	call := db.calls[0]
	if !strings.Contains(call.sql, "WHERE recipient_void_id=$1 AND expires_at>$2 ORDER BY created_at,sender_void_id,event_id LIMIT $3") || !reflect.DeepEqual(call.args, []any{int64(101), event.CreatedAt, 100}) {
		t.Fatal("pending query ownership,expiry or bounds changed")
	}
	events[0].Payload[0] = 99
	if original[0] == 99 {
		t.Fatal("row payload memory escaped without copy")
	}
}

func TestQueueExpiryDeletesBoundedBatchesWithoutHistory(t *testing.T) {
	now := queueSampleEvent().CreatedAt
	db := &queueTestDB{tag: pgconn.NewCommandTag("DELETE 12")}
	s := &QueueStore{db: db}
	n, err := s.Expire(context.Background(), now, 100)
	if err != nil || n != 12 {
		t.Fatal(n, err)
	}
	call := db.calls[0]
	if !strings.Contains(call.sql, "WHERE expires_at<=$1") || !strings.Contains(call.sql, "LIMIT $2)") || !strings.Contains(call.sql, "DELETE FROM public.messages m USING expired e WHERE m.sender_void_id=e.sender_void_id AND m.event_id=e.event_id AND m.expires_at<=$1") || strings.Contains(call.sql, "FOR UPDATE") || strings.Contains(call.sql, "SKIP LOCKED") || !reflect.DeepEqual(call.args, []any{now, 100}) {
		t.Fatal("unbounded or incorrectly keyed expiry", call)
	}
}

func queueValidSchemaStore() (*QueueStore, *queueTestDB) {
	constraints := queueRows(
		[]any{"p", true, false, []string{"sender_void_id", "event_id"}, false, ""},
		[]any{"f", true, false, []string{"sender_void_id"}, true, ""},
		[]any{"f", true, false, []string{"recipient_void_id"}, true, ""},
		[]any{"c", true, false, []string{"payload"}, false, "((octet_length(payload) >= 1) AND (octet_length(payload) <= 1048576))"},
		[]any{"c", true, false, []string{"expires_at", "created_at"}, false, "(expires_at = (created_at + '720:00:00'::interval))"},
	)
	indexes := queueRows(
		[]any{[]string{"sender_void_id", "event_id"}, true},
		[]any{[]string{"recipient_void_id", "created_at", "sender_void_id", "event_id"}, true},
		[]any{[]string{"expires_at", "sender_void_id", "event_id"}, true},
	)
	db := &queueTestDB{rowResults: []pgx.Row{queueRow(true)}, rowsResults: []pgx.Rows{constraints, indexes}}
	return &QueueStore{db: db}, db
}

func TestQueueValidationUsesOnlyCatalogReads(t *testing.T) {
	s, db := queueValidSchemaStore()
	if err := s.Validate(context.Background()); err != nil {
		t.Fatal(err)
	}
	if db.beginCount != 0 || len(db.calls) != 3 {
		t.Fatal("unexpected schema validation operations")
	}
	for _, call := range db.calls {
		if !strings.HasPrefix(call.sql, "SELECT ") || !strings.Contains(call.sql, "pg_catalog.") {
			t.Fatal("validation performs non-catalog mutation/read", call.sql)
		}
	}
	if !strings.Contains(db.calls[0].sql, "has_table_privilege(c.oid,'DELETE')") || !strings.Contains(db.calls[0].sql, "count(*)=6") || !strings.Contains(db.calls[0].sql, "ARRAY['accounts','messages']") {
		t.Fatal("legacy schema or missing delete privilege could pass")
	}
}

func TestQueueValidationRejectsLegacyOrIncompleteSchema(t *testing.T) {
	for _, tc := range []struct {
		name   string
		change func(*queueTestDB)
	}{
		{"old_schema_or_access", func(db *queueTestDB) { db.rowResults = []pgx.Row{queueRow(false)} }},
		{"missing_table", func(db *queueTestDB) {
			db.rowResults = []pgx.Row{queueTestRow(func(...any) error { return pgx.ErrNoRows })}
		}},
		{"wrong_primary_key", func(db *queueTestDB) { db.rowsResults[0].(*queueTestRows).values[0][3] = []string{"event_id"} }},
		{"wrong_owner_fk", func(db *queueTestDB) { db.rowsResults[0].(*queueTestRows).values[1][4] = false }},
		{"overlong_retention", func(db *queueTestDB) {
			db.rowsResults[0].(*queueTestRows).values[4][5] = "expires_at>created_at AND expires_at<=created_at+'31 days'::interval"
		}},
		{"missing_expiry_index", func(db *queueTestDB) {
			db.rowsResults[1].(*queueTestRows).values = db.rowsResults[1].(*queueTestRows).values[:2]
		}},
		{"unusable_delivery_index", func(db *queueTestDB) { db.rowsResults[1].(*queueTestRows).values[1][1] = false }},
	} {
		t.Run(tc.name, func(t *testing.T) {
			s, db := queueValidSchemaStore()
			tc.change(db)
			if !errors.Is(s.Validate(context.Background()), ErrQueueSchema) {
				t.Fatal("bad queue schema accepted")
			}
		})
	}
}

func TestQueueInvalidInputsAndDiagnosticErrorsDoNotReachStorage(t *testing.T) {
	db := &queueTestDB{}
	s := &QueueStore{db: db}
	event := queueSampleEvent()
	event.ExpiresAt = event.CreatedAt.Add(queue.Retention + time.Second)
	if !errors.Is(s.Enqueue(context.Background(), event), queue.ErrInvalid) {
		t.Fatal("overlong TTL accepted")
	}
	if _, err := s.Pending(context.Background(), "101", event.CreatedAt, 101); !errors.Is(err, queue.ErrInvalid) {
		t.Fatal(err)
	}
	if _, err := s.Expire(context.Background(), event.CreatedAt, 1001); !errors.Is(err, queue.ErrInvalid) {
		t.Fatal(err)
	}
	if err := s.Ack(context.Background(), "102", "101", "invalid"); !errors.Is(err, queue.ErrInvalid) {
		t.Fatal(err)
	}
	if len(db.calls) != 0 || db.beginCount != 0 {
		t.Fatal("invalid input reached storage")
	}
	secret := errors.New("secret connection string and encrypted payload")
	if got := queueError(context.Background(), secret); got != queue.ErrUnavailable || errors.Is(got, secret) {
		t.Fatal(got)
	}
	ctx, cancel := context.WithCancel(context.Background())
	cancel()
	if !errors.Is(queueError(ctx, secret), context.Canceled) {
		t.Fatal("context cancellation lost")
	}
}

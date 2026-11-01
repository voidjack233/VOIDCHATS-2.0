package postgres

import (
	"context"
	"errors"
	"strings"
	"testing"

	"github.com/jackc/pgx/v5"
	"github.com/jackc/pgx/v5/pgconn"
	"github.com/jackc/pgx/v5/pgtype"
	"void-nothingness/internal/messaging"
)

type messagingExecTx struct {
	pgx.Tx
	sql  string
	args []any
	tag  pgconn.CommandTag
	err  error
}

func (t *messagingExecTx) Exec(_ context.Context, sql string, args ...any) (pgconn.CommandTag, error) {
	t.sql = sql
	t.args = args
	return t.tag, t.err
}

// This checks the PostgreSQL-specific regression without a database: inserting
// -1 and relying on ON CONFLICT fails PostgreSQL's INSERT CHECK validation before
// the existing row is updated. Negative changes must exclusively use UPDATE.
func TestMessagingReceiptDecrementUpdatesExistingCounter(t *testing.T) {
	db := &messagingExecTx{tag: pgconn.NewCommandTag("UPDATE 1")}
	tx := &messagingTx{ctx: context.Background(), tx: db}
	if e := tx.ReserveReceipt("9223372036854775807", -1); e != nil {
		t.Fatal(e)
	}
	if !strings.HasPrefix(db.sql, "UPDATE public.void_event_counters") || strings.Contains(db.sql, "INSERT") {
		t.Fatalf("decrement must UPDATE: %s", db.sql)
	}
	if db.args[0] != (pgtype.Int8{Int64: 9223372036854775807, Valid: true}) || db.args[1] != -1 {
		t.Fatalf("typed parameters: %v", db.args)
	}
	db.tag = pgconn.NewCommandTag("UPDATE 0")
	if e := tx.ReserveReceipt("101", -1); !errors.Is(e, errMessagingUnavailable) {
		t.Fatalf("missing reservation silently accepted: %v", e)
	}
}
func TestMessagingReceiptIncrementUpsertsPositiveCounter(t *testing.T) {
	db := &messagingExecTx{tag: pgconn.NewCommandTag("INSERT 0 1")}
	tx := &messagingTx{ctx: context.Background(), tx: db}
	if e := tx.ReserveReceipt("101", 1); e != nil {
		t.Fatal(e)
	}
	if !strings.HasPrefix(db.sql, "INSERT INTO public.void_event_counters") || db.args[1] != 1 {
		t.Fatal(db.sql, db.args)
	}
}

func TestMessagingPostgresTypedIdentityAndUUIDBinaryEncoding(t *testing.T) {
	m := pgtype.NewMap()
	n := pgInt("9223372036854775807")
	encoded, e := m.Encode(pgtype.Int8OID, pgtype.BinaryFormatCode, n, nil)
	if e != nil || len(encoded) != 8 {
		t.Fatalf("binary int8: %x %v", encoded, e)
	}
	var roundtrip int64
	if e = m.Scan(pgtype.Int8OID, pgtype.BinaryFormatCode, encoded, &roundtrip); e != nil || roundtrip != 9223372036854775807 {
		t.Fatal(roundtrip, e)
	}
	id := "00000000-0000-4000-8000-000000000001"
	encoded, e = m.Encode(pgtype.UUIDOID, pgtype.BinaryFormatCode, pgUUID(id), nil)
	if e != nil || len(encoded) != 16 {
		t.Fatalf("binary UUID: %x %v", encoded, e)
	}
	var text string
	if e = m.Scan(pgtype.UUIDOID, pgtype.BinaryFormatCode, encoded, &text); e != nil || text != id {
		t.Fatal(text, e)
	}
	for _, empty := range []any{pgInt(""), pgUUID("")} {
		oid := uint32(pgtype.Int8OID)
		if _, ok := empty.(pgtype.UUID); ok {
			oid = pgtype.UUIDOID
		}
		encoded, e = m.Encode(oid, pgtype.BinaryFormatCode, empty, nil)
		if e != nil || encoded != nil {
			t.Fatalf("nullable: %x %v", encoded, e)
		}
	}
}

func TestMessagingDiagnosticsAreNotReturned(t *testing.T) {
	secret := errors.New("SQL includes secret connection string and ciphertext")
	e := safeMessagingError(context.Background(), secret)
	if e != errMessagingUnavailable || errors.Is(e, secret) || strings.Contains(e.Error(), "secret") {
		t.Fatalf("unsafe diagnostics: %v", e)
	}
	app := &messaging.Error{Status: 409, Code: "idempotency_conflict", Message: "fixed safe message"}
	if safeMessagingError(context.Background(), app) != app {
		t.Fatal("application error changed")
	}
	ctx, cancel := context.WithCancel(context.Background())
	cancel()
	if !errors.Is(safeMessagingError(ctx, secret), context.Canceled) {
		t.Fatal("cancellation lost")
	}
}

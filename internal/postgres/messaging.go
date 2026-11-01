package postgres

import (
	"context"
	"encoding/base64"
	"encoding/json"
	"errors"
	"strconv"
	"time"

	"github.com/jackc/pgx/v5"
	"github.com/jackc/pgx/v5/pgtype"
	"github.com/jackc/pgx/v5/pgxpool"
	"void-nothingness/internal/messaging"
)

var ErrMessagingSchema = errors.New("postgres: reviewed messaging migration or required runtime privileges are missing")
var errMessagingUnavailable = errors.New("postgres: messaging temporarily unavailable")

type MessagingStore struct{ pool *pgxpool.Pool }

func NewMessagingStore(pool *pgxpool.Pool) *MessagingStore { return &MessagingStore{pool: pool} }

var _ messaging.Store = (*MessagingStore)(nil)

// Validate is read-only. In particular it never runs the migration or grants.
func (s *MessagingStore) Validate(ctx context.Context) error {
	if s.pool == nil {
		return ErrMessagingSchema
	}
	var version int
	if e := s.pool.QueryRow(ctx, `SELECT version FROM public.void_messaging_schema WHERE version=1`).Scan(&version); e != nil {
		return ErrMessagingSchema
	}
	tables := []string{"void_key_packages", "void_requests", "void_conversations", "void_blocks", "void_messages", "void_event_counters", "void_events"}
	for _, table := range tables {
		var okay bool
		e := s.pool.QueryRow(ctx, `SELECT COALESCE(c.relkind='r' AND c.relpersistence='p' AND NOT c.relrowsecurity AND has_table_privilege(c.oid,'SELECT') AND has_table_privilege(c.oid,'INSERT') AND has_table_privilege(c.oid,'UPDATE'),false) FROM pg_catalog.pg_class c WHERE c.oid=pg_catalog.to_regclass($1)`, "public."+table).Scan(&okay)
		if e != nil || !okay {
			return ErrMessagingSchema
		}
	}
	// Preparing these zero-row projections catches incomplete/old migrations and
	// column type mismatches without exposing or changing any account data.
	checks := []string{
		`SELECT key_package_id::uuid,owner_void_id::bigint,fingerprint::text,package_base64::text,expires_at::timestamptz,claimed_by::bigint,claim_id::uuid,request_id::uuid FROM public.void_key_packages LIMIT 0`,
		`SELECT request_id::uuid,conversation_id::uuid,sender_void_id::bigint,recipient_void_id::bigint,client_request_id::uuid,key_package_id::uuid,status::text,pair_a::bigint,pair_b::bigint,record::bytea FROM public.void_requests LIMIT 0`,
		`SELECT conversation_id::uuid,request_id::uuid,sender_void_id::bigint,recipient_void_id::bigint,last_sequence::bigint,record::bytea FROM public.void_conversations LIMIT 0`,
		`SELECT blocker_void_id::bigint,target_void_id::bigint,created_at::timestamptz FROM public.void_blocks LIMIT 0`,
		`SELECT message_id::uuid,conversation_id::uuid,sender_void_id::bigint,recipient_void_id::bigint,client_message_id::uuid,sequence::bigint,ciphertext_bytes::integer,record::bytea FROM public.void_messages LIMIT 0`,
		`SELECT owner_void_id::bigint,last_cursor::bigint,receipt_reservations::integer FROM public.void_event_counters LIMIT 0`,
		`SELECT event_id::uuid,owner_void_id::bigint,cursor::bigint,record::bytea,outcome::text FROM public.void_events LIMIT 0`,
		`SELECT void_id,username,auth_public_key FROM public.accounts LIMIT 0`,
	}
	for _, q := range checks {
		rows, e := s.pool.Query(ctx, q)
		if e != nil {
			return ErrMessagingSchema
		}
		rows.Close()
		if rows.Err() != nil {
			return ErrMessagingSchema
		}
	}
	return nil
}
func (s *MessagingStore) Run(ctx context.Context, fn func(messaging.Tx) error) (err error) {
	if s.pool == nil {
		return errMessagingUnavailable
	}
	tx, e := s.pool.BeginTx(ctx, pgx.TxOptions{IsoLevel: pgx.ReadCommitted})
	if e != nil {
		return safeMessagingError(ctx, e)
	}
	defer func() {
		cleanup, cancel := context.WithTimeout(context.Background(), 5*time.Second)
		defer cancel()
		_ = tx.Rollback(cleanup)
	}()
	// All messaging mutations share this lock, across processes. Allocating an
	// event cursor therefore follows commit order: polling can never skip a late
	// committing earlier event. This first version trades write throughput for a
	// simple, auditable atomic protocol. No account/registration lock is involved.
	if _, e = tx.Exec(ctx, `SET LOCAL lock_timeout='5s'`); e != nil {
		return safeMessagingError(ctx, e)
	}
	if _, e = tx.Exec(ctx, `SET LOCAL statement_timeout='15s'`); e != nil {
		return safeMessagingError(ctx, e)
	}
	if _, e = tx.Exec(ctx, `SELECT pg_advisory_xact_lock(6218986722290336081)`); e != nil {
		return safeMessagingError(ctx, e)
	}
	if e = fn(&messagingTx{ctx: ctx, tx: tx}); e != nil {
		return safeMessagingError(ctx, e)
	}
	return safeMessagingError(ctx, tx.Commit(ctx))
}
func safeMessagingError(ctx context.Context, e error) error {
	if e == nil {
		return nil
	}
	if ctx.Err() != nil {
		return ctx.Err()
	}
	var app *messaging.Error
	if errors.As(e, &app) {
		return app
	}
	return errMessagingUnavailable
}

type messagingTx struct {
	ctx context.Context
	tx  pgx.Tx
}

func (t *messagingTx) Person(id string) (out messaging.Person, ok bool, err error) {
	var key []byte
	var n int64
	err = t.tx.QueryRow(t.ctx, `SELECT void_id,username,auth_public_key FROM public.accounts WHERE void_id=$1`, pgInt(id)).Scan(&n, &out.Username, &key)
	if errors.Is(err, pgx.ErrNoRows) {
		return out, false, nil
	}
	if err != nil {
		return out, false, err
	}
	out.VoidID = strconv.FormatInt(n, 10)
	out.AuthPublicKey = base64.StdEncoding.EncodeToString(key)
	return out, true, nil
}
func packageRow(row pgx.Row) (out messaging.PackageRow, ok bool, err error) {
	err = row.Scan(&out.ID, &out.Owner, &out.Fingerprint, &out.Data, &out.ExpiresAt, &out.ClaimedBy, &out.ClaimID, &out.RequestID)
	if errors.Is(err, pgx.ErrNoRows) {
		return out, false, nil
	}
	return out, err == nil, err
}

const packageColumns = `key_package_id::text,owner_void_id::text,fingerprint,package_base64,expires_at,COALESCE(claimed_by::text,''),COALESCE(claim_id::text,''),COALESCE(request_id::text,'')`

func (t *messagingTx) Package(id string) (messaging.PackageRow, bool, error) {
	return packageRow(t.tx.QueryRow(t.ctx, `SELECT `+packageColumns+` FROM public.void_key_packages WHERE key_package_id=$1`, pgUUID(id)))
}
func (t *messagingTx) PackageByFingerprint(owner, fp string) (messaging.PackageRow, bool, error) {
	return packageRow(t.tx.QueryRow(t.ctx, `SELECT `+packageColumns+` FROM public.void_key_packages WHERE owner_void_id=$1 AND fingerprint=$2`, pgInt(owner), fp))
}
func (t *messagingTx) AvailablePackage(owner string, now time.Time) (messaging.PackageRow, bool, error) {
	return packageRow(t.tx.QueryRow(t.ctx, `SELECT `+packageColumns+` FROM public.void_key_packages WHERE owner_void_id=$1 AND claimed_by IS NULL AND expires_at>$2 ORDER BY key_package_id LIMIT 1 FOR UPDATE`, pgInt(owner), now))
}
func (t *messagingTx) ClaimedPackage(actor, client string) (messaging.PackageRow, bool, error) {
	return packageRow(t.tx.QueryRow(t.ctx, `SELECT `+packageColumns+` FROM public.void_key_packages WHERE claimed_by=$1 AND claim_id=$2`, pgInt(actor), pgUUID(client)))
}
func (t *messagingTx) PackageCount(owner string) (n, size int, err error) {
	err = t.tx.QueryRow(t.ctx, `SELECT count(*),COALESCE(sum(octet_length(package_base64)),0) FROM public.void_key_packages WHERE owner_void_id=$1`, pgInt(owner)).Scan(&n, &size)
	return
}
func nullable(s string) any {
	if s == "" {
		return nil
	}
	return s
}
func (t *messagingTx) SavePackage(p messaging.PackageRow) error {
	_, e := t.tx.Exec(t.ctx, `INSERT INTO public.void_key_packages(key_package_id,owner_void_id,fingerprint,package_base64,expires_at,claimed_by,claim_id,request_id) VALUES($1,$2,$3,$4,$5,$6,$7,$8) ON CONFLICT(key_package_id) DO UPDATE SET claimed_by=excluded.claimed_by,claim_id=excluded.claim_id,request_id=excluded.request_id`, pgUUID(p.ID), pgInt(p.Owner), p.Fingerprint, p.Data, p.ExpiresAt, pgInt(p.ClaimedBy), pgUUID(p.ClaimID), pgUUID(p.RequestID))
	return e
}

func jsonRow[T any](row pgx.Row) (out T, ok bool, err error) {
	var b []byte
	err = row.Scan(&b)
	if errors.Is(err, pgx.ErrNoRows) {
		return out, false, nil
	}
	if err != nil {
		return out, false, err
	}
	err = json.Unmarshal(b, &out)
	return out, err == nil, err
}
func jsonRows[T any](rows pgx.Rows, err error) ([]T, error) {
	if err != nil {
		return nil, err
	}
	defer rows.Close()
	out := []T{}
	for rows.Next() {
		var b []byte
		if e := rows.Scan(&b); e != nil {
			return nil, e
		}
		var value T
		if e := json.Unmarshal(b, &value); e != nil {
			return nil, e
		}
		out = append(out, value)
	}
	return out, rows.Err()
}
func (t *messagingTx) Request(id string) (messaging.Request, bool, error) {
	return jsonRow[messaging.Request](t.tx.QueryRow(t.ctx, `SELECT record FROM public.void_requests WHERE request_id=$1`, pgUUID(id)))
}
func (t *messagingTx) RequestByClient(actor, client string) (messaging.Request, bool, error) {
	return jsonRow[messaging.Request](t.tx.QueryRow(t.ctx, `SELECT record FROM public.void_requests WHERE sender_void_id=$1 AND client_request_id=$2`, pgInt(actor), pgUUID(client)))
}
func (t *messagingTx) ActivePair(a, b string) (messaging.Request, bool, error) {
	return jsonRow[messaging.Request](t.tx.QueryRow(t.ctx, `SELECT record FROM public.void_requests WHERE pair_a=least($1::bigint,$2::bigint) AND pair_b=greatest($1::bigint,$2::bigint) AND status IN ('pending','accepted')`, pgInt(a), pgInt(b)))
}
func (t *messagingTx) RequestCount(owner string) (n int, err error) {
	err = t.tx.QueryRow(t.ctx, `SELECT count(*) FROM public.void_requests WHERE sender_void_id=$1 OR recipient_void_id=$1`, pgInt(owner)).Scan(&n)
	return
}
func (t *messagingTx) Requests(owner, after string, limit int) ([]messaging.Request, error) {
	rows, e := t.tx.Query(t.ctx, `SELECT record FROM public.void_requests WHERE (sender_void_id=$1 OR recipient_void_id=$1) AND ($2::uuid IS NULL OR request_id>$2::uuid) ORDER BY request_id LIMIT $3`, pgInt(owner), pgUUID(after), limit)
	return jsonRows[messaging.Request](rows, e)
}
func (t *messagingTx) SaveRequest(r messaging.Request) error {
	b, e := json.Marshal(r)
	if e != nil {
		return e
	}
	_, e = t.tx.Exec(t.ctx, `INSERT INTO public.void_requests(request_id,conversation_id,sender_void_id,recipient_void_id,client_request_id,key_package_id,status,pair_a,pair_b,record) VALUES($1,$2,$3,$4,$5,$6,$7,least($3::bigint,$4::bigint),greatest($3::bigint,$4::bigint),$8) ON CONFLICT(request_id) DO UPDATE SET status=excluded.status,record=excluded.record`, pgUUID(r.RequestID), pgUUID(r.ConversationID), pgInt(r.Sender.VoidID), pgInt(r.Recipient.VoidID), pgUUID(r.ClientRequestID), pgUUID(r.KeyPackageID), r.Status, b)
	return e
}
func (t *messagingTx) Conversation(id string) (out messaging.ConversationRow, ok bool, err error) {
	var b []byte
	err = t.tx.QueryRow(t.ctx, `SELECT record,last_sequence FROM public.void_conversations WHERE conversation_id=$1`, pgUUID(id)).Scan(&b, &out.LastSequence)
	if errors.Is(err, pgx.ErrNoRows) {
		return out, false, nil
	}
	if err != nil {
		return out, false, err
	}
	err = json.Unmarshal(b, &out.Conversation)
	return out, err == nil, err
}
func (t *messagingTx) Conversations(owner, after string, limit int) ([]messaging.Conversation, error) {
	rows, e := t.tx.Query(t.ctx, `SELECT record FROM public.void_conversations WHERE (sender_void_id=$1 OR recipient_void_id=$1) AND ($2::uuid IS NULL OR conversation_id>$2::uuid) ORDER BY conversation_id LIMIT $3`, pgInt(owner), pgUUID(after), limit)
	return jsonRows[messaging.Conversation](rows, e)
}
func (t *messagingTx) SaveConversation(c messaging.ConversationRow) error {
	b, e := json.Marshal(c.Conversation)
	if e != nil {
		return e
	}
	if len(c.Conversation.Participants) != 2 {
		return errMessagingUnavailable
	}
	_, e = t.tx.Exec(t.ctx, `INSERT INTO public.void_conversations(conversation_id,request_id,sender_void_id,recipient_void_id,last_sequence,record) VALUES($1,$2,$3,$4,$5,$6) ON CONFLICT(conversation_id) DO UPDATE SET last_sequence=excluded.last_sequence,record=excluded.record`, pgUUID(c.Conversation.ConversationID), pgUUID(c.Conversation.RequestID), pgInt(c.Conversation.Participants[0].VoidID), pgInt(c.Conversation.Participants[1].VoidID), c.LastSequence, b)
	return e
}
func (t *messagingTx) Blocked(a, b string) (out bool, err error) {
	err = t.tx.QueryRow(t.ctx, `SELECT EXISTS(SELECT 1 FROM public.void_blocks WHERE (blocker_void_id=$1 AND target_void_id=$2) OR (blocker_void_id=$2 AND target_void_id=$1))`, pgInt(a), pgInt(b)).Scan(&out)
	return
}
func (t *messagingTx) SaveBlock(a, b string, at time.Time) (bool, error) {
	tag, e := t.tx.Exec(t.ctx, `INSERT INTO public.void_blocks(blocker_void_id,target_void_id,created_at) VALUES($1,$2,$3) ON CONFLICT DO NOTHING`, pgInt(a), pgInt(b), at)
	return tag.RowsAffected() == 1, e
}
func (t *messagingTx) MessageByClient(actor, client string) (messaging.Message, bool, error) {
	return jsonRow[messaging.Message](t.tx.QueryRow(t.ctx, `SELECT record FROM public.void_messages WHERE sender_void_id=$1 AND client_message_id=$2`, pgInt(actor), pgUUID(client)))
}
func (t *messagingTx) MessageCount(conv string) (n, size int, err error) {
	err = t.tx.QueryRow(t.ctx, `SELECT count(*),COALESCE(sum(ciphertext_bytes),0) FROM public.void_messages WHERE conversation_id=$1`, pgUUID(conv)).Scan(&n, &size)
	return
}
func (t *messagingTx) SaveMessage(m messaging.Message) error {
	b, e := json.Marshal(m)
	if e != nil {
		return e
	}
	_, e = t.tx.Exec(t.ctx, `INSERT INTO public.void_messages(message_id,conversation_id,sender_void_id,recipient_void_id,client_message_id,sequence,ciphertext_bytes,record) VALUES($1,$2,$3,$4,$5,$6,$7,$8)`, pgUUID(m.MessageID), pgUUID(m.ConversationID), pgInt(m.SenderVoidID), pgInt(m.RecipientVoidID), pgUUID(m.ClientMessageID), pgInt(m.Sequence), len(m.CiphertextBase64), b)
	return e
}
func (t *messagingTx) Event(owner, id string) (out messaging.EventRow, ok bool, err error) {
	var b []byte
	out.Owner = owner
	err = t.tx.QueryRow(t.ctx, `SELECT record,COALESCE(outcome,'') FROM public.void_events WHERE owner_void_id=$1 AND event_id=$2`, pgInt(owner), pgUUID(id)).Scan(&b, &out.Outcome)
	if errors.Is(err, pgx.ErrNoRows) {
		return out, false, nil
	}
	if err != nil {
		return out, false, err
	}
	err = json.Unmarshal(b, &out.Event)
	return out, err == nil, err
}
func (t *messagingTx) Events(owner string, after int64, limit int) ([]messaging.EventRow, error) {
	rows, e := t.tx.Query(t.ctx, `SELECT record FROM public.void_events WHERE owner_void_id=$1 AND cursor>$2 AND outcome IS NULL ORDER BY cursor LIMIT $3`, pgInt(owner), after, limit)
	events, e := jsonRows[messaging.Event](rows, e)
	if e != nil {
		return nil, e
	}
	out := make([]messaging.EventRow, 0, len(events))
	for _, event := range events {
		out = append(out, messaging.EventRow{Owner: owner, Event: event})
	}
	return out, nil
}
func (t *messagingTx) EventCounts(owner string) (total, pending, totalBytes, pendingBytes int, err error) {
	err = t.tx.QueryRow(t.ctx, `SELECT count(*),count(*) FILTER(WHERE outcome IS NULL),COALESCE(sum(octet_length(record)),0),COALESCE(sum(octet_length(record)) FILTER(WHERE outcome IS NULL),0) FROM public.void_events WHERE owner_void_id=$1`, pgInt(owner)).Scan(&total, &pending, &totalBytes, &pendingBytes)
	return
}
func (t *messagingTx) NextCursor(owner string) (n int64, err error) {
	err = t.tx.QueryRow(t.ctx, `INSERT INTO public.void_event_counters(owner_void_id,last_cursor) VALUES($1,1) ON CONFLICT(owner_void_id) DO UPDATE SET last_cursor=void_event_counters.last_cursor+1 RETURNING last_cursor`, pgInt(owner)).Scan(&n)
	return
}
func (t *messagingTx) ReceiptReservations(owner string) (n int, err error) {
	err = t.tx.QueryRow(t.ctx, `SELECT receipt_reservations FROM public.void_event_counters WHERE owner_void_id=$1`, pgInt(owner)).Scan(&n)
	if errors.Is(err, pgx.ErrNoRows) {
		return 0, nil
	}
	return
}
func (t *messagingTx) ReserveReceipt(owner string, delta int) error {
	if delta < 0 {
		tag, e := t.tx.Exec(t.ctx, `UPDATE public.void_event_counters SET receipt_reservations=receipt_reservations+$2 WHERE owner_void_id=$1 AND receipt_reservations>=(-$2)`, pgInt(owner), delta)
		if e != nil {
			return e
		}
		if tag.RowsAffected() != 1 {
			return errMessagingUnavailable
		}
		return nil
	}
	_, e := t.tx.Exec(t.ctx, `INSERT INTO public.void_event_counters(owner_void_id,receipt_reservations) VALUES($1,$2) ON CONFLICT(owner_void_id) DO UPDATE SET receipt_reservations=void_event_counters.receipt_reservations+$2`, pgInt(owner), delta)
	return e
}
func (t *messagingTx) SaveEvent(row messaging.EventRow) error {
	b, e := json.Marshal(row.Event)
	if e != nil {
		return e
	}
	_, e = t.tx.Exec(t.ctx, `INSERT INTO public.void_events(event_id,owner_void_id,cursor,record,outcome) VALUES($1,$2,$3,$4,$5) ON CONFLICT(event_id) DO UPDATE SET outcome=excluded.outcome`, pgUUID(row.Event.EventID), pgInt(row.Owner), pgInt(row.Event.Cursor), b, nullable(row.Outcome))
	return e
}

// Explicit pgtype values avoid driver text/binary coercion of decimal JSON IDs.
func pgInt(s string) pgtype.Int8 {
	n, e := strconv.ParseInt(s, 10, 64)
	return pgtype.Int8{Int64: n, Valid: e == nil}
}
func pgUUID(s string) pgtype.UUID {
	var value pgtype.UUID
	if s != "" {
		_ = value.Scan(s)
	}
	return value
}

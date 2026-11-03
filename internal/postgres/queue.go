package postgres

import (
	"context"
	"encoding/base64"
	"errors"
	"sort"
	"strconv"
	"strings"
	"time"

	"github.com/jackc/pgx/v5"
	"github.com/jackc/pgx/v5/pgconn"
	"github.com/jackc/pgx/v5/pgtype"
	"github.com/jackc/pgx/v5/pgxpool"
	"void-nothingness/internal/auth"
	"void-nothingness/internal/queue"
)

var ErrQueueSchema = errors.New("postgres: reviewed temporary queue schema or required access is missing")

type queueDB interface {
	BeginTx(context.Context, pgx.TxOptions) (pgx.Tx, error)
	Query(context.Context, string, ...any) (pgx.Rows, error)
	QueryRow(context.Context, string, ...any) pgx.Row
	Exec(context.Context, string, ...any) (pgconn.CommandTag, error)
}

// QueueStore stores only events awaiting recipient acknowledgment or expiry.
// It never creates schema objects and never holds a transaction during network I/O.
type QueueStore struct{ db queueDB }

func NewQueueStore(pool *pgxpool.Pool) *QueueStore {
	if pool == nil {
		return &QueueStore{}
	}
	return &QueueStore{db: pool}
}

var _ queue.Store = (*QueueStore)(nil)

func queueUUID(id string) pgtype.UUID {
	var value pgtype.UUID
	_ = value.Scan(id)
	return value
}

func queueNumber(id string) int64 { n, _ := strconv.ParseInt(id, 10, 64); return n }

func queueError(ctx context.Context, err error) error {
	if err == nil {
		return nil
	}
	if ctx.Err() != nil {
		return ctx.Err()
	}
	for _, known := range []error{queue.ErrInvalid, queue.ErrNotFound, queue.ErrDuplicate, queue.ErrCapacity} {
		if errors.Is(err, known) {
			return known
		}
	}
	// Do not retain SQL, ciphertext, keys or connection diagnostics in errors.
	return queue.ErrUnavailable
}

func (s *QueueStore) Lookup(ctx context.Context, id string) (queue.Person, error) {
	if !auth.ValidVoidID(id) {
		return queue.Person{}, queue.ErrInvalid
	}
	if s.db == nil {
		return queue.Person{}, queue.ErrUnavailable
	}
	var n int64
	var key []byte
	err := s.db.QueryRow(ctx, `SELECT void_id,auth_public_key FROM public.accounts WHERE void_id=$1`, queueNumber(id)).Scan(&n, &key)
	if errors.Is(err, pgx.ErrNoRows) {
		return queue.Person{}, queueError(ctx, queue.ErrNotFound)
	}
	if err != nil {
		return queue.Person{}, queueError(ctx, err)
	}
	if len(key) != 32 {
		return queue.Person{}, queue.ErrUnavailable
	}
	return queue.Person{VoidID: strconv.FormatInt(n, 10), AuthPublicKey: base64.StdEncoding.EncodeToString(key)}, nil
}

func (s *QueueStore) Enqueue(ctx context.Context, event queue.Event) error {
	if !auth.ValidVoidID(event.SenderVoidID) || !auth.ValidVoidID(event.RecipientVoidID) || !queue.ValidEventID(event.EventID) || len(event.Payload) < 1 || len(event.Payload) > queue.MaxPayloadBytes || event.CreatedAt.IsZero() || event.ExpiresAt.Sub(event.CreatedAt) != queue.Retention {
		return queue.ErrInvalid
	}
	if s.db == nil {
		return queue.ErrUnavailable
	}
	tx, err := s.db.BeginTx(ctx, pgx.TxOptions{IsoLevel: pgx.ReadCommitted})
	if err != nil {
		return queueError(ctx, err)
	}
	defer func() {
		cleanup, cancel := context.WithTimeout(context.Background(), 5*time.Second)
		defer cancel()
		_ = tx.Rollback(cleanup)
	}()
	if _, err = tx.Exec(ctx, `SET LOCAL lock_timeout='5s'`); err != nil {
		return queueError(ctx, err)
	}
	if _, err = tx.Exec(ctx, `SET LOCAL statement_timeout='15s'`); err != nil {
		return queueError(ctx, err)
	}

	ids := []int64{queueNumber(event.SenderVoidID)}
	if event.RecipientVoidID != event.SenderVoidID {
		ids = append(ids, queueNumber(event.RecipientVoidID))
	}
	sort.Slice(ids, func(i, j int) bool { return ids[i] < ids[j] })
	// Serialize only the affected accounts, in numeric order, without requiring
	// account UPDATE privileges. Splitting a positive int64 into two int32 keys
	// is injective; this lock space is separate from PostgreSQL's bigint locks.
	for _, id := range ids {
		if _, err = tx.Exec(ctx, `SELECT pg_advisory_xact_lock($1::integer,$2::integer)`, int32(id>>32), int32(id)); err != nil {
			return queueError(ctx, err)
		}
	}
	rows, err := tx.Query(ctx, `SELECT void_id FROM public.accounts WHERE void_id=ANY($1::bigint[]) ORDER BY void_id`, ids)
	if err != nil {
		return queueError(ctx, err)
	}
	found := 0
	for rows.Next() {
		var id int64
		if err = rows.Scan(&id); err != nil {
			rows.Close()
			return queueError(ctx, err)
		}
		found++
	}
	err = rows.Err()
	rows.Close()
	if err != nil {
		return queueError(ctx, err)
	}
	if found != len(ids) {
		return queue.ErrNotFound
	}

	var duplicate bool
	if err = tx.QueryRow(ctx, `SELECT EXISTS(SELECT 1 FROM public.messages WHERE sender_void_id=$1 AND event_id=$2)`, queueNumber(event.SenderVoidID), queueUUID(event.EventID)).Scan(&duplicate); err != nil {
		return queueError(ctx, err)
	}
	if duplicate {
		return queue.ErrDuplicate
	}
	for _, check := range []struct {
		query string
		id    int64
	}{
		{`SELECT count(*),COALESCE(sum(octet_length(payload)),0) FROM public.messages WHERE sender_void_id=$1 AND expires_at>$2`, queueNumber(event.SenderVoidID)},
		{`SELECT count(*),COALESCE(sum(octet_length(payload)),0) FROM public.messages WHERE recipient_void_id=$1 AND expires_at>$2`, queueNumber(event.RecipientVoidID)},
	} {
		var count, size int64
		if err = tx.QueryRow(ctx, check.query, check.id, event.CreatedAt).Scan(&count, &size); err != nil {
			return queueError(ctx, err)
		}
		if count >= queue.MaxPendingPerAccount || size+int64(len(event.Payload)) > queue.MaxPendingBytesPerAccount {
			return queue.ErrCapacity
		}
	}
	tag, err := tx.Exec(ctx, `INSERT INTO public.messages(event_id,sender_void_id,recipient_void_id,payload,created_at,expires_at) VALUES($1,$2,$3,$4,$5,$6) ON CONFLICT(sender_void_id,event_id) DO NOTHING`, queueUUID(event.EventID), queueNumber(event.SenderVoidID), queueNumber(event.RecipientVoidID), event.Payload, event.CreatedAt, event.ExpiresAt)
	if err != nil {
		return queueError(ctx, err)
	}
	if tag.RowsAffected() != 1 {
		return queue.ErrDuplicate
	}
	// Acceptance is returned only after the database confirms commit.
	return queueError(ctx, tx.Commit(ctx))
}

func (s *QueueStore) Pending(ctx context.Context, recipient string, now time.Time, limit int) ([]queue.Event, error) {
	if !auth.ValidVoidID(recipient) || limit < 1 || limit > queue.MaxBatch {
		return nil, queue.ErrInvalid
	}
	if s.db == nil {
		return nil, queue.ErrUnavailable
	}
	rows, err := s.db.Query(ctx, `SELECT event_id::text,sender_void_id::text,recipient_void_id::text,payload,created_at,expires_at FROM public.messages WHERE recipient_void_id=$1 AND expires_at>$2 ORDER BY created_at,sender_void_id,event_id LIMIT $3`, queueNumber(recipient), now, limit)
	if err != nil {
		return nil, queueError(ctx, err)
	}
	defer rows.Close()
	out := []queue.Event{}
	for rows.Next() {
		var event queue.Event
		if err = rows.Scan(&event.EventID, &event.SenderVoidID, &event.RecipientVoidID, &event.Payload, &event.CreatedAt, &event.ExpiresAt); err != nil {
			return nil, queueError(ctx, err)
		}
		event.CreatedAt = event.CreatedAt.UTC()
		event.ExpiresAt = event.ExpiresAt.UTC()
		event.Payload = append([]byte(nil), event.Payload...)
		out = append(out, event)
	}
	if err = rows.Err(); err != nil {
		return nil, queueError(ctx, err)
	}
	return out, nil
}

func (s *QueueStore) Ack(ctx context.Context, recipient, sender, eventID string) error {
	if !auth.ValidVoidID(recipient) || !auth.ValidVoidID(sender) || !queue.ValidEventID(eventID) {
		return queue.ErrInvalid
	}
	if s.db == nil {
		return queue.ErrUnavailable
	}
	_, err := s.db.Exec(ctx, `DELETE FROM public.messages WHERE recipient_void_id=$1 AND sender_void_id=$2 AND event_id=$3`, queueNumber(recipient), queueNumber(sender), queueUUID(eventID))
	return queueError(ctx, err)
}

func (s *QueueStore) Expire(ctx context.Context, now time.Time, limit int) (int64, error) {
	if limit < 1 || limit > 1000 {
		return 0, queue.ErrInvalid
	}
	if s.db == nil {
		return 0, queue.ErrUnavailable
	}
	// A plain bounded selection needs only SELECT/DELETE access. FOR UPDATE
	// would also require UPDATE privileges. The single cleanup worker relies on
	// the connection's lock/statement deadlines to bound contention with ACKs.
	tag, err := s.db.Exec(ctx, `WITH expired AS (SELECT sender_void_id,event_id FROM public.messages WHERE expires_at<=$1 ORDER BY expires_at,sender_void_id,event_id LIMIT $2) DELETE FROM public.messages m USING expired e WHERE m.sender_void_id=e.sender_void_id AND m.event_id=e.event_id AND m.expires_at<=$1`, now, limit)
	if err != nil {
		return 0, queueError(ctx, err)
	}
	return tag.RowsAffected(), nil
}

// Validate is read-only. It permits the base schema or the reviewed prekey extension,
// whose additional tables are separately checked by PrekeyStore.Validate. It verifies payload/expiry bounds,
// ownership FKs, composite key, delivery/expiry indexes and effective privileges.
func (s *QueueStore) Validate(ctx context.Context) error {
	if s.db == nil {
		return ErrQueueSchema
	}
	var okay bool
	err := s.db.QueryRow(ctx, `SELECT c.relkind='r' AND c.relpersistence='p' AND NOT c.relrowsecurity AND has_table_privilege(c.oid,'SELECT') AND has_table_privilege(c.oid,'INSERT') AND has_table_privilege(c.oid,'DELETE') AND (SELECT array_agg(t.relname::text ORDER BY t.relname COLLATE "C") IN (ARRAY['accounts','messages']::text[],ARRAY['accounts','messages','signal_prekey_claims','signal_prekey_inventory']::text[]) FROM pg_catalog.pg_class t JOIN pg_catalog.pg_namespace n ON n.oid=t.relnamespace WHERE n.nspname='public' AND t.relkind IN ('r','p')) AND (SELECT count(*)=6 AND bool_and(a.attnotnull AND a.attidentity='' AND a.attgenerated='' AND NOT a.atthasdef AND a.atttypid=CASE a.attname WHEN 'event_id' THEN 'uuid'::regtype WHEN 'sender_void_id' THEN 'bigint'::regtype WHEN 'recipient_void_id' THEN 'bigint'::regtype WHEN 'payload' THEN 'bytea'::regtype WHEN 'created_at' THEN 'timestamptz'::regtype WHEN 'expires_at' THEN 'timestamptz'::regtype ELSE 0::oid END) FROM pg_catalog.pg_attribute a WHERE a.attrelid=c.oid AND a.attnum>0 AND NOT a.attisdropped) FROM pg_catalog.pg_class c WHERE c.oid=pg_catalog.to_regclass('public.messages')`).Scan(&okay)
	if err != nil || !okay {
		return ErrQueueSchema
	}
	rows, err := s.db.Query(ctx, `SELECT c.contype::text,c.convalidated AND c.conenforced,c.condeferrable,ARRAY(SELECT a.attname::text FROM unnest(c.conkey) WITH ORDINALITY k(num,ord) JOIN pg_catalog.pg_attribute a ON a.attrelid=c.conrelid AND a.attnum=k.num ORDER BY k.ord),COALESCE(c.confrelid=pg_catalog.to_regclass('public.accounts') AND c.confupdtype='a' AND c.confdeltype='a' AND c.confmatchtype='s' AND c.confkey=ARRAY[(SELECT attnum FROM pg_catalog.pg_attribute WHERE attrelid=c.confrelid AND attname='void_id')]::smallint[],false),COALESCE(pg_catalog.pg_get_expr(c.conbin,c.conrelid),'') FROM pg_catalog.pg_constraint c WHERE c.conrelid=pg_catalog.to_regclass('public.messages') AND c.contype IN ('p','u','f','c','x')`)
	if err != nil {
		return ErrQueueSchema
	}
	seen := map[string]bool{}
	for rows.Next() {
		var kind, expression string
		var valid, deferred, accountFK bool
		var columns []string
		if err = rows.Scan(&kind, &valid, &deferred, &columns, &accountFK, &expression); err != nil {
			rows.Close()
			return ErrQueueSchema
		}
		key := ""
		switch {
		case kind == "p" && strings.Join(columns, ",") == "sender_void_id,event_id":
			key = "primary"
		case kind == "f" && accountFK && len(columns) == 1 && (columns[0] == "sender_void_id" || columns[0] == "recipient_void_id"):
			key = columns[0]
		case kind == "c":
			switch normalizeCheck(expression) {
			case "octet_lengthpayload>=1andoctet_lengthpayload<=1048576":
				key = "payload"
			case "expires_at=created_at+'720:00:00'::interval", "expires_at=created_at+'720hours'::interval":
				key = "expiry"
			}
		}
		if key == "" || !valid || deferred || seen[key] {
			rows.Close()
			return ErrQueueSchema
		}
		seen[key] = true
	}
	err = rows.Err()
	rows.Close()
	if err != nil || len(seen) != 5 {
		return ErrQueueSchema
	}
	rows, err = s.db.Query(ctx, `SELECT ARRAY(SELECT a.attname::text FROM unnest(i.indkey::smallint[]) WITH ORDINALITY k(num,ord) JOIN pg_catalog.pg_attribute a ON a.attrelid=i.indrelid AND a.attnum=k.num ORDER BY k.ord),i.indisvalid AND i.indisready AND i.indpred IS NULL AND i.indexprs IS NULL AND i.indnkeyatts=i.indnatts AND am.amname='btree' FROM pg_catalog.pg_index i JOIN pg_catalog.pg_class c ON c.oid=i.indexrelid JOIN pg_catalog.pg_am am ON am.oid=c.relam WHERE i.indrelid=pg_catalog.to_regclass('public.messages')`)
	if err != nil {
		return ErrQueueSchema
	}
	var delivery, expiry bool
	for rows.Next() {
		var columns []string
		var usable bool
		if err = rows.Scan(&columns, &usable); err != nil {
			rows.Close()
			return ErrQueueSchema
		}
		if usable {
			switch strings.Join(columns, ",") {
			case "recipient_void_id,created_at,sender_void_id,event_id":
				delivery = true
			case "expires_at,sender_void_id,event_id":
				expiry = true
			}
		}
	}
	err = rows.Err()
	rows.Close()
	if err != nil || !delivery || !expiry {
		return ErrQueueSchema
	}
	return nil
}

package postgres

import (
	"context"
	"encoding/base64"
	"encoding/hex"
	"encoding/json"
	"errors"
	"strings"
	"time"

	"github.com/jackc/pgx/v5"
	"github.com/jackc/pgx/v5/pgxpool"
	"void-nothingness/internal/auth"
	"void-nothingness/internal/prekeys"
	"void-nothingness/internal/queue"
)

var ErrPrekeySchema = errors.New("postgres: reviewed public prekey schema or required access is missing")

// These bigint advisory locks occupy a different lock space from the message
// queue's pair-of-int32 account locks. They serialize only prekey allocations.
const (
	prekeyInventoryLock int64 = 0x5052454b50554201
	prekeyClaimLock     int64 = 0x5052454b434c4d01
)

// PrekeyStore persists public prekeys and bounded, retry-safe claim records.
// It neither creates schema objects nor stores private Signal state.
type PrekeyStore struct{ db queueDB }

func NewPrekeyStore(pool *pgxpool.Pool) *PrekeyStore {
	if pool == nil {
		return &PrekeyStore{}
	}
	return &PrekeyStore{db: pool}
}

var _ prekeys.Store = (*PrekeyStore)(nil)

func prekeyError(ctx context.Context, err error) error {
	if err == nil {
		return nil
	}
	if ctx.Err() != nil {
		return ctx.Err()
	}
	for _, known := range []error{prekeys.ErrInvalid, prekeys.ErrConflict, prekeys.ErrCapacity, prekeys.ErrExpired, prekeys.ErrUnavailable} {
		if errors.Is(err, known) {
			return known
		}
	}
	// PostgreSQL diagnostics may include SQL, public keys or credentials.
	return prekeys.ErrUnavailable
}

func beginPrekeyTx(ctx context.Context, db queueDB, lock int64) (pgx.Tx, error) {
	tx, err := db.BeginTx(ctx, pgx.TxOptions{IsoLevel: pgx.ReadCommitted})
	if err != nil {
		return nil, err
	}
	for _, query := range []string{`SET LOCAL lock_timeout='5s'`, `SET LOCAL statement_timeout='15s'`} {
		if _, err = tx.Exec(ctx, query); err != nil {
			rollbackPrekeyTx(tx)
			return nil, err
		}
	}
	if _, err = tx.Exec(ctx, `SELECT pg_advisory_xact_lock($1::bigint)`, lock); err != nil {
		rollbackPrekeyTx(tx)
		return nil, err
	}
	return tx, nil
}

func rollbackPrekeyTx(tx pgx.Tx) {
	ctx, cancel := context.WithTimeout(context.Background(), 5*time.Second)
	defer cancel()
	_ = tx.Rollback(ctx)
}

const selectPrekeyInventory = `SELECT device_id::integer,registration_id,identity_key,account_signature,signed_prekey,kyber_last_resort,one_time_prekeys,one_time_kyber_prekeys,max_ec_prekey_id,max_kyber_prekey_id,publication_history,created_at,updated_at FROM public.signal_prekey_inventory WHERE void_id=$1 FOR UPDATE`

func readPrekeyInventory(row pgx.Row) (prekeys.Inventory, error) {
	var inventory prekeys.Inventory
	var identity, signature, signed, fallback, ec, kyber, publications []byte
	err := row.Scan(&inventory.Upload.DeviceID, &inventory.Upload.RegistrationID, &identity, &signature, &signed, &fallback, &ec, &kyber, &inventory.MaxECID, &inventory.MaxKyberID, &publications, &inventory.CreatedAt, &inventory.UpdatedAt)
	if err != nil {
		return inventory, err
	}
	inventory.Upload.V = 1
	inventory.Upload.IdentityKey = base64.StdEncoding.EncodeToString(identity)
	inventory.Upload.AccountSignature = base64.StdEncoding.EncodeToString(signature)
	for _, value := range []struct {
		data []byte
		dest any
	}{{signed, &inventory.Upload.SignedPrekey}, {fallback, &inventory.Upload.KyberLastResort}, {ec, &inventory.Upload.OneTimePrekeys}, {kyber, &inventory.Upload.OneTimeKyberPrekeys}, {publications, &inventory.Publications}} {
		if err = json.Unmarshal(value.data, value.dest); err != nil {
			return prekeys.Inventory{}, err
		}
	}
	inventory.CreatedAt = inventory.CreatedAt.UTC()
	inventory.UpdatedAt = inventory.UpdatedAt.UTC()
	return inventory, nil
}

func marshalPrekeyValues(values ...any) ([][]byte, error) {
	data := make([][]byte, len(values))
	for i, value := range values {
		var err error
		data[i], err = json.Marshal(value)
		if err != nil {
			return nil, err
		}
	}
	return data, nil
}

func (s *PrekeyStore) Publish(ctx context.Context, actor, publicationID string, upload prekeys.Upload, hash [32]byte, now time.Time) (prekeys.Counts, error) {
	if !auth.ValidVoidID(actor) || !queue.ValidEventID(publicationID) || now.IsZero() {
		return prekeys.Counts{}, prekeys.ErrInvalid
	}
	if s.db == nil {
		return prekeys.Counts{}, prekeys.ErrUnavailable
	}
	now = now.UTC().Truncate(time.Microsecond)
	tx, err := beginPrekeyTx(ctx, s.db, prekeyInventoryLock)
	if err != nil {
		return prekeys.Counts{}, prekeyError(ctx, err)
	}
	defer rollbackPrekeyTx(tx)
	current, err := readPrekeyInventory(tx.QueryRow(ctx, selectPrekeyInventory, queueNumber(actor)))
	var existing *prekeys.Inventory
	if err == nil {
		existing = &current
	} else if !errors.Is(err, pgx.ErrNoRows) {
		return prekeys.Counts{}, prekeyError(ctx, err)
	}
	next, counts, err := prekeys.ApplyPublication(existing, publicationID, upload, hash, now)
	if err != nil {
		return prekeys.Counts{}, prekeyError(ctx, err)
	}
	if existing != nil && existing.Publications[publicationID] == hex.EncodeToString(hash[:]) {
		// Retrying an older publication must not resurrect consumed keys or
		// replace the most recent publication's metadata.
		if err = tx.Commit(ctx); err != nil {
			return prekeys.Counts{}, prekeyError(ctx, err)
		}
		return counts, nil
	}
	if existing == nil {
		var count int64
		if err = tx.QueryRow(ctx, `SELECT count(*) FROM public.signal_prekey_inventory`).Scan(&count); err != nil {
			return prekeys.Counts{}, prekeyError(ctx, err)
		}
		if count >= prekeys.MaxInventories {
			return prekeys.Counts{}, prekeys.ErrCapacity
		}
	}
	identity, err := base64.StdEncoding.Strict().DecodeString(next.Upload.IdentityKey)
	if err != nil {
		return prekeys.Counts{}, prekeys.ErrInvalid
	}
	signature, err := base64.StdEncoding.Strict().DecodeString(next.Upload.AccountSignature)
	if err != nil {
		return prekeys.Counts{}, prekeys.ErrInvalid
	}
	data, err := marshalPrekeyValues(next.Upload.SignedPrekey, next.Upload.KyberLastResort, next.Upload.OneTimePrekeys, next.Upload.OneTimeKyberPrekeys, next.Publications)
	if err != nil {
		return prekeys.Counts{}, prekeyError(ctx, err)
	}
	tag, err := tx.Exec(ctx, `INSERT INTO public.signal_prekey_inventory(void_id,device_id,registration_id,identity_key,account_signature,signed_prekey,kyber_last_resort,one_time_prekeys,one_time_kyber_prekeys,max_ec_prekey_id,max_kyber_prekey_id,publication_id,publication_hash,publication_history,created_at,updated_at) VALUES($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11,$12,$13,$14,$15,$16) ON CONFLICT(void_id) DO UPDATE SET one_time_prekeys=EXCLUDED.one_time_prekeys,one_time_kyber_prekeys=EXCLUDED.one_time_kyber_prekeys,max_ec_prekey_id=EXCLUDED.max_ec_prekey_id,max_kyber_prekey_id=EXCLUDED.max_kyber_prekey_id,publication_id=EXCLUDED.publication_id,publication_hash=EXCLUDED.publication_hash,publication_history=EXCLUDED.publication_history,updated_at=EXCLUDED.updated_at`, queueNumber(actor), next.Upload.DeviceID, next.Upload.RegistrationID, identity, signature, data[0], data[1], data[2], data[3], next.MaxECID, next.MaxKyberID, queueUUID(publicationID), hash[:], data[4], next.CreatedAt, next.UpdatedAt)
	if err != nil {
		return prekeys.Counts{}, prekeyError(ctx, err)
	}
	if tag.RowsAffected() != 1 {
		return prekeys.Counts{}, prekeys.ErrUnavailable
	}
	if err = tx.Commit(ctx); err != nil {
		return prekeys.Counts{}, prekeyError(ctx, err)
	}
	return counts, nil
}

func readPrekeyClaim(ctx context.Context, tx pgx.Tx, actor, requestID, target string, now time.Time) (prekeys.Bundle, error) {
	var recipient *int64
	var data []byte
	var expires time.Time
	err := tx.QueryRow(ctx, `SELECT recipient_void_id,public_bundle,claim_expires_at FROM public.signal_prekey_claims WHERE requester_void_id=$1 AND request_id=$2`, queueNumber(actor), queueUUID(requestID)).Scan(&recipient, &data, &expires)
	if err != nil {
		return prekeys.Bundle{}, err
	}
	// Scrubbed/expired UUIDs are terminal regardless of the submitted recipient.
	if !now.Before(expires) || recipient == nil || len(data) == 0 {
		return prekeys.Bundle{}, prekeys.ErrExpired
	}
	if *recipient != queueNumber(target) {
		return prekeys.Bundle{}, prekeys.ErrConflict
	}
	var bundle prekeys.Bundle
	if err = json.Unmarshal(data, &bundle); err != nil {
		return prekeys.Bundle{}, err
	}
	bundle.ClaimExpiresAt = expires.UTC()
	return bundle, nil
}

func (s *PrekeyStore) Claim(ctx context.Context, actor, requestID, target string, now time.Time) (prekeys.Bundle, error) {
	if !auth.ValidVoidID(actor) || !auth.ValidVoidID(target) || actor == target || !queue.ValidEventID(requestID) || now.IsZero() {
		return prekeys.Bundle{}, prekeys.ErrInvalid
	}
	if s.db == nil {
		return prekeys.Bundle{}, prekeys.ErrUnavailable
	}
	now = now.UTC().Truncate(time.Microsecond)
	tx, err := beginPrekeyTx(ctx, s.db, prekeyClaimLock)
	if err != nil {
		return prekeys.Bundle{}, prekeyError(ctx, err)
	}
	defer rollbackPrekeyTx(tx)
	if bundle, claimErr := readPrekeyClaim(ctx, tx, actor, requestID, target, now); !errors.Is(claimErr, pgx.ErrNoRows) {
		if claimErr != nil {
			return prekeys.Bundle{}, prekeyError(ctx, claimErr)
		}
		if err = tx.Commit(ctx); err != nil {
			return prekeys.Bundle{}, prekeyError(ctx, err)
		}
		return bundle, nil
	}
	var exists bool
	if err = tx.QueryRow(ctx, `SELECT EXISTS(SELECT 1 FROM public.accounts WHERE void_id=$1)`, queueNumber(target)).Scan(&exists); err != nil {
		return prekeys.Bundle{}, prekeyError(ctx, err)
	}
	if !exists {
		return prekeys.Bundle{}, prekeys.ErrInvalid
	}
	current, err := readPrekeyInventory(tx.QueryRow(ctx, selectPrekeyInventory, queueNumber(target)))
	if err != nil {
		return prekeys.Bundle{}, prekeyError(ctx, err)
	}
	// Recheck after locking inventory before allocating any one-time keys.
	if bundle, claimErr := readPrekeyClaim(ctx, tx, actor, requestID, target, now); !errors.Is(claimErr, pgx.ErrNoRows) {
		if claimErr != nil {
			return prekeys.Bundle{}, prekeyError(ctx, claimErr)
		}
		if err = tx.Commit(ctx); err != nil {
			return prekeys.Bundle{}, prekeyError(ctx, err)
		}
		return bundle, nil
	}
	var personal, total int64
	if err = tx.QueryRow(ctx, `SELECT count(*) FILTER (WHERE requester_void_id=$1),count(*) FROM public.signal_prekey_claims`, queueNumber(actor)).Scan(&personal, &total); err != nil {
		return prekeys.Bundle{}, prekeyError(ctx, err)
	}
	if personal >= prekeys.MaxClaimsPerAccount || total >= prekeys.MaxClaimsGlobal {
		return prekeys.Bundle{}, prekeys.ErrCapacity
	}
	next, bundle, err := prekeys.SelectBundle(current, now)
	if err != nil {
		return prekeys.Bundle{}, prekeyError(ctx, err)
	}
	data, err := marshalPrekeyValues(next.Upload.OneTimePrekeys, next.Upload.OneTimeKyberPrekeys, bundle)
	if err != nil {
		return prekeys.Bundle{}, prekeyError(ctx, err)
	}
	tag, err := tx.Exec(ctx, `UPDATE public.signal_prekey_inventory SET one_time_prekeys=$2,one_time_kyber_prekeys=$3,updated_at=$4 WHERE void_id=$1`, queueNumber(target), data[0], data[1], next.UpdatedAt)
	if err != nil {
		return prekeys.Bundle{}, prekeyError(ctx, err)
	}
	if tag.RowsAffected() != 1 {
		return prekeys.Bundle{}, prekeys.ErrUnavailable
	}
	tag, err = tx.Exec(ctx, `INSERT INTO public.signal_prekey_claims(requester_void_id,request_id,recipient_void_id,device_id,public_bundle,claimed_at,claim_expires_at) VALUES($1,$2,$3,$4,$5,$6,$7)`, queueNumber(actor), queueUUID(requestID), queueNumber(target), bundle.DeviceID, data[2], now, bundle.ClaimExpiresAt)
	if err != nil {
		return prekeys.Bundle{}, prekeyError(ctx, err)
	}
	if tag.RowsAffected() != 1 {
		return prekeys.Bundle{}, prekeys.ErrUnavailable
	}
	if err = tx.Commit(ctx); err != nil {
		return prekeys.Bundle{}, prekeyError(ctx, err)
	}
	return bundle, nil
}

func (s *PrekeyStore) Expire(ctx context.Context, now time.Time, limit int) (int64, error) {
	if now.IsZero() || limit < 1 || limit > 1000 {
		return 0, prekeys.ErrInvalid
	}
	if s.db == nil {
		return 0, prekeys.ErrUnavailable
	}
	// Retain only requester + UUID + deadline tombstones. Removing these rows
	// would let a delayed retry consume another one-time key.
	tag, err := s.db.Exec(ctx, `WITH expired AS (SELECT requester_void_id,request_id FROM public.signal_prekey_claims WHERE claim_expires_at<=$1 AND public_bundle IS NOT NULL ORDER BY claim_expires_at,requester_void_id,request_id LIMIT $2) UPDATE public.signal_prekey_claims c SET recipient_void_id=NULL,public_bundle=NULL FROM expired e WHERE c.requester_void_id=e.requester_void_id AND c.request_id=e.request_id AND c.claim_expires_at<=$1 AND c.public_bundle IS NOT NULL`, now, limit)
	if err != nil {
		return 0, prekeyError(ctx, err)
	}
	return tag.RowsAffected(), nil
}

type prekeyColumn struct {
	typ, def string
	notNull  bool
	update   bool
}

var prekeyInventoryColumns = map[string]prekeyColumn{
	"void_id":                {"int8", "", true, false},
	"device_id":              {"int2", "1", true, false},
	"registration_id":        {"int4", "", true, false},
	"identity_key":           {"bytea", "", true, false},
	"account_signature":      {"bytea", "", true, false},
	"signed_prekey":          {"jsonb", "", true, false},
	"kyber_last_resort":      {"jsonb", "", true, false},
	"one_time_prekeys":       {"jsonb", "'[]'::jsonb", true, true},
	"one_time_kyber_prekeys": {"jsonb", "'[]'::jsonb", true, true},
	"max_ec_prekey_id":       {"int4", "0", true, true},
	"max_kyber_prekey_id":    {"int4", "0", true, true},
	"publication_id":         {"uuid", "", true, true},
	"publication_hash":       {"bytea", "", true, true},
	"publication_history":    {"jsonb", "", true, true},
	"created_at":             {"timestamptz", "now()", true, false},
	"updated_at":             {"timestamptz", "now()", true, true},
}

var prekeyClaimColumns = map[string]prekeyColumn{
	"requester_void_id": {"int8", "", true, false},
	"request_id":        {"uuid", "", true, false},
	"recipient_void_id": {"int8", "", false, true},
	"device_id":         {"int2", "1", true, false},
	"public_bundle":     {"jsonb", "", false, true},
	"claimed_at":        {"timestamptz", "now()", true, false},
	"claim_expires_at":  {"timestamptz", "", true, false},
}

var prekeyInventoryChecks = []string{
	`device_id = 1`,
	`registration_id >= 1 AND registration_id <= 16380`,
	`octet_length(identity_key) = 33`,
	`octet_length(account_signature) = 64`,
	`jsonb_typeof(signed_prekey) = 'object' AND octet_length(signed_prekey::text) <= 8192`,
	`jsonb_typeof(kyber_last_resort) = 'object' AND octet_length(kyber_last_resort::text) <= 8192`,
	`jsonb_typeof(one_time_prekeys) = 'array' AND jsonb_array_length(one_time_prekeys) <= 64 AND octet_length(one_time_prekeys::text) <= 32768`,
	`jsonb_typeof(one_time_kyber_prekeys) = 'array' AND jsonb_array_length(one_time_kyber_prekeys) <= 64 AND octet_length(one_time_kyber_prekeys::text) <= 262144`,
	`max_ec_prekey_id >= 0 AND max_ec_prekey_id <= 16777214`,
	`max_kyber_prekey_id >= 0 AND max_kyber_prekey_id <= 16777214`,
	`publication_id <> '00000000-0000-0000-0000-000000000000'::uuid`,
	`octet_length(publication_hash) = 32`,
	`jsonb_typeof(publication_history) = 'object' AND octet_length(publication_history::text) <= 8192 AND publication_history ? publication_id::text AND publication_history ->> publication_id::text = encode(publication_hash, 'hex')`,
	`updated_at >= created_at`,
}

var prekeyClaimChecks = []string{
	`request_id <> '00000000-0000-0000-0000-000000000000'::uuid`,
	`device_id = 1`,
	`public_bundle IS NULL AND recipient_void_id IS NULL OR public_bundle IS NOT NULL AND recipient_void_id IS NOT NULL AND jsonb_typeof(public_bundle) = 'object' AND octet_length(public_bundle::text) <= 32768`,
	`requester_void_id <> recipient_void_id`,
	`claim_expires_at = claimed_at + '24:00:00'::interval`,
}

// Validate inspects catalogs and effective column grants without changing data.
// QueueStore separately validates the complete permitted public table inventory.
func (s *PrekeyStore) Validate(ctx context.Context) error {
	if s.db == nil {
		return ErrPrekeySchema
	}
	for _, table := range []struct {
		name    string
		columns map[string]prekeyColumn
		checks  []string
		primary string
		foreign []string
	}{
		{"signal_prekey_inventory", prekeyInventoryColumns, prekeyInventoryChecks, "void_id", []string{"void_id"}},
		{"signal_prekey_claims", prekeyClaimColumns, prekeyClaimChecks, "requester_void_id,request_id", []string{"requester_void_id", "recipient_void_id"}},
	} {
		name := "public." + table.name
		var valid bool
		err := s.db.QueryRow(ctx, `SELECT c.relkind='r' AND c.relpersistence='p' AND NOT c.relrowsecurity FROM pg_catalog.pg_class c WHERE c.oid=pg_catalog.to_regclass($1)`, name).Scan(&valid)
		if err != nil || !valid {
			return ErrPrekeySchema
		}
		rows, err := s.db.Query(ctx, `SELECT a.attname,t.typname,a.atttypmod,a.attnotnull,COALESCE(pg_catalog.pg_get_expr(d.adbin,d.adrelid),''),a.attidentity<>'' OR a.attgenerated<>'',has_column_privilege(c.oid,a.attnum,'SELECT'),has_column_privilege(c.oid,a.attnum,'INSERT'),has_column_privilege(c.oid,a.attnum,'UPDATE') FROM pg_catalog.pg_attribute a JOIN pg_catalog.pg_class c ON c.oid=a.attrelid JOIN pg_catalog.pg_type t ON t.oid=a.atttypid LEFT JOIN pg_catalog.pg_attrdef d ON d.adrelid=a.attrelid AND d.adnum=a.attnum WHERE c.oid=pg_catalog.to_regclass($1) AND a.attnum>0 AND NOT a.attisdropped`, name)
		if err != nil {
			return ErrPrekeySchema
		}
		seen := map[string]bool{}
		for rows.Next() {
			var column, typ, def string
			var modifier int32
			var notNull, generated, selectable, insertable, updatable bool
			if err = rows.Scan(&column, &typ, &modifier, &notNull, &def, &generated, &selectable, &insertable, &updatable); err != nil {
				rows.Close()
				return ErrPrekeySchema
			}
			want, known := table.columns[column]
			if !known || seen[column] || typ != want.typ || def != want.def || notNull != want.notNull || modifier != -1 || generated || !selectable || !insertable || (want.update && !updatable) {
				rows.Close()
				return ErrPrekeySchema
			}
			seen[column] = true
		}
		err = rows.Err()
		rows.Close()
		if err != nil || len(seen) != len(table.columns) {
			return ErrPrekeySchema
		}
		if err = s.validatePrekeyConstraints(ctx, name, table.checks, table.primary, table.foreign); err != nil {
			return err
		}
	}
	rows, err := s.db.Query(ctx, `SELECT ARRAY(SELECT a.attname::text FROM unnest(i.indkey::smallint[]) WITH ORDINALITY k(num,ord) JOIN pg_catalog.pg_attribute a ON a.attrelid=i.indrelid AND a.attnum=k.num ORDER BY k.ord),i.indisvalid AND i.indisready AND i.indexprs IS NULL AND i.indnkeyatts=i.indnatts AND am.amname='btree',COALESCE(pg_catalog.pg_get_expr(i.indpred,i.indrelid),'') FROM pg_catalog.pg_index i JOIN pg_catalog.pg_class c ON c.oid=i.indexrelid JOIN pg_catalog.pg_am am ON am.oid=c.relam WHERE i.indrelid=pg_catalog.to_regclass('public.signal_prekey_claims')`)
	if err != nil {
		return ErrPrekeySchema
	}
	var expiry bool
	for rows.Next() {
		var columns []string
		var usable bool
		var predicate string
		if err = rows.Scan(&columns, &usable, &predicate); err != nil {
			rows.Close()
			return ErrPrekeySchema
		}
		if usable && strings.Join(columns, ",") == "claim_expires_at,requester_void_id,request_id" && normalizeCheck(predicate) == "public_bundleisnotnull" {
			expiry = true
		}
	}
	err = rows.Err()
	rows.Close()
	if err != nil || !expiry {
		return ErrPrekeySchema
	}
	return nil
}

func (s *PrekeyStore) validatePrekeyConstraints(ctx context.Context, table string, checks []string, primary string, foreign []string) error {
	wanted := map[string]bool{"p:" + primary: true}
	for _, column := range foreign {
		wanted["f:"+column] = true
	}
	for _, expression := range checks {
		wanted["c:"+normalizeCheck(expression)] = true
	}
	rows, err := s.db.Query(ctx, `SELECT c.contype::text,c.convalidated AND c.conenforced,c.condeferrable,ARRAY(SELECT a.attname::text FROM unnest(c.conkey) WITH ORDINALITY k(num,ord) JOIN pg_catalog.pg_attribute a ON a.attrelid=c.conrelid AND a.attnum=k.num ORDER BY k.ord),COALESCE(c.confrelid=pg_catalog.to_regclass('public.accounts') AND c.confupdtype='a' AND c.confdeltype='a' AND c.confmatchtype='s' AND c.confkey=ARRAY[(SELECT attnum FROM pg_catalog.pg_attribute WHERE attrelid=c.confrelid AND attname='void_id')]::smallint[],false),COALESCE(pg_catalog.pg_get_expr(c.conbin,c.conrelid),'') FROM pg_catalog.pg_constraint c WHERE c.conrelid=pg_catalog.to_regclass($1) AND c.contype IN ('p','u','f','c','x')`, table)
	if err != nil {
		return ErrPrekeySchema
	}
	seen := map[string]bool{}
	for rows.Next() {
		var kind, expression string
		var valid, deferred, accountFK bool
		var columns []string
		if err = rows.Scan(&kind, &valid, &deferred, &columns, &accountFK, &expression); err != nil {
			rows.Close()
			return ErrPrekeySchema
		}
		key := kind + ":" + strings.Join(columns, ",")
		if kind == "c" {
			key = "c:" + normalizeCheck(expression)
			key = strings.ReplaceAll(key, "'24hours'::interval", "'24:00:00'::interval")
		}
		if !valid || deferred || (kind == "f" && !accountFK) || !wanted[key] || seen[key] {
			rows.Close()
			return ErrPrekeySchema
		}
		seen[key] = true
	}
	err = rows.Err()
	rows.Close()
	if err != nil || len(seen) != len(wanted) {
		return ErrPrekeySchema
	}
	return nil
}

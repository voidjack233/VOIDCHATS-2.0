package postgres

import (
	"bytes"
	"context"
	"crypto/sha256"
	"encoding/base64"
	"encoding/json"
	"errors"
	"reflect"
	"strings"
	"testing"
	"time"

	"github.com/jackc/pgx/v5"
	"github.com/jackc/pgx/v5/pgconn"
	"void-nothingness/internal/prekeys"
)

const (
	prekeyTestPublication = "00000000-0000-4000-8000-000000000001"
	prekeyTestRequest     = "00000000-0000-4000-8000-000000000002"
)

func prekeySampleUpload() prekeys.Upload {
	ec := base64.StdEncoding.EncodeToString(append([]byte{5}, bytes.Repeat([]byte{23}, 32)...))
	signature := base64.StdEncoding.EncodeToString(bytes.Repeat([]byte{31}, 64))
	kyber := base64.StdEncoding.EncodeToString(bytes.Repeat([]byte{42}, 1568))
	return prekeys.Upload{
		V: 1, DeviceID: 1, RegistrationID: 999, IdentityKey: ec, AccountSignature: signature,
		SignedPrekey:        prekeys.SignedKey{KeyID: 1, PublicKey: ec, Signature: signature},
		KyberLastResort:     prekeys.SignedKey{KeyID: 99, PublicKey: kyber, Signature: signature},
		OneTimePrekeys:      []prekeys.PublicKey{{KeyID: 1, PublicKey: ec}},
		OneTimeKyberPrekeys: []prekeys.SignedKey{{KeyID: 1, PublicKey: kyber, Signature: signature}},
	}
}

func prekeySampleInventory(t *testing.T) prekeys.Inventory {
	t.Helper()
	upload := prekeySampleUpload()
	inventory, _, err := prekeys.ApplyPublication(nil, prekeyTestPublication, upload, sha256.Sum256([]byte("publication")), queueSampleEvent().CreatedAt)
	if err != nil {
		t.Fatal(err)
	}
	return inventory
}

func prekeyInventoryRow(inventory prekeys.Inventory) pgx.Row {
	upload := inventory.Upload
	identity, _ := base64.StdEncoding.DecodeString(upload.IdentityKey)
	signature, _ := base64.StdEncoding.DecodeString(upload.AccountSignature)
	signed, _ := json.Marshal(upload.SignedPrekey)
	fallback, _ := json.Marshal(upload.KyberLastResort)
	ec, _ := json.Marshal(upload.OneTimePrekeys)
	kyber, _ := json.Marshal(upload.OneTimeKyberPrekeys)
	history, _ := json.Marshal(inventory.Publications)
	return queueRow(upload.DeviceID, upload.RegistrationID, identity, signature, signed, fallback, ec, kyber, inventory.MaxECID, inventory.MaxKyberID, history, inventory.CreatedAt, inventory.UpdatedAt)
}

func prekeyNoRow() pgx.Row {
	return queueTestRow(func(...any) error { return pgx.ErrNoRows })
}

func prekeyTestStore(rows ...pgx.Row) (*PrekeyStore, *queueTestTx) {
	tx := &queueTestTx{queueTestDB: queueTestDB{rowResults: rows, tag: pgconn.NewCommandTag("INSERT 0 1")}}
	return &PrekeyStore{db: &queueTestDB{tx: tx}}, tx
}

func TestPrekeyPublicationCommitsBeforeAcceptance(t *testing.T) {
	store, tx := prekeyTestStore(prekeyNoRow(), queueRow(int64(0)))
	upload := prekeySampleUpload()
	hash := sha256.Sum256([]byte("publication"))
	now := queueSampleEvent().CreatedAt
	counts, err := store.Publish(context.Background(), "101", prekeyTestPublication, upload, hash, now)
	if err != nil || counts != (prekeys.Counts{ECRemaining: 1, KyberRemaining: 1}) || !tx.committed || !tx.rolledBack {
		t.Fatal(counts, err, tx.committed, tx.rolledBack)
	}
	if tx.calls[2].sql != "SELECT pg_advisory_xact_lock($1::bigint)" || tx.calls[2].args[0] != prekeyInventoryLock || !strings.HasSuffix(tx.calls[3].sql, "FOR UPDATE") {
		t.Fatal("inventory allocation lock order changed", tx.calls)
	}
	insert := tx.calls[len(tx.calls)-1]
	if !strings.HasPrefix(insert.sql, "INSERT INTO public.signal_prekey_inventory(") || insert.args[0] != int64(101) || !bytes.Equal(insert.args[12].([]byte), hash[:]) || insert.args[14] != now || insert.args[15] != now {
		t.Fatal("publication data changed", insert)
	}
	update := strings.Split(insert.sql, "DO UPDATE SET ")[1]
	if strings.Contains(update, "identity_key=") || strings.Contains(update, "signed_prekey=") || strings.Contains(update, "created_at=") || strings.Contains(update, "registration_id=") {
		t.Fatal("immutable public identity fields are updated", update)
	}
}

func TestPrekeyPublicationRetriesDoNotResurrectConsumedKeys(t *testing.T) {
	inventory := prekeySampleInventory(t)
	inventory.Upload.OneTimePrekeys = []prekeys.PublicKey{}
	inventory.Upload.OneTimeKyberPrekeys = []prekeys.SignedKey{}
	for _, conflict := range []bool{false, true} {
		store, tx := prekeyTestStore(prekeyInventoryRow(inventory))
		hash := sha256.Sum256([]byte("publication"))
		if conflict {
			hash[0]++
		}
		counts, err := store.Publish(context.Background(), "101", prekeyTestPublication, prekeySampleUpload(), hash, inventory.CreatedAt.Add(time.Hour))
		if conflict {
			if !errors.Is(err, prekeys.ErrConflict) || tx.committed {
				t.Fatal("changed bytes reused publication UUID", err)
			}
		} else if err != nil || counts != (prekeys.Counts{}) || !tx.committed {
			t.Fatal("exact old publication retry", counts, err)
		}
		if len(tx.calls) != 4 || !tx.rolledBack {
			t.Fatal("retry rewrote publication or consumed keys", tx.calls)
		}
	}
}

func TestPrekeyPublicationQuotaAndCommitFailureDoNotAccept(t *testing.T) {
	for _, tc := range []struct {
		name   string
		count  int64
		commit error
		want   error
	}{
		{"global_inventory_limit", prekeys.MaxInventories, nil, prekeys.ErrCapacity},
		{"commit_failure", 0, errors.New("private DSN and public key"), prekeys.ErrUnavailable},
	} {
		t.Run(tc.name, func(t *testing.T) {
			store, tx := prekeyTestStore(prekeyNoRow(), queueRow(tc.count))
			tx.commitErr = tc.commit
			counts, err := store.Publish(context.Background(), "101", prekeyTestPublication, prekeySampleUpload(), sha256.Sum256([]byte("publication")), queueSampleEvent().CreatedAt)
			if !errors.Is(err, tc.want) || counts != (prekeys.Counts{}) || tx.committed || !tx.rolledBack || (tc.commit != nil && errors.Is(err, tc.commit)) {
				t.Fatal(counts, err, tx.committed, tx.rolledBack)
			}
		})
	}
}

func prekeyNewClaimStore(t *testing.T) (*PrekeyStore, *queueTestTx, prekeys.Inventory) {
	t.Helper()
	inventory := prekeySampleInventory(t)
	store, tx := prekeyTestStore(prekeyNoRow(), queueRow(true), prekeyInventoryRow(inventory), prekeyNoRow(), queueRow(int64(0), int64(0)))
	return store, tx, inventory
}

func TestPrekeyClaimConsumesAndPersistsAtomicallyBeforeResponse(t *testing.T) {
	store, tx, inventory := prekeyNewClaimStore(t)
	now := inventory.CreatedAt.Add(time.Minute)
	bundle, err := store.Claim(context.Background(), "102", prekeyTestRequest, "101", now)
	if err != nil || !tx.committed || !tx.rolledBack || bundle.OneTimePrekey == nil || bundle.KyberPrekey.LastResort || bundle.ClaimExpiresAt != now.Add(prekeys.ClaimTTL) {
		t.Fatal(bundle, err, tx.committed, tx.rolledBack)
	}
	if tx.calls[2].sql != "SELECT pg_advisory_xact_lock($1::bigint)" || tx.calls[2].args[0] != prekeyClaimLock || prekeyClaimLock == prekeyInventoryLock {
		t.Fatal("claim allocator lock shares inventory namespace", tx.calls[2])
	}
	if !strings.HasSuffix(tx.calls[5].sql, "FOR UPDATE") || !strings.Contains(tx.calls[6].sql, "WHERE requester_void_id=$1 AND request_id=$2") || strings.Contains(tx.calls[7].sql, "expires") {
		t.Fatal("claim recheck or lifetime capacity count missing", tx.calls)
	}
	update, insert := tx.calls[8], tx.calls[9]
	if !strings.HasPrefix(update.sql, "UPDATE public.signal_prekey_inventory SET ") || string(update.args[1].([]byte)) != "[]" || string(update.args[2].([]byte)) != "[]" {
		t.Fatal("claim failed to remove exactly the selected one-time keys", update)
	}
	if !strings.HasPrefix(insert.sql, "INSERT INTO public.signal_prekey_claims(") || insert.args[0] != int64(102) || insert.args[2] != int64(101) || insert.args[5] != now || insert.args[6] != now.Add(prekeys.ClaimTTL) {
		t.Fatal("persisted claim does not match committed bundle", insert)
	}
	var stored prekeys.Bundle
	if err = json.Unmarshal(insert.args[4].([]byte), &stored); err != nil || !reflect.DeepEqual(stored, bundle) {
		t.Fatal("response differs from persisted retry bundle", stored, err)
	}
}

func TestPrekeyClaimFailuresNeverReturnOrConsumeBundle(t *testing.T) {
	secret := errors.New("private SQL diagnostics")
	for _, tc := range []struct {
		name   string
		change func(*queueTestTx)
		want   error
	}{
		{"missing_account", func(tx *queueTestTx) { tx.rowResults[1] = queueRow(false) }, prekeys.ErrInvalid},
		{"unpublished_account", func(tx *queueTestTx) { tx.rowResults[2] = prekeyNoRow() }, prekeys.ErrUnavailable},
		{"requester_lifetime_cap", func(tx *queueTestTx) {
			tx.rowResults[4] = queueRow(int64(prekeys.MaxClaimsPerAccount), int64(prekeys.MaxClaimsPerAccount))
		}, prekeys.ErrCapacity},
		{"global_lifetime_cap", func(tx *queueTestTx) { tx.rowResults[4] = queueRow(int64(0), int64(prekeys.MaxClaimsGlobal)) }, prekeys.ErrCapacity},
		{"insert_failure", func(tx *queueTestTx) { tx.insertErr = secret }, prekeys.ErrUnavailable},
		{"missing_inventory_update", func(tx *queueTestTx) { tx.tag = pgconn.NewCommandTag("UPDATE 0") }, prekeys.ErrUnavailable},
		{"commit_failure", func(tx *queueTestTx) { tx.commitErr = secret }, prekeys.ErrUnavailable},
	} {
		t.Run(tc.name, func(t *testing.T) {
			store, tx, inventory := prekeyNewClaimStore(t)
			before, _ := json.Marshal(inventory)
			tc.change(tx)
			bundle, err := store.Claim(context.Background(), "102", prekeyTestRequest, "101", inventory.CreatedAt)
			after, _ := json.Marshal(inventory)
			if !errors.Is(err, tc.want) || errors.Is(err, secret) || bundle.IdentityKey != "" || tx.committed || !tx.rolledBack || !bytes.Equal(before, after) {
				t.Fatal(bundle, err, tx.committed, tx.rolledBack)
			}
		})
	}
}

func TestPrekeyClaimRetryAndExpiredTombstonesNeverConsumeAgain(t *testing.T) {
	inventory := prekeySampleInventory(t)
	now := inventory.CreatedAt
	_, original, err := prekeys.SelectBundle(inventory, now)
	if err != nil {
		t.Fatal(err)
	}
	encoded, _ := json.Marshal(original)
	recipient := int64(101)
	for _, tc := range []struct {
		name      string
		target    string
		now       time.Time
		recipient *int64
		data      []byte
		want      error
	}{
		{"exact_retry", "101", now.Add(time.Hour), &recipient, encoded, nil},
		{"wrong_recipient", "103", now.Add(time.Hour), &recipient, encoded, prekeys.ErrConflict},
		{"expired_wrong_recipient", "103", original.ClaimExpiresAt, &recipient, encoded, prekeys.ErrExpired},
		{"scrubbed_expired_retry", "103", original.ClaimExpiresAt.Add(time.Hour), nil, nil, prekeys.ErrExpired},
	} {
		t.Run(tc.name, func(t *testing.T) {
			store, tx := prekeyTestStore(queueRow(tc.recipient, tc.data, original.ClaimExpiresAt))
			bundle, err := store.Claim(context.Background(), "102", prekeyTestRequest, tc.target, tc.now)
			if !errors.Is(err, tc.want) || tx.committed != (tc.want == nil) || !tx.rolledBack || len(tx.calls) != 4 {
				t.Fatal(bundle, err, tx.calls)
			}
			if tc.want == nil {
				actual, _ := json.Marshal(bundle)
				if !bytes.Equal(actual, encoded) {
					t.Fatal("retry bytes changed", string(actual))
				}
			}
		})
	}
}

func TestPrekeyConcurrentClaimRecheckFindsExistingCommit(t *testing.T) {
	store, tx, inventory := prekeyNewClaimStore(t)
	_, original, _ := prekeys.SelectBundle(inventory, inventory.CreatedAt)
	data, _ := json.Marshal(original)
	recipient := int64(101)
	tx.rowResults[3] = queueRow(&recipient, data, original.ClaimExpiresAt)
	bundle, err := store.Claim(context.Background(), "102", prekeyTestRequest, "101", inventory.CreatedAt.Add(time.Minute))
	if err != nil || !reflect.DeepEqual(bundle, original) || !tx.committed || len(tx.calls) != 7 {
		t.Fatal("claim recheck allocated another key", bundle, err, tx.calls)
	}
}

func TestPrekeyExpiryScrubsRelationshipsAndRetainsTerminalUUIDs(t *testing.T) {
	db := &queueTestDB{tag: pgconn.NewCommandTag("UPDATE 2")}
	store := &PrekeyStore{db: db}
	now := queueSampleEvent().CreatedAt
	count, err := store.Expire(context.Background(), now, 100)
	if err != nil || count != 2 || len(db.calls) != 1 {
		t.Fatal(count, err)
	}
	call := db.calls[0]
	if strings.Contains(call.sql, "DELETE") || strings.Contains(call.sql, "FOR UPDATE") || !strings.Contains(call.sql, "SET recipient_void_id=NULL,public_bundle=NULL") || !strings.Contains(call.sql, "LIMIT $2") || !reflect.DeepEqual(call.args, []any{now, 100}) {
		t.Fatal("expiry deleted terminal claim UUIDs or retained recipient data", call)
	}
}

func TestPrekeyInvalidInputsAndSanitizedErrors(t *testing.T) {
	db := &queueTestDB{}
	store := &PrekeyStore{db: db}
	if _, err := store.Publish(context.Background(), "invalid", prekeyTestPublication, prekeySampleUpload(), [32]byte{}, time.Now()); !errors.Is(err, prekeys.ErrInvalid) {
		t.Fatal(err)
	}
	if _, err := store.Claim(context.Background(), "101", prekeyTestRequest, "101", time.Now()); !errors.Is(err, prekeys.ErrInvalid) {
		t.Fatal(err)
	}
	if _, err := store.Expire(context.Background(), time.Now(), 1001); !errors.Is(err, prekeys.ErrInvalid) {
		t.Fatal(err)
	}
	if db.beginCount != 0 || len(db.calls) != 0 {
		t.Fatal("invalid identifiers reached PostgreSQL")
	}
	secret := errors.New("password and public material")
	if got := prekeyError(context.Background(), secret); got != prekeys.ErrUnavailable || errors.Is(got, secret) {
		t.Fatal(got)
	}
	ctx, cancel := context.WithCancel(context.Background())
	cancel()
	if !errors.Is(prekeyError(ctx, secret), context.Canceled) {
		t.Fatal("context cancellation lost")
	}
}

func prekeySchemaColumns(columns map[string]prekeyColumn) *queueTestRows {
	rows := queueRows()
	for name, column := range columns {
		rows.values = append(rows.values, []any{name, column.typ, int32(-1), column.notNull, column.def, false, true, true, column.update})
	}
	return rows
}

func prekeySchemaConstraints(primary []string, foreign []string, checks []string) *queueTestRows {
	rows := queueRows([]any{"p", true, false, primary, false, ""})
	for _, column := range foreign {
		rows.values = append(rows.values, []any{"f", true, false, []string{column}, true, ""})
	}
	for _, expression := range checks {
		rows.values = append(rows.values, []any{"c", true, false, []string{}, false, expression})
	}
	return rows
}

func prekeySchemaStore() (*PrekeyStore, *queueTestDB) {
	db := &queueTestDB{
		rowResults: []pgx.Row{queueRow(true), queueRow(true)},
		rowsResults: []pgx.Rows{
			prekeySchemaColumns(prekeyInventoryColumns),
			prekeySchemaConstraints([]string{"void_id"}, []string{"void_id"}, prekeyInventoryChecks),
			prekeySchemaColumns(prekeyClaimColumns),
			prekeySchemaConstraints([]string{"requester_void_id", "request_id"}, []string{"requester_void_id", "recipient_void_id"}, prekeyClaimChecks),
			queueRows([]any{[]string{"claim_expires_at", "requester_void_id", "request_id"}, true, "(public_bundle IS NOT NULL)"}),
		},
	}
	return &PrekeyStore{db: db}, db
}

func TestPrekeySchemaValidationUsesOnlyRequiredColumnGrants(t *testing.T) {
	store, db := prekeySchemaStore()
	if err := store.Validate(context.Background()); err != nil {
		t.Fatal(err)
	}
	if db.beginCount != 0 || len(db.calls) != 7 {
		t.Fatal("schema validation mutated the database", db.calls)
	}
	for _, call := range db.calls {
		if strings.Contains(call.sql, "'DELETE'") || strings.Contains(call.sql, "has_table_privilege") || strings.Contains(call.sql, "public.accounts SET") {
			t.Fatal("prekey validation requires excessive runtime grants", call.sql)
		}
	}
}

func TestPrekeySchemaValidationRejectsIncompleteOrChangedSchemas(t *testing.T) {
	for _, tc := range []struct {
		name   string
		change func(*queueTestDB)
	}{
		{"missing_table", func(db *queueTestDB) { db.rowResults[0] = prekeyNoRow() }},
		{"row_security", func(db *queueTestDB) { db.rowResults[0] = queueRow(false) }},
		{"extra_column", func(db *queueTestDB) {
			rows := db.rowsResults[0].(*queueTestRows)
			rows.values = append(rows.values, []any{"private_key", "bytea", int32(-1), false, "", false, true, true, false})
		}},
		{"missing_insert_privilege", func(db *queueTestDB) { db.rowsResults[0].(*queueTestRows).values[0][7] = false }},
		{"missing_scrub_update_privilege", func(db *queueTestDB) {
			for _, row := range db.rowsResults[2].(*queueTestRows).values {
				if row[0] == "public_bundle" {
					row[8] = false
				}
			}
		}},
		{"wrong_nullability", func(db *queueTestDB) { db.rowsResults[0].(*queueTestRows).values[0][3] = false }},
		{"wrong_default", func(db *queueTestDB) { db.rowsResults[0].(*queueTestRows).values[0][4] = "unexpected()" }},
		{"missing_inventory_check", func(db *queueTestDB) {
			rows := db.rowsResults[1].(*queueTestRows)
			rows.values = rows.values[:len(rows.values)-1]
		}},
		{"unenforced_claim_check", func(db *queueTestDB) { db.rowsResults[3].(*queueTestRows).values[3][1] = false }},
		{"foreign_cascade", func(db *queueTestDB) { db.rowsResults[3].(*queueTestRows).values[1][4] = false }},
		{"wrong_expiry_index_predicate", func(db *queueTestDB) { db.rowsResults[4].(*queueTestRows).values[0][2] = "public_bundle IS NULL" }},
	} {
		t.Run(tc.name, func(t *testing.T) {
			store, db := prekeySchemaStore()
			tc.change(db)
			if !errors.Is(store.Validate(context.Background()), ErrPrekeySchema) {
				t.Fatal("unreviewed or inaccessible prekey schema accepted")
			}
		})
	}
}

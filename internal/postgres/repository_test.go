package postgres

import (
	"bytes"
	"context"
	"errors"
	"fmt"
	"strings"
	"testing"
	"time"

	"github.com/jackc/pgx/v5"
	"github.com/jackc/pgx/v5/pgconn"

	"void-nothingness/internal/account"
)

type accountInsertQuery func(context.Context, string, ...any) pgx.Row

func (q accountInsertQuery) QueryRow(ctx context.Context, sql string, args ...any) pgx.Row {
	return q(ctx, sql, args...)
}

type accountInsertRow struct {
	id      int64
	created time.Time
}

func (row accountInsertRow) Scan(dest ...any) error {
	*dest[0].(*int64) = row.id
	*dest[1].(*time.Time) = row.created
	return nil
}

func TestAccountInsertUsesOnlyIdentityAndKey(t *testing.T) {
	key := bytes.Repeat([]byte{42}, 32)
	created := time.Unix(1234, 0).UTC()
	writer := accountInsertQuery(func(_ context.Context, sql string, args ...any) pgx.Row {
		if len(args) != 2 || args[0] != int64(9223372036854775807) || !bytes.Equal(args[1].([]byte), key) {
			t.Fatal("insertion changed account identity or public key")
		}
		if strings.Contains(strings.ToLower(sql), "username") {
			t.Fatal("registration must not read or create a username")
		}
		return accountInsertRow{id: 9223372036854775807, created: created}
	})
	got, err := insertAccount(context.Background(), writer, 9223372036854775807, key)
	if err != nil || got.VoidID != "9223372036854775807" || !got.CreatedAt.Equal(created) {
		t.Fatalf("insert result: %+v %v", got, err)
	}
}

func TestAccountSchemaRequiresIdentityColumns(t *testing.T) {
	if len(expectedAccountColumns) != 4 {
		t.Fatal("account schema must contain exactly four columns")
	}
	for name, column := range expectedAccountColumns {
		if !column.notNull {
			t.Errorf("identity column permits null values: %s", name)
		}
		if !validAccountColumn(name, column.typ, column.def, column.modifier, column.notNull, false) {
			t.Errorf("approved column rejected: %s", name)
		}
		if validAccountColumn(name, column.typ, column.def, column.modifier, !column.notNull, false) {
			t.Errorf("changed nullability accepted: %s", name)
		}
		if validAccountColumn(name, column.typ, column.def, column.modifier, column.notNull, true) {
			t.Errorf("generated identity data accepted: %s", name)
		}
	}
	if validAccountColumn("username", "varchar", "", 132, false, false) {
		t.Fatal("legacy username column accepted")
	}
	if validAccountColumn("unexpected", "text", "", -1, false, false) {
		t.Fatal("unknown account column accepted")
	}
}

func TestAccountSchemaRejectsChangedTypesAndDefaults(t *testing.T) {
	for name, column := range expectedAccountColumns {
		if validAccountColumn(name, "text", column.def, column.modifier, column.notNull, false) {
			t.Errorf("changed type accepted: %s", name)
		}
		if validAccountColumn(name, column.typ, "unexpected()", column.modifier, column.notNull, false) {
			t.Errorf("changed default accepted: %s", name)
		}
		if validAccountColumn(name, column.typ, column.def, column.modifier+1, column.notNull, false) {
			t.Errorf("changed type modifier accepted: %s", name)
		}
	}
}

func TestRequiredColumnAccessAllowsConfiguredLoginPrivileges(t *testing.T) {
	for _, column := range []string{"id", "void_id", "auth_public_key", "created_at"} {
		t.Run(column, func(t *testing.T) {
			// A table owner or superuser has both permissions on every column.
			// These extra privileges must not reject the configured login.
			if !hasRequiredColumnAccess(column, true, true) {
				t.Fatal("rejected existing login with full column privileges")
			}
		})
	}
}

func TestRequiredColumnAccessMatchesRegistrationAndAuthentication(t *testing.T) {
	tests := []struct {
		column                 string
		selectable, insertable bool
		want                   bool
	}{
		{"id", false, false, true},
		{"void_id", false, false, false},
		{"void_id", false, true, false},
		{"void_id", true, false, false},
		{"auth_public_key", false, true, false},
		{"auth_public_key", false, false, false},
		{"auth_public_key", true, false, false},
		{"auth_public_key", true, true, true},
		{"created_at", true, false, true},
		{"created_at", false, false, false},
		{"created_at", false, true, false},
	}
	for _, test := range tests {
		t.Run(fmt.Sprintf("%s/select=%t/insert=%t", test.column, test.selectable, test.insertable), func(t *testing.T) {
			got := hasRequiredColumnAccess(test.column, test.selectable, test.insertable)
			if got != test.want {
				t.Fatalf("got %t, want %t", got, test.want)
			}
		})
	}
}

func TestDatabaseErrorsAreClassifiedWithoutLeakingDiagnostics(t *testing.T) {
	tests := []struct {
		name, code, schema, table, constraint string
		want                                  error
	}{
		{"VOID-ID collision", "23505", "public", "accounts", "void_unique", account.ErrIDCollision},
		{"duplicate public key", "23505", "public", "accounts", "key_unique", account.ErrDuplicateKey},
		{"unknown constraint", "23505", "public", "accounts", "other", account.ErrUnavailable},
		{"different table", "23505", "public", "other", "key_unique", account.ErrUnavailable},
		{"different schema", "23505", "private", "accounts", "key_unique", account.ErrUnavailable},
		{"constraint failure", "23514", "public", "accounts", "key_unique", account.ErrUnavailable},
	}
	for _, test := range tests {
		t.Run(test.name, func(t *testing.T) {
			original := &pgconn.PgError{
				Code: test.code, SchemaName: test.schema, TableName: test.table, ConstraintName: test.constraint,
				Message: "secret SQL or credentials", Detail: "secret authentication key",
			}
			got := classifyError(context.Background(), original, "void_unique", "key_unique")
			if !errors.Is(got, test.want) || errors.Is(got, original) || strings.Contains(got.Error(), "secret") {
				t.Fatalf("classification exposed diagnostics or returned incorrect error: %v", got)
			}
		})
	}
	if got := classifyError(context.Background(), errors.New("secret connection credentials"), "id", "key"); got != account.ErrUnavailable {
		t.Fatalf("unexpected connection error: %v", got)
	}
}

func TestCanceledDatabaseOperationPreservesContextError(t *testing.T) {
	ctx, cancel := context.WithCancel(context.Background())
	cancel()
	if got := classifyError(ctx, errors.New("secret connection details"), "id", "key"); !errors.Is(got, context.Canceled) {
		t.Fatalf("got %v, want context cancellation", got)
	}
}

func TestValidationDiagnosticsAreActionableAndNeverRetainDatabaseErrors(t *testing.T) {
	tests := []struct {
		name, code string
		want       error
	}{
		{"missing database", "3D000", ErrDatabaseNotFound},
		{"invalid password", "28P01", ErrAuthenticationFailed},
		{"invalid authorization", "28000", ErrAuthenticationFailed},
		{"insufficient privileges", "42501", ErrRuntimeRole},
		{"unknown database failure", "XX000", account.ErrUnavailable},
	}
	for _, test := range tests {
		t.Run(test.name, func(t *testing.T) {
			original := &pgconn.PgError{
				Code:    test.code,
				Message: "secret password=do-not-log", Detail: "secret SQL SELECT auth_public_key FROM accounts",
				Hint: "secret credential", Where: "secret connection string", InternalQuery: "secret SQL",
				SchemaName: "secret schema", TableName: "secret table", ConstraintName: "secret constraint",
			}
			wrapped := fmt.Errorf("secret connection credentials: %w", original)
			got := classifyValidationError(context.Background(), wrapped)
			if !errors.Is(got, test.want) {
				t.Fatalf("got %v, want %v", got, test.want)
			}
			var exposed *pgconn.PgError
			if strings.Contains(got.Error(), "secret") || errors.As(got, &exposed) || errors.Is(got, wrapped) || errors.Unwrap(got) != nil {
				t.Fatalf("validation exposed database diagnostics: %v", got)
			}
		})
	}
	if got := classifyValidationError(context.Background(), errors.New("secret connection credentials")); got != account.ErrUnavailable {
		t.Fatalf("unexpected connection error: %v", got)
	}
}

func TestValidationPreservesOnlySafeContextErrors(t *testing.T) {
	for _, contextError := range []error{context.Canceled, context.DeadlineExceeded} {
		t.Run(contextError.Error(), func(t *testing.T) {
			wrapped := fmt.Errorf("secret connection credentials: %w", contextError)
			if got := classifyValidationError(context.Background(), wrapped); got != contextError {
				t.Fatalf("got %v, want safe context sentinel", got)
			}
		})
	}
	ctx, cancel := context.WithCancel(context.Background())
	cancel()
	if got := classifyValidationError(ctx, &pgconn.PgError{Code: "3D000", Message: "secret database"}); got != context.Canceled {
		t.Fatalf("canceled context did not take precedence: %v", got)
	}
}

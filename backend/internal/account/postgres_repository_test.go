package account

import (
	"context"
	"errors"
	"fmt"
	"strings"
	"testing"

	"github.com/jackc/pgx/v5/pgconn"
)

func TestPostgresDuplicateErrorRequiresAccountConstraint(t *testing.T) {
	tests := []struct {
		name       string
		code       string
		constraint string
		want       error
	}{
		{"account duplicate", "23505", "accounts_void_id_key", ErrAlreadyExists},
		{"different unique constraint", "23505", "other_unique_key", errPostgresOperation},
		{"different error code", "23514", "accounts_void_id_key", errPostgresOperation},
	}
	for _, tt := range tests {
		t.Run(tt.name, func(t *testing.T) {
			pgError := &pgconn.PgError{Code: tt.code, ConstraintName: tt.constraint, Detail: "private-fixture-value"}
			got := postgresCreateError(fmt.Errorf("driver: %w", pgError))
			if got != tt.want {
				t.Fatalf("postgresCreateError() = %v, want %v", got, tt.want)
			}
			if strings.Contains(got.Error(), pgError.Detail) || errors.Is(got, pgError) {
				t.Fatal("repository error must not expose or wrap raw PostgreSQL details")
			}
		})
	}
}

func TestPostgresErrorsPreserveCancellationAndHideDriverDetails(t *testing.T) {
	for _, want := range []error{context.Canceled, context.DeadlineExceeded} {
		if got := postgresOperationError(fmt.Errorf("driver: %w", want)); got != want {
			t.Fatalf("postgresOperationError() = %v, want %v", got, want)
		}
	}
	raw := errors.New("connection failed with password=private-fixture-value")
	if got := postgresOperationError(raw); got != errPostgresOperation || strings.Contains(got.Error(), "private-fixture-value") {
		t.Fatalf("driver failure was not sanitized: %v", got)
	}
}

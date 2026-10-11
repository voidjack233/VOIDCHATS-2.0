package database

import (
	"context"
	"errors"
	"fmt"
	"testing"
	"testing/fstest"

	"github.com/voidjack233/VOIDCHATS-2.0/backend/migrations"
)

func TestLoadMigrationsOrdersVersionsAndChecksumsContent(t *testing.T) {
	files := fstest.MapFS{
		"10_later.up.sql":  {Data: []byte("SELECT 10;")},
		"2_earlier.up.sql": {Data: []byte("abc")},
		"README.md":        {Data: []byte("ignored")},
		"1_old.down.sql":   {Data: []byte("ignored destructive SQL")},
	}
	loaded, err := loadMigrations(files)
	if err != nil || len(loaded) != 2 {
		t.Fatalf("loadMigrations(): length %d, error %v; want 2, nil", len(loaded), err)
	}
	if loaded[0].version != 2 || loaded[1].version != 10 {
		t.Fatal("migrations must be ordered by numeric version")
	}
	if loaded[0].checksum != "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad" {
		t.Fatal("checksum must be the SHA256 of the exact embedded file bytes")
	}
	if loaded[0].name != "2_earlier.up.sql" || loaded[0].sql != "abc" {
		t.Fatal("loader must preserve the migration name and content")
	}
}

func TestLoadMigrationsRejectsInvalidVersionsAndEmptySQL(t *testing.T) {
	tests := map[string]fstest.MapFS{
		"empty":               {},
		"zero version":        {"0000_zero.up.sql": {Data: []byte("SELECT 1;")}},
		"non-numeric version": {"bad_name.up.sql": {Data: []byte("SELECT 1;")}},
		"no name separator":   {"0001.up.sql": {Data: []byte("SELECT 1;")}},
		"duplicate numeric version": {
			"1_first.up.sql":   {Data: []byte("SELECT 1;")},
			"01_second.up.sql": {Data: []byte("SELECT 2;")},
		},
		"empty SQL": {"0001_empty.up.sql": {Data: []byte(" \n\t")}},
	}
	for name, files := range tests {
		t.Run(name, func(t *testing.T) {
			if _, err := loadMigrations(files); err == nil {
				t.Fatal("invalid migration set was accepted")
			}
		})
	}
	if _, err := loadMigrations(migrations.Files); err != nil {
		t.Fatalf("shipped migrations must load: %v", err)
	}
}

func TestMigrationHistoryRequiresUnchangedContiguousPrefix(t *testing.T) {
	first := migration{version: 1, name: "0001_first.up.sql", checksum: "first"}
	second := migration{version: 2, name: "0002_second.up.sql", checksum: "second"}
	changedName := first
	changedName.name = "0001_renamed.up.sql"
	changedChecksum := first
	changedChecksum.checksum = "changed"
	tests := []struct {
		name    string
		applied map[int64]migration
		want    error
	}{
		{"fresh database", map[int64]migration{}, nil},
		{"prefix", map[int64]migration{1: first}, nil},
		{"complete", map[int64]migration{1: first, 2: second}, nil},
		{"missing predecessor", map[int64]migration{2: second}, ErrMigrationHistory},
		{"renamed file", map[int64]migration{1: changedName}, ErrMigrationHistory},
		{"edited file", map[int64]migration{1: changedChecksum}, ErrMigrationHistory},
		{"unknown database version", map[int64]migration{99: {version: 99}}, ErrMigrationHistory},
	}
	for _, tt := range tests {
		t.Run(tt.name, func(t *testing.T) {
			if got := validateHistory([]migration{first, second}, tt.applied); got != tt.want {
				t.Fatalf("validateHistory() = %v, want %v", got, tt.want)
			}
		})
	}
}

func TestMigrationErrorsHideDriverDetails(t *testing.T) {
	for _, want := range []error{context.Canceled, context.DeadlineExceeded} {
		if got := migrationError(fmt.Errorf("driver: %w", want)); got != want {
			t.Fatalf("migrationError() = %v, want %v", got, want)
		}
	}
	if got := migrationError(errors.New("password=private-fixture-value")); got != ErrMigrationFailed {
		t.Fatal("migration failure must expose only a safe generic error")
	}
}

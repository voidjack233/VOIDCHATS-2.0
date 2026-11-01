package config

import (
	"net/url"
	"os"
	"path/filepath"
	"strings"
	"testing"
)

func TestLiteralEnvAndOverride(t *testing.T) {
	path := filepath.Join(t.TempDir(), "config.env")
	if err := os.WriteFile(path, []byte("PGUSER=app\nPGDATABASE=void\nPGPASSWORD='literal$(do-not-run)`command`'\nVOID_RATE_BURST=4\n"), 0600); err != nil {
		t.Fatal(err)
	}
	t.Setenv("VOID_RATE_BURST", "8")
	c, err := Load(path)
	if err != nil {
		t.Fatal(err)
	}
	if c.RateBurst != 8 || c.ListenAddr != "127.0.0.1:8080" {
		t.Fatal("configuration defaults or environment precedence mismatch")
	}
	if !strings.Contains(c.DatabaseURL, "app:") {
		t.Fatal("expected URL user")
	}
	values, err := ReadEnvFile(path)
	if err != nil || values["PGPASSWORD"] != "literal$(do-not-run)`command`" {
		t.Fatalf("literal env parsing: %v", err)
	}
}

func TestConnectionValuesAreLiteral(t *testing.T) {
	// Names are operator-owned: a suspicious-looking name must be preserved,
	// never corrected or redirected by the backend. Reserved password characters
	// must survive URL encoding without becoming connection options.
	t.Setenv("DATABASE_URL", "")
	t.Setenv("PGHOST", "localhost")
	t.Setenv("PGPORT", "5432")
	t.Setenv("PGDATABASE", "void2.0e2ee")
	t.Setenv("PGUSER", "literal-user")
	const password = "literal@:/?#%&=value$(command)`text`"
	t.Setenv("PGPASSWORD", password)
	c, err := Load("")
	if err != nil {
		t.Fatal(err)
	}
	u, err := url.Parse(c.DatabaseURL)
	if err != nil {
		t.Fatal("invalid connection URL")
	}
	storedPassword, ok := u.User.Password()
	if u.Path != "/void2.0e2ee" || u.User.Username() != "literal-user" || !ok || storedPassword != password || len(u.Query()) != 1 {
		t.Fatal("connection fields were modified or credentials became URL options")
	}
}

func TestDatabaseURLTakesPrecedence(t *testing.T) {
	const dsn = "postgresql://example-user:synthetic-password@localhost:5432/owner-chosen-database?sslmode=require"
	t.Setenv("DATABASE_URL", dsn)
	t.Setenv("PGDATABASE", "different-name")
	t.Setenv("PGUSER", "different-user")
	t.Setenv("PGPORT", "not-a-port")
	c, err := Load("")
	if err != nil {
		t.Fatal(err)
	}
	if c.DatabaseURL != dsn {
		t.Fatal("DATABASE_URL did not retain precedence over individual PG settings")
	}
}

func TestInvalidConfigDoesNotLeakValues(t *testing.T) {
	t.Setenv("PGUSER", "app")
	t.Setenv("PGDATABASE", "void")
	for _, key := range []string{"VOID_WORKER_ID", "VOID_CHALLENGE_TTL", "VOID_MAX_CHALLENGES", "VOID_HTTP_ADDR", "VOID_DATABASE_MAX_CONNS", "VOID_SNOWFLAKE_EPOCH"} {
		t.Run(key, func(t *testing.T) {
			t.Setenv(key, "secret-invalid-value")
			_, err := Load("")
			if err == nil || strings.Contains(err.Error(), "secret-invalid-value") {
				t.Fatalf("unsafe configuration error: %v", err)
			}
		})
	}
}

func TestDuplicateAndUnterminatedAssignments(t *testing.T) {
	for _, input := range []string{"PGUSER=a\nPGUSER=b\n", "PGPASSWORD='unclosed", "not-an-assignment"} {
		path := filepath.Join(t.TempDir(), "bad.env")
		if err := os.WriteFile(path, []byte(input), 0600); err != nil {
			t.Fatal(err)
		}
		if _, err := ReadEnvFile(path); err == nil {
			t.Fatal("accepted invalid file")
		}
	}
}

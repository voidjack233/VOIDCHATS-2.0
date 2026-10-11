package config

import "testing"

func TestLoad(t *testing.T) {
	for _, test := range []struct {
		name string
		addr string
		want string
	}{
		{name: "default", want: "127.0.0.1:8080"},
		{name: "configured", addr: "127.0.0.1:9090", want: "127.0.0.1:9090"},
		{name: "all interfaces", addr: ":8080", want: ":8080"},
		{name: "IPv6", addr: "[::1]:8080", want: "[::1]:8080"},
	} {
		t.Run(test.name, func(t *testing.T) {
			developmentEnv(t)
			t.Setenv("VOID_HTTP_ADDR", test.addr)
			cfg, err := Load()
			if err != nil {
				t.Fatal(err)
			}
			if cfg.HTTPAddr != test.want {
				t.Fatalf("HTTPAddr = %q, want %q", cfg.HTTPAddr, test.want)
			}
		})
	}
}

func TestLoadRejectsInvalidAddress(t *testing.T) {
	for _, addr := range []string{"127.0.0.1", "127.0.0.1:http", "127.0.0.1:0", "127.0.0.1:65536", "::1:8080"} {
		t.Run(addr, func(t *testing.T) {
			developmentEnv(t)
			t.Setenv("VOID_HTTP_ADDR", addr)
			if _, err := Load(); err == nil {
				t.Fatalf("Load accepted invalid address %q", addr)
			}
		})
	}
}

func TestDatabaseDefaultsAndCredentialRequirement(t *testing.T) {
	developmentEnv(t)
	cfg, err := Load()
	if err != nil {
		t.Fatal(err)
	}
	if cfg.Database.Host != "127.0.0.1" || cfg.Database.Port != 5432 || cfg.Database.User != "void_dev" || cfg.Database.Name != "void_dev" || cfg.Database.SSLMode != "disable" || cfg.Environment != "development" {
		t.Fatal("unexpected local development defaults")
	}
	t.Setenv("PGPASSWORD", "")
	if _, err := Load(); err == nil {
		t.Fatal("missing database credentials were accepted")
	}
}

func TestHTTPHostAndPortAndAddressPrecedence(t *testing.T) {
	developmentEnv(t)
	t.Setenv("VOID_HTTP_HOST", "0.0.0.0")
	t.Setenv("VOID_HTTP_PORT", "9000")
	cfg, err := Load()
	if err != nil || cfg.HTTPAddr != "0.0.0.0:9000" {
		t.Fatalf("host/port configuration failed: address=%s, error=%v", cfg.HTTPAddr, err)
	}
	t.Setenv("VOID_HTTP_ADDR", "127.0.0.1:9001")
	cfg, err = Load()
	if err != nil || cfg.HTTPAddr != "127.0.0.1:9001" {
		t.Fatalf("address precedence failed: address=%s, error=%v", cfg.HTTPAddr, err)
	}
}

func TestRemoteDatabaseRequiresVerifiedTLS(t *testing.T) {
	developmentEnv(t)
	t.Setenv("PGHOST", "database.example.test")
	cfg, err := Load()
	if err != nil || cfg.Database.SSLMode != "verify-full" {
		t.Fatal("remote database did not default to certificate verification")
	}
	t.Setenv("PGSSLMODE", "disable")
	if _, err := Load(); err == nil {
		t.Fatal("unencrypted remote database configuration accepted")
	}
}

func TestInvalidDatabaseConfiguration(t *testing.T) {
	for _, test := range []struct{ name, value string }{
		{"PGPORT", "65536"},
		{"PGPORT", "0"},
		{"PGPORT", "postgres"},
		{"PGHOST", "host1,host2"},
		{"PGUSER", " "},
		{"PGDATABASE", "\t"},
		{"PGSSLMODE", "prefer"},
		{"VOID_ENV", "unexpected"},
	} {
		t.Run(test.name+"/"+test.value, func(t *testing.T) {
			developmentEnv(t)
			t.Setenv(test.name, test.value)
			if _, err := Load(); err == nil {
				t.Fatalf("invalid %s configuration accepted", test.name)
			}
		})
	}
}

func TestDiagnosticAddressRemainsLoopback(t *testing.T) {
	for _, test := range []struct {
		addr  string
		valid bool
	}{
		{addr: "", valid: true},
		{addr: "127.0.0.1:8091", valid: true},
		{addr: "[::1]:8091", valid: true},
		{addr: "0.0.0.0:8091"},
		{addr: ":8091"},
		{addr: "192.0.2.1:8091"},
		{addr: "example.test:8091"},
		{addr: "127.0.0.1:0"},
	} {
		t.Run(test.addr, func(t *testing.T) {
			t.Setenv("VOID_DIAGNOSTIC_ADDR", test.addr)
			addr, err := LoadDiagnosticAddr()
			if (err == nil) != test.valid {
				t.Fatalf("valid=%v, error=%v", test.valid, err)
			}
			if test.addr == "" && addr != "127.0.0.1:8091" {
				t.Fatalf("diagnostic default = %s", addr)
			}
		})
	}
}

func developmentEnv(t *testing.T) {
	t.Helper()
	for _, name := range []string{"VOID_HTTP_ADDR", "VOID_HTTP_HOST", "VOID_HTTP_PORT", "VOID_ENV", "PGHOST", "PGPORT", "PGUSER", "PGDATABASE", "PGSSLMODE", "PGSSLROOTCERT"} {
		t.Setenv(name, "")
	}
	t.Setenv("PGPASSWORD", "unit-test-only-password")
}

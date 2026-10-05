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
			t.Setenv("VOID_HTTP_ADDR", addr)
			if _, err := Load(); err == nil {
				t.Fatalf("Load accepted invalid address %q", addr)
			}
		})
	}
}

// Package config reads the API process's minimal runtime configuration.
package config

import (
	"fmt"
	"net"
	"os"
	"strconv"
	"strings"
)

type Config struct {
	HTTPAddr    string
	Environment string
	Database    Database
}

type Database struct {
	Host        string
	Port        uint16
	User        string
	Password    string
	Name        string
	SSLMode     string
	SSLRootCert string
}

// Load reads explicit development settings without loading files or logging secrets.
func Load() (Config, error) {
	addr := os.Getenv("VOID_HTTP_ADDR")
	if addr == "" {
		addr = net.JoinHostPort(envOr("VOID_HTTP_HOST", "127.0.0.1"), envOr("VOID_HTTP_PORT", "8080"))
	}
	if err := validateAddr(addr, "VOID_HTTP_ADDR"); err != nil {
		return Config{}, err
	}
	environment := envOr("VOID_ENV", "development")
	if environment != "development" && environment != "test" && environment != "production" {
		return Config{}, fmt.Errorf("VOID_ENV must be development, test, or production")
	}

	port, err := numericPort(envOr("PGPORT", "5432"))
	if err != nil {
		return Config{}, fmt.Errorf("PGPORT must be a numeric port between 1 and 65535")
	}
	db := Database{
		Host:        envOr("PGHOST", "127.0.0.1"),
		Port:        port,
		User:        envOr("PGUSER", "void_dev"),
		Password:    os.Getenv("PGPASSWORD"),
		Name:        envOr("PGDATABASE", "void_dev"),
		SSLRootCert: os.Getenv("PGSSLROOTCERT"),
	}
	if strings.ContainsAny(db.Host, " /\\,\t\r\n") {
		return Config{}, fmt.Errorf("PGHOST must identify one database host")
	}
	if strings.TrimSpace(db.User) == "" || strings.TrimSpace(db.Name) == "" {
		return Config{}, fmt.Errorf("PGUSER and PGDATABASE must be nonblank")
	}
	if db.Password == "" {
		return Config{}, fmt.Errorf("PGPASSWORD is required; load the local development credentials")
	}
	db.SSLMode = os.Getenv("PGSSLMODE")
	if db.SSLMode == "" {
		db.SSLMode = "verify-full"
		if loopbackHost(db.Host) {
			db.SSLMode = "disable"
		}
	}
	if db.SSLMode != "disable" && db.SSLMode != "verify-full" {
		return Config{}, fmt.Errorf("PGSSLMODE must be disable or verify-full")
	}
	if db.SSLMode == "disable" && !loopbackHost(db.Host) {
		return Config{}, fmt.Errorf("PGSSLMODE=disable is allowed only for a loopback database host")
	}
	return Config{HTTPAddr: addr, Environment: environment, Database: db}, nil
}

// LoadDiagnosticAddr configures the independent health-only tunnel origin.
func LoadDiagnosticAddr() (string, error) {
	addr := envOr("VOID_DIAGNOSTIC_ADDR", "127.0.0.1:8091")
	if err := validateAddr(addr, "VOID_DIAGNOSTIC_ADDR"); err != nil {
		return "", err
	}
	host, _, _ := net.SplitHostPort(addr)
	ip := net.ParseIP(host)
	if ip == nil || !ip.IsLoopback() {
		return "", fmt.Errorf("VOID_DIAGNOSTIC_ADDR must bind a numeric loopback IP address")
	}
	return addr, nil
}

func envOr(name, fallback string) string {
	if value := os.Getenv(name); value != "" {
		return value
	}
	return fallback
}

func validateAddr(addr, name string) error {
	_, port, err := net.SplitHostPort(addr)
	if err != nil {
		return fmt.Errorf("%s must be a host:port address", name)
	}
	if _, err := numericPort(port); err != nil {
		return fmt.Errorf("%s must use a numeric port between 1 and 65535", name)
	}
	return nil
}

func numericPort(value string) (uint16, error) {
	port, err := strconv.ParseUint(value, 10, 16)
	if err != nil || port == 0 {
		return 0, fmt.Errorf("invalid port")
	}
	return uint16(port), nil
}

func loopbackHost(host string) bool {
	if strings.EqualFold(host, "localhost") {
		return true
	}
	ip := net.ParseIP(host)
	return ip != nil && ip.IsLoopback()
}

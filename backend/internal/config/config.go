// Package config reads the API process's minimal runtime configuration.
package config

import (
	"fmt"
	"net"
	"os"
	"strconv"
)

const defaultHTTPAddr = "127.0.0.1:8080"

type Config struct {
	HTTPAddr string
}

// Load reads VOID_HTTP_ADDR, falling back to a loopback development listener.
func Load() (Config, error) {
	addr := os.Getenv("VOID_HTTP_ADDR")
	if addr == "" {
		addr = defaultHTTPAddr
	}

	_, port, err := net.SplitHostPort(addr)
	if err != nil {
		return Config{}, fmt.Errorf("VOID_HTTP_ADDR must be a host:port address: %w", err)
	}
	portNumber, err := strconv.Atoi(port)
	if err != nil || portNumber < 1 || portNumber > 65535 {
		return Config{}, fmt.Errorf("VOID_HTTP_ADDR must use a numeric port between 1 and 65535")
	}

	return Config{HTTPAddr: addr}, nil
}

package config

import (
	"bufio"
	"errors"
	"fmt"
	"net"
	"net/url"
	"os"
	"strconv"
	"strings"
	"time"
)

type Config struct {
	DatabaseURL           string
	ListenAddr            string
	WorkerID              uint16
	Epoch                 time.Time
	ChallengeTTL          time.Duration
	MaxChallenges         int
	RequestTimeout        time.Duration
	ShutdownTimeout       time.Duration
	RatePerMinute         int
	RateBurst             int
	MaxRateClients        int
	MaxConcurrentRequests int
	DatabaseMaxConns      int32
}

// ReadEnvFile parses literal KEY=VALUE assignments, never shell code. Existing
// process environment takes precedence, including an explicitly empty value.
func ReadEnvFile(path string) (map[string]string, error) {
	values := make(map[string]string)
	if path == "" {
		return values, nil
	}
	f, err := os.Open(path)
	if errors.Is(err, os.ErrNotExist) {
		return values, nil
	}
	if err != nil {
		return nil, errors.New("cannot open environment file")
	}
	defer f.Close()
	scanner := bufio.NewScanner(f)
	for scanner.Scan() {
		line := strings.TrimSpace(scanner.Text())
		if line == "" || strings.HasPrefix(line, "#") {
			continue
		}
		line = strings.TrimPrefix(line, "export ")
		key, value, ok := strings.Cut(line, "=")
		key, value = strings.TrimSpace(key), strings.TrimSpace(value)
		if !ok || key == "" || strings.ContainsAny(key, " \t\r\n\x00") {
			return nil, errors.New("invalid environment file assignment")
		}
		if len(value) > 0 && (value[0] == '\'' || value[0] == '"') {
			if len(value) < 2 || value[len(value)-1] != value[0] {
				return nil, errors.New("invalid environment file quoting")
			}
			value = value[1 : len(value)-1]
		}
		if _, exists := values[key]; exists {
			return nil, errors.New("duplicate environment file key")
		}
		values[key] = value
	}
	if scanner.Err() != nil {
		return nil, errors.New("cannot read environment file")
	}
	return values, nil
}

func Load(path string) (Config, error) {
	file, err := ReadEnvFile(path)
	if err != nil {
		return Config{}, err
	}
	get := func(key, fallback string) string {
		if value, ok := os.LookupEnv(key); ok {
			return value
		}
		if value, ok := file[key]; ok {
			return value
		}
		return fallback
	}
	c := Config{ListenAddr: get("VOID_HTTP_ADDR", "127.0.0.1:8080")}
	if _, _, err := net.SplitHostPort(c.ListenAddr); err != nil {
		return Config{}, errors.New("invalid VOID_HTTP_ADDR")
	}
	c.DatabaseURL = get("DATABASE_URL", "")
	if c.DatabaseURL == "" {
		user, database := get("PGUSER", ""), get("PGDATABASE", "")
		if user == "" || database == "" {
			return Config{}, errors.New("set DATABASE_URL or PGUSER and PGDATABASE")
		}
		port, err := strconv.Atoi(get("PGPORT", "5432"))
		if err != nil || port < 1 || port > 65535 {
			return Config{}, errors.New("invalid PGPORT")
		}
		u := url.URL{Scheme: "postgres", Host: net.JoinHostPort(get("PGHOST", "localhost"), strconv.Itoa(port)), Path: "/" + database, User: url.UserPassword(user, get("PGPASSWORD", ""))}
		q := u.Query()
		q.Set("sslmode", get("PGSSLMODE", "prefer"))
		u.RawQuery = q.Encode()
		c.DatabaseURL = u.String()
	}
	epoch, err := time.Parse(time.RFC3339Nano, get("VOID_SNOWFLAKE_EPOCH", "2026-01-01T00:00:00Z"))
	if err != nil || epoch.UnixMilli() < 0 || epoch.After(time.Now()) {
		return Config{}, errors.New("invalid or future VOID_SNOWFLAKE_EPOCH")
	}
	c.Epoch = epoch.UTC()
	integer := func(key string, fallback, min, max int) (int, error) {
		n, err := strconv.Atoi(get(key, strconv.Itoa(fallback)))
		if err != nil || n < min || n > max {
			return 0, fmt.Errorf("invalid %s", key)
		}
		return n, nil
	}
	worker, err := integer("VOID_WORKER_ID", 0, 0, 1023)
	if err != nil {
		return Config{}, err
	}
	c.WorkerID = uint16(worker)
	fields := []struct {
		key                string
		fallback, min, max int
		target             *int
	}{
		{"VOID_MAX_CHALLENGES", 4096, 1, 100000, &c.MaxChallenges},
		{"VOID_RATE_PER_MINUTE", 30, 1, 10000, &c.RatePerMinute},
		{"VOID_RATE_BURST", 10, 1, 1000, &c.RateBurst},
		{"VOID_MAX_RATE_CLIENTS", 4096, 1, 100000, &c.MaxRateClients},
		{"VOID_MAX_CONCURRENT_REQUESTS", 64, 1, 1024, &c.MaxConcurrentRequests},
	}
	for _, field := range fields {
		n, err := integer(field.key, field.fallback, field.min, field.max)
		if err != nil {
			return Config{}, err
		}
		*field.target = n
	}
	connections, err := integer("VOID_DATABASE_MAX_CONNS", 8, 1, 64)
	if err != nil {
		return Config{}, err
	}
	c.DatabaseMaxConns = int32(connections)
	durations := []struct {
		key, fallback string
		min, max      time.Duration
		target        *time.Duration
	}{
		{"VOID_CHALLENGE_TTL", "2m", time.Second, 10 * time.Minute, &c.ChallengeTTL},
		{"VOID_REQUEST_TIMEOUT", "5s", 100 * time.Millisecond, 30 * time.Second, &c.RequestTimeout},
		{"VOID_SHUTDOWN_TIMEOUT", "10s", time.Second, time.Minute, &c.ShutdownTimeout},
	}
	for _, field := range durations {
		d, err := time.ParseDuration(get(field.key, field.fallback))
		if err != nil || d < field.min || d > field.max {
			return Config{}, fmt.Errorf("invalid %s", field.key)
		}
		*field.target = d
	}
	return c, nil
}

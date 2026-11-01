package messagingapi

import (
	"math"
	"net"
	"net/netip"
	"sync"
	"time"
)

type bucket struct {
	tokens        float64
	updated, seen time.Time
}
type limiter struct {
	mu               sync.Mutex
	entries          map[string]bucket
	perSecond, burst float64
	capacity         int
	now              func() time.Time
}

func newLimiter(rate, burst, capacity int) *limiter {
	return &limiter{entries: make(map[string]bucket), perSecond: float64(rate) / 60, burst: float64(burst), capacity: capacity, now: time.Now}
}

func (l *limiter) allow(key string) (bool, int) {
	l.mu.Lock()
	defer l.mu.Unlock()
	now := l.now()
	b, found := l.entries[key]
	if !found {
		if len(l.entries) >= l.capacity {
			for k, entry := range l.entries {
				if now.Sub(entry.seen) >= 10*time.Minute {
					delete(l.entries, k)
				}
			}
			if len(l.entries) >= l.capacity {
				return false, 60
			}
		}
		b = bucket{tokens: l.burst, updated: now}
	}
	b.tokens = min(l.burst, b.tokens+max(0, now.Sub(b.updated).Seconds())*l.perSecond)
	b.updated, b.seen = now, now
	allowed := b.tokens+1e-9 >= 1
	retry := 0
	if allowed {
		b.tokens = max(0, b.tokens-1)
	} else {
		retry = max(1, int(math.Ceil((1-b.tokens)/l.perSecond-1e-9)))
	}
	l.entries[key] = b
	return allowed, retry
}

func remoteIP(addr string) string {
	host, _, err := net.SplitHostPort(addr)
	if err != nil {
		host = addr
	}
	if parsed, err := netip.ParseAddr(host); err == nil {
		return parsed.Unmap().String()
	}
	return "unknown"
}

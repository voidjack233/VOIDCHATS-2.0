package httpapi

import (
	"math"
	"net"
	"net/netip"
	"sync"
	"time"
)

type bucket struct {
	tokens   float64
	updated  time.Time
	lastSeen time.Time
}

type ipLimiter struct {
	mu          sync.Mutex
	clients     map[string]bucket
	rate        float64
	burst       float64
	maxClients  int
	idleTTL     time.Duration
	nextCleanup time.Time
	now         func() time.Time
}

func newIPLimiter(perMinute, burst, maxClients int) *ipLimiter {
	idleTTL := max(10*time.Minute, time.Duration(float64(burst)/float64(perMinute)*float64(time.Minute)))
	return &ipLimiter{
		clients:    make(map[string]bucket),
		rate:       float64(perMinute) / 60,
		burst:      float64(burst),
		maxClients: maxClients,
		idleTTL:    idleTTL,
		now:        time.Now,
	}
}

func (l *ipLimiter) allow(remoteAddr string) (bool, int) {
	ip := clientIP(remoteAddr)
	l.mu.Lock()
	defer l.mu.Unlock()
	// Sample time in the same order as bucket updates, so concurrent requests
	// cannot move updated backwards and credit a refill interval twice.
	now := l.now()
	if !now.Before(l.nextCleanup) {
		for key, entry := range l.clients {
			if now.Sub(entry.lastSeen) >= l.idleTTL {
				delete(l.clients, key)
			}
		}
		l.nextCleanup = now.Add(time.Minute)
	}
	entry, found := l.clients[ip]
	if !found {
		if len(l.clients) >= l.maxClients {
			return false, 60
		}
		entry = bucket{tokens: l.burst, updated: now}
	}
	entry.tokens = min(l.burst, entry.tokens+max(0, now.Sub(entry.updated).Seconds())*l.rate)
	entry.updated = now
	entry.lastSeen = now
	// Repeated fractional refills can leave a full token a few float ULPs short.
	if entry.tokens+1e-9 < 1 {
		l.clients[ip] = entry
		return false, max(1, int(math.Ceil((1-entry.tokens)/l.rate-1e-9)))
	}
	entry.tokens = max(0, entry.tokens-1)
	l.clients[ip] = entry
	return true, 0
}

func clientIP(remoteAddr string) string {
	host, _, err := net.SplitHostPort(remoteAddr)
	if err != nil {
		host = remoteAddr
	}
	if addr, err := netip.ParseAddr(host); err == nil {
		return addr.Unmap().String()
	}
	return "unknown"
}

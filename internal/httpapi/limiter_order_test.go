package httpapi

import (
	"sync/atomic"
	"testing"
	"time"
)

func TestIPLimiterConcurrentClockSamplesCannotCreditSameIntervalTwice(t *testing.T) {
	const remoteAddr = "192.0.2.1:1000"
	start := time.Unix(1000, 0)
	end := start.Add(time.Second)
	l := newIPLimiter(60, 1, 10)
	l.now = func() time.Time { return start }
	if allowed, _ := l.allow(remoteAddr); !allowed {
		t.Fatal("initial token unavailable")
	}

	firstSample := make(chan bool, 1)
	releaseFirst := make(chan struct{})
	var samples atomic.Int64
	l.now = func() time.Time {
		if samples.Add(1) == 1 {
			// If sampling is outside the lock, force the newer sample to
			// update first. Otherwise release this sample before the next
			// request can acquire the lock. Neither path relies on timing.
			locked := !l.mu.TryLock()
			if !locked {
				l.mu.Unlock()
			}
			firstSample <- locked
			<-releaseFirst
			return start
		}
		return end
	}

	firstDone := make(chan bool, 1)
	go func() {
		allowed, _ := l.allow(remoteAddr)
		firstDone <- allowed
	}()
	clockLocked := <-firstSample
	secondDone := make(chan bool, 1)
	go func() {
		allowed, _ := l.allow(remoteAddr)
		secondDone <- allowed
	}()
	var secondAllowed bool
	if !clockLocked {
		secondAllowed = <-secondDone
	}
	close(releaseFirst)
	firstAllowed := <-firstDone
	if clockLocked {
		secondAllowed = <-secondDone
	}

	l.mu.Lock()
	entry := l.clients[clientIP(remoteAddr)]
	l.mu.Unlock()
	if !entry.updated.Equal(end) || !entry.lastSeen.Equal(end) {
		t.Errorf("concurrent requests regressed bucket timestamps: updated=%v lastSeen=%v, want %v", entry.updated, entry.lastSeen, end)
	}

	accepted := 0
	for _, allowed := range []bool{firstAllowed, secondAllowed} {
		if allowed {
			accepted++
		}
	}
	if allowed, retry := l.allow(remoteAddr); allowed {
		accepted++
		t.Error("the same refill interval supplied another token")
	} else if retry != 1 {
		t.Errorf("retry = %d, want 1 second", retry)
	}
	if accepted != 1 {
		t.Errorf("accepted %d requests from a one-token refill, want 1", accepted)
	}
}

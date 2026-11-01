// Package snowflake generates positive, signed 64-bit IDs using 41 bits of
// milliseconds since a configured epoch, 10 bits of worker ID, and 12 bits of
// sequence number. Each worker can issue 4,096 IDs per millisecond.
//
// Every concurrently running generator must have a distinct worker ID. Worker
// allocation and safe reuse across restarts must be coordinated by the caller;
// this package does not persist timestamp or sequence state. Reusing a worker ID
// while an earlier generator can still issue IDs, or restarting it in an already
// used millisecond, can produce duplicates across generators.
package snowflake

import (
	"context"
	"errors"
	"fmt"
	"sync"
	"time"
)

const (
	timestampBits = 41
	workerBits    = 10
	sequenceBits  = 12

	// MaxWorkerID is the largest worker ID supported by the 10-bit worker field.
	MaxWorkerID  uint16 = 1<<workerBits - 1
	maxTimestamp int64  = 1<<timestampBits - 1
	maxSequence  int64  = 1<<sequenceBits - 1

	workerShift    = sequenceBits
	timestampShift = workerBits + sequenceBits
)

var (
	ErrInvalidWorkerID     = errors.New("snowflake: worker ID must be between 0 and 1023")
	ErrBeforeEpoch         = errors.New("snowflake: current time is before the epoch")
	ErrClockMovedBackwards = errors.New("snowflake: clock moved backwards")
	ErrTimestampOverflow   = errors.New("snowflake: timestamp exceeds 41 bits")
	ErrNonPositiveID       = errors.New("snowflake: generated ID would not be positive")
)

// Generator is safe for concurrent use. Construct it with New.
type Generator struct {
	mu sync.Mutex

	workerID      uint16
	epoch         time.Time
	lastTimestamp int64
	sequence      int64
	hasLast       bool

	// Keep time and waiting injectable for deterministic boundary tests.
	now  func() time.Time
	wait func(context.Context, time.Duration) error
}

// New creates a generator with a worker ID in [0, MaxWorkerID]. The epoch is
// rounded down to a whole millisecond. Next accepts timestamps from that epoch
// through epoch + (2^41 - 1) milliseconds, inclusive.
func New(workerID uint16, epoch time.Time) (*Generator, error) {
	if workerID > MaxWorkerID {
		return nil, ErrInvalidWorkerID
	}
	return &Generator{
		workerID: workerID,
		epoch:    epoch.Truncate(time.Millisecond),
		now:      time.Now,
		wait:     waitForTime,
	}, nil
}

// Next returns a positive ID, waiting for the next millisecond if the sequence
// is exhausted. Waiting observes ctx cancellation. A clock rollback is returned
// as ErrClockMovedBackwards and never resets the previously issued sequence.
// At the exact epoch, worker 0 returns ErrNonPositiveID until time advances,
// because its first ID would otherwise be zero.
func (g *Generator) Next(ctx context.Context) (int64, error) {
	for {
		if err := ctx.Err(); err != nil {
			return 0, err
		}

		g.mu.Lock()
		if err := ctx.Err(); err != nil {
			g.mu.Unlock()
			return 0, err
		}
		now := g.now().Truncate(time.Millisecond)
		timestamp := int64(now.Sub(g.epoch) / time.Millisecond)
		if g.hasLast && timestamp < g.lastTimestamp {
			last := g.lastTimestamp
			g.mu.Unlock()
			return 0, fmt.Errorf("%w: current millisecond %d precedes %d", ErrClockMovedBackwards, timestamp, last)
		}
		if now.Before(g.epoch) {
			g.mu.Unlock()
			return 0, ErrBeforeEpoch
		}
		if timestamp > maxTimestamp {
			g.mu.Unlock()
			return 0, ErrTimestampOverflow
		}

		var sequence int64
		if g.hasLast && timestamp == g.lastTimestamp {
			if g.sequence == maxSequence {
				g.mu.Unlock()
				if err := g.wait(ctx, time.Millisecond); err != nil {
					return 0, err
				}
				continue
			}
			sequence = g.sequence + 1
		}
		id := timestamp<<timestampShift | int64(g.workerID)<<workerShift | sequence
		if id <= 0 {
			g.mu.Unlock()
			return 0, ErrNonPositiveID
		}
		g.lastTimestamp = timestamp
		g.sequence = sequence
		g.hasLast = true
		g.mu.Unlock()
		return id, nil
	}
}

func waitForTime(ctx context.Context, delay time.Duration) error {
	timer := time.NewTimer(delay)
	defer timer.Stop()
	select {
	case <-ctx.Done():
		return ctx.Err()
	case <-timer.C:
		return nil
	}
}

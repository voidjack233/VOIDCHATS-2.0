package snowflake

import (
	"context"
	"errors"
	"math"
	"sync"
	"sync/atomic"
	"testing"
	"time"
)

func newTestGenerator(t *testing.T, worker uint16, timestamp int64) (*Generator, *atomic.Int64) {
	t.Helper()
	epoch := time.Date(2024, time.January, 1, 0, 0, 0, 0, time.UTC)
	g, err := New(worker, epoch)
	if err != nil {
		t.Fatal(err)
	}
	clock := &atomic.Int64{}
	clock.Store(timestamp)
	g.now = func() time.Time { return epoch.Add(time.Duration(clock.Load()) * time.Millisecond) }
	return g, clock
}

func TestWorkerIDBounds(t *testing.T) {
	for _, worker := range []uint16{0, MaxWorkerID} {
		if _, err := New(worker, time.Now()); err != nil {
			t.Fatalf("worker %d: %v", worker, err)
		}
	}
	if _, err := New(MaxWorkerID+1, time.Now()); !errors.Is(err, ErrInvalidWorkerID) {
		t.Fatalf("expected invalid worker error, got %v", err)
	}
}

func TestLayoutAndSequenceReset(t *testing.T) {
	g, clock := newTestGenerator(t, 513, 12345)
	for sequence := int64(0); sequence < 3; sequence++ {
		id, err := g.Next(context.Background())
		if err != nil {
			t.Fatal(err)
		}
		if id>>timestampShift != 12345 || (id>>workerShift)&int64(MaxWorkerID) != 513 || id&maxSequence != sequence {
			t.Fatalf("unexpected ID layout: %d", id)
		}
	}
	clock.Add(1)
	id, err := g.Next(context.Background())
	if err != nil {
		t.Fatal(err)
	}
	if id>>timestampShift != 12346 || id&maxSequence != 0 {
		t.Fatalf("sequence did not reset in next millisecond: %d", id)
	}
}

func TestRollbackPreservesSequence(t *testing.T) {
	g, clock := newTestGenerator(t, 1, 100)
	first, err := g.Next(context.Background())
	if err != nil {
		t.Fatal(err)
	}
	clock.Store(99)
	if _, err := g.Next(context.Background()); !errors.Is(err, ErrClockMovedBackwards) {
		t.Fatalf("expected rollback error, got %v", err)
	}
	clock.Store(100)
	next, err := g.Next(context.Background())
	if err != nil {
		t.Fatal(err)
	}
	if next != first+1 {
		t.Fatalf("rollback changed sequence: first=%d next=%d", first, next)
	}
}

func TestTimestampBoundsAndPositiveIDs(t *testing.T) {
	g, clock := newTestGenerator(t, MaxWorkerID, maxTimestamp)
	g.hasLast = true
	g.lastTimestamp = maxTimestamp
	g.sequence = maxSequence - 1
	id, err := g.Next(context.Background())
	if err != nil || id != math.MaxInt64 {
		t.Fatalf("expected maximum signed int64, got id=%d err=%v", id, err)
	}
	clock.Store(maxTimestamp + 1)
	if _, err := g.Next(context.Background()); !errors.Is(err, ErrTimestampOverflow) {
		t.Fatalf("expected overflow error, got %v", err)
	}

	g, clock = newTestGenerator(t, 0, -1)
	if _, err := g.Next(context.Background()); !errors.Is(err, ErrBeforeEpoch) {
		t.Fatalf("expected before-epoch error, got %v", err)
	}
	clock.Store(0)
	if _, err := g.Next(context.Background()); !errors.Is(err, ErrNonPositiveID) {
		t.Fatalf("expected nonpositive ID error, got %v", err)
	}
	clock.Store(1)
	if id, err := g.Next(context.Background()); err != nil || id <= 0 {
		t.Fatalf("expected positive ID after epoch, got id=%d err=%v", id, err)
	}
}

func TestSequenceExhaustionWaitsForNextMillisecond(t *testing.T) {
	g, clock := newTestGenerator(t, 7, 100)
	waits := 0
	g.wait = func(ctx context.Context, delay time.Duration) error {
		waits++
		if delay <= 0 {
			t.Fatalf("expected positive wait, got %s", delay)
		}
		clock.Add(1)
		return nil
	}
	var last int64
	for i := int64(0); i <= maxSequence; i++ {
		id, err := g.Next(context.Background())
		if err != nil {
			t.Fatal(err)
		}
		last = id
	}
	if waits != 0 {
		t.Fatalf("waited before exhausting sequence: %d", waits)
	}
	id, err := g.Next(context.Background())
	if err != nil {
		t.Fatal(err)
	}
	if waits != 1 || id <= last || id>>timestampShift != 101 || id&maxSequence != 0 {
		t.Fatalf("unexpected rollover: waits=%d last=%d id=%d", waits, last, id)
	}
}

func TestExhaustedSequenceCanBeCanceled(t *testing.T) {
	g, clock := newTestGenerator(t, 1, 100)
	g.hasLast = true
	g.lastTimestamp = 100
	g.sequence = maxSequence
	enteredWait := make(chan struct{})
	g.wait = func(ctx context.Context, _ time.Duration) error {
		close(enteredWait)
		<-ctx.Done()
		return ctx.Err()
	}
	ctx, cancel := context.WithCancel(context.Background())
	defer cancel()
	result := make(chan error, 1)
	go func() {
		_, err := g.Next(ctx)
		result <- err
	}()
	select {
	case <-enteredWait:
	case <-time.After(time.Second):
		t.Fatal("generator never waited for exhausted sequence")
	}

	// A caller waiting for time must not keep the mutex and prevent another
	// caller from using a newly available millisecond.
	clock.Add(1)
	nextResult := make(chan error, 1)
	go func() {
		_, err := g.Next(context.Background())
		nextResult <- err
	}()
	select {
	case err := <-nextResult:
		if err != nil {
			t.Fatalf("concurrent caller could not generate an ID: %v", err)
		}
	case <-time.After(time.Second):
		t.Fatal("waiting caller kept the generator locked")
	}

	canceled, stop := context.WithCancel(context.Background())
	stop()
	if _, err := g.Next(canceled); !errors.Is(err, context.Canceled) {
		t.Fatalf("expected cancellation, got %v", err)
	}
	cancel()
	select {
	case err := <-result:
		if !errors.Is(err, context.Canceled) {
			t.Fatalf("expected cancellation while waiting, got %v", err)
		}
	case <-time.After(time.Second):
		t.Fatal("generator did not return after cancellation")
	}
}

func TestConcurrentIDsAreUnique(t *testing.T) {
	g, clock := newTestGenerator(t, 42, 1)
	g.wait = func(context.Context, time.Duration) error {
		clock.Add(1)
		return nil
	}
	const workers, perWorker = 24, 1000
	ids := make(chan int64, workers*perWorker)
	errs := make(chan error, workers)
	var wg sync.WaitGroup
	for i := 0; i < workers; i++ {
		wg.Add(1)
		go func() {
			defer wg.Done()
			for j := 0; j < perWorker; j++ {
				id, err := g.Next(context.Background())
				if err != nil {
					errs <- err
					return
				}
				ids <- id
			}
		}()
	}
	wg.Wait()
	close(ids)
	close(errs)
	for err := range errs {
		t.Fatal(err)
	}
	seen := make(map[int64]bool, workers*perWorker)
	for id := range ids {
		if id <= 0 || seen[id] {
			t.Fatalf("invalid or duplicated ID: %d", id)
		}
		seen[id] = true
	}
	if len(seen) != workers*perWorker {
		t.Fatalf("generated %d IDs, expected %d", len(seen), workers*perWorker)
	}
}

func TestWaitObservesCanceledContext(t *testing.T) {
	ctx, cancel := context.WithCancel(context.Background())
	cancel()
	if err := waitForTime(ctx, time.Hour); !errors.Is(err, context.Canceled) {
		t.Fatalf("expected cancellation, got %v", err)
	}
}

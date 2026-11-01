package relay

import (
	"bufio"
	"context"
	"errors"
	"io"
	"net"
	"net/http"
	"net/http/httptest"
	"sync"
	"testing"
	"time"

	"github.com/coder/websocket"
)

type delayedUpgrade struct {
	header  http.Header
	entered chan struct{}
	release chan struct{}
	conn    net.Conn
}

func (w *delayedUpgrade) Header() http.Header            { return w.header }
func (w *delayedUpgrade) WriteHeader(int)                {}
func (w *delayedUpgrade) Write(body []byte) (int, error) { return len(body), nil }
func (w *delayedUpgrade) Hijack() (net.Conn, *bufio.ReadWriter, error) {
	close(w.entered)
	<-w.release
	return w.conn, bufio.NewReadWriter(bufio.NewReader(w.conn), bufio.NewWriter(w.conn)), nil
}

func TestShutdownDuringUpgradeClosesNewlyHijackedSocket(t *testing.T) {
	f := newFixture(t, Options{}, 1)
	serverConn, peer := net.Pipe()
	defer peer.Close()
	w := &delayedUpgrade{header: make(http.Header), entered: make(chan struct{}), release: make(chan struct{}), conn: serverConn}
	r := httptest.NewRequest(http.MethodGet, "http://example.test/ws", nil)
	r.Header.Set("Connection", "Upgrade")
	r.Header.Set("Upgrade", "websocket")
	r.Header.Set("Sec-WebSocket-Version", "13")
	r.Header.Set("Sec-WebSocket-Key", "MDEyMzQ1Njc4OWFiY2RlZg==")
	done := make(chan struct{})
	go func() { defer close(done); f.server.ServeHTTP(w, r) }()
	mustReceive(t, w.entered)
	if connectionCount(f.server) != 1 {
		t.Fatal("upgrade did not reserve capacity")
	}
	if err := f.server.Close(); err != nil {
		t.Fatal(err)
	}
	close(w.release)
	mustReceive(t, done)
	// net.Pipe may already report a closed pipe when installing the deadline.
	_ = peer.SetReadDeadline(time.Now().Add(time.Second))
	if _, err := peer.Read(make([]byte, 1)); !errors.Is(err, io.EOF) {
		t.Fatalf("socket upgraded during shutdown remained open: %v", err)
	}
	if connectionCount(f.server) != 0 {
		t.Fatal("shutdown upgrade reservation leaked")
	}
}

type controlledSocket struct {
	started  chan struct{}
	release  chan struct{}
	closed   chan struct{}
	once     sync.Once
	writeErr error
}

func newControlledSocket() *controlledSocket {
	return &controlledSocket{started: make(chan struct{}, 8), release: make(chan struct{}), closed: make(chan struct{})}
}

func (c *controlledSocket) Read(ctx context.Context) (websocket.MessageType, []byte, error) {
	select {
	case <-ctx.Done():
		return 0, nil, ctx.Err()
	case <-c.closed:
		return 0, nil, errStopped
	}
}

func (c *controlledSocket) Write(ctx context.Context, _ websocket.MessageType, _ []byte) error {
	c.started <- struct{}{}
	select {
	case <-ctx.Done():
		return ctx.Err()
	case <-c.closed:
		return errStopped
	case <-c.release:
		return c.writeErr
	}
}

func (c *controlledSocket) Ping(ctx context.Context) error { return ctx.Err() }
func (c *controlledSocket) CloseNow() error                { c.once.Do(func() { close(c.closed) }); return nil }

func fakeSession(t *testing.T, s *Server, id string, conn socket) *session {
	t.Helper()
	ctx, cancel := context.WithCancel(context.Background())
	client := &session{server: s, ctx: ctx, cancel: cancel, out: make(chan outbound, s.opts.QueueSize), conn: conn, voidID: id}
	s.mu.Lock()
	s.connections[client] = struct{}{}
	s.active[id] = client
	s.mu.Unlock()
	t.Cleanup(func() { client.stop(); s.remove(client) })
	return client
}

func runWriter(t *testing.T, client *session) {
	t.Helper()
	done := make(chan struct{})
	go func() { defer close(done); client.writeLoop() }()
	t.Cleanup(func() {
		client.stop()
		select {
		case <-done:
		case <-time.After(time.Second):
			t.Error("writer did not exit")
		}
	})
}

func mustReceive[T any](t *testing.T, ch <-chan T) T {
	t.Helper()
	select {
	case value := <-ch:
		return value
	case <-time.After(time.Second):
		t.Fatal("timed out waiting for test event")
		var zero T
		return zero
	}
}

func TestSlowRecipientQueueAndWriteReceipts(t *testing.T) {
	f := newFixture(t, Options{QueueSize: 1, ForwardTimeout: 2 * time.Second}, 1)
	sender := fakeSession(t, f.server, "1", newControlledSocket())
	slowSocket := newControlledSocket()
	slow := fakeSession(t, f.server, "2", slowSocket)
	fastSocket := newControlledSocket()
	close(fastSocket.release)
	fast := fakeSession(t, f.server, "3", fastSocket)
	runWriter(t, slow)
	runWriter(t, fast)
	first := make(chan string, 1)
	go func() { first <- sender.forward(send("first", "2", "AQ==")) }()
	mustReceive(t, slowSocket.started)
	select {
	case status := <-first:
		t.Fatalf("result before recipient write completed: %s", status)
	default:
	}
	second := make(chan string, 1)
	go func() { second <- sender.forward(send("second", "2", "Ag==")) }()
	eventually(t, "outgoing queue never filled", func() bool { return len(slow.out) == 1 })
	if status := sender.forward(send("over-capacity", "2", "Aw==")); status != "overloaded" {
		t.Fatalf("full queue status=%s", status)
	}
	// A separate recipient still progresses while the first transport is stuck.
	if status := sender.forward(send("unrelated", "3", "BA==")); status != "forwarded" {
		t.Fatalf("unrelated recipient blocked: %s", status)
	}
	close(slowSocket.release)
	if status := mustReceive(t, first); status != "forwarded" {
		t.Fatalf("completed write status=%s", status)
	}
	if status := mustReceive(t, second); status != "forwarded" {
		t.Fatalf("queued write status=%s", status)
	}
}

func TestQueuedMessageIsNotReportedForwarded(t *testing.T) {
	f := newFixture(t, Options{QueueSize: 1, ForwardTimeout: 40 * time.Millisecond}, 1)
	sender := fakeSession(t, f.server, "1", newControlledSocket())
	recipient := fakeSession(t, f.server, "2", newControlledSocket())
	// No writer: accepting an item into a queue must not report forwarding.
	if status := sender.forward(send("queued", "2", "AQ==")); status != "failed" {
		t.Fatalf("queue acceptance status=%s", status)
	}
	if len(recipient.out) != 1 {
		t.Fatal("test did not exercise queue acceptance")
	}
	item := <-recipient.out
	if item.ctx.Err() == nil {
		t.Fatal("expired queued work could still be written")
	}
}

func TestFailedRecipientWriteAndTimeout(t *testing.T) {
	for _, test := range []struct {
		name string
		fail bool
	}{{"write failure", true}, {"write timeout", false}} {
		t.Run(test.name, func(t *testing.T) {
			f := newFixture(t, Options{WriteTimeout: 40 * time.Millisecond, ForwardTimeout: time.Second}, 1)
			sender := fakeSession(t, f.server, "1", newControlledSocket())
			conn := newControlledSocket()
			if test.fail {
				conn.writeErr = errStopped
				close(conn.release)
			}
			recipient := fakeSession(t, f.server, "2", conn)
			runWriter(t, recipient)
			if status := sender.forward(send("failure", "2", "AQ==")); status != "failed" {
				t.Fatalf("failed write status=%s", status)
			}
			eventually(t, "failed recipient was not stopped", func() bool { return recipient.ctx.Err() != nil })
		})
	}
}

func TestReplacedSenderCannotEmitQueuedMessage(t *testing.T) {
	f := newFixture(t, Options{ForwardTimeout: time.Second}, 1)
	old := fakeSession(t, f.server, "1", newControlledSocket())
	conn := newControlledSocket()
	close(conn.release)
	recipient := fakeSession(t, f.server, "2", conn)
	status := make(chan string, 1)
	go func() { status <- old.forward(send("stale-queued", "2", "AQ==")) }()
	eventually(t, "message was not queued", func() bool { return len(recipient.out) == 1 })
	// Publish a replacement before starting the recipient writer. The old
	// message must fail its registry check even if socket cancellation races.
	_ = fakeSession(t, f.server, "1", newControlledSocket())
	runWriter(t, recipient)
	if result := mustReceive(t, status); result != "failed" {
		t.Fatalf("replaced sender status=%s", result)
	}
	select {
	case <-conn.started:
		t.Fatal("stale queued message reached transport")
	default:
	}
}

// Package relay binds existing account identities to bounded, process-local
// WebSocket sessions and forwards opaque bytes between online accounts.
package relay

import (
	"context"
	"crypto/ed25519"
	"encoding/json"
	"errors"
	"log/slog"
	"net/http"
	"sync"
	"sync/atomic"
	"time"

	"github.com/coder/websocket"

	"void-nothingness/internal/auth"
	"void-nothingness/internal/prekeys"
	"void-nothingness/internal/queue"
)

const (
	DefaultMaxPayloadBytes = queue.MaxPayloadBytes
	DefaultMaxFrameBytes   = ((DefaultMaxPayloadBytes + 2) / 3 * 4) + 2048
	maxQueuedBytes         = 2 << 20
)

var errStopped = errors.New("relay session stopped")

// Options uses safe defaults for zero values. Capacity and duration overrides
// permit smaller deployments and deterministic tests; payloads remain <=1 MiB.
type Options struct {
	MaxConnections  int
	QueueSize       int
	MaxPayloadBytes int
	MaxFrameBytes   int64
	AuthTimeout     time.Duration
	PingInterval    time.Duration
	PongTimeout     time.Duration
	WriteTimeout    time.Duration
	ForwardTimeout  time.Duration
	Logger          *slog.Logger
	Queue           *queue.Service
	Prekeys         *prekeys.Service
}

// Server is an HTTP handler for the WebSocket endpoint. Close must be called
// explicitly during HTTP shutdown because upgraded sockets are hijacked.
type Server struct {
	auth        *auth.Service
	opts        Options
	mu          sync.Mutex
	closed      bool
	connections map[*session]struct{}
	active      map[string]*session
	rates       map[string]commandBucket
	slots       chan struct{}
}

type commandBucket struct {
	tokens       float64
	seen         time.Time
	prekeyTokens float64
	prekeySeen   time.Time
}

type session struct {
	server *Server
	ctx    context.Context
	cancel context.CancelFunc
	out    chan outbound
	// conn is assigned after admission; shutdown may happen during the upgrade.
	connMu         sync.Mutex
	conn           socket
	voidID         string // Set once at activation before publishing in the registry.
	deliveryMu     sync.Mutex
	inFlightSender string
	inFlightID     string
	inFlightExpiry time.Time
	wake           chan struct{}
	queuedBytes    atomic.Int64
}

// The transport contract also permits deterministic slow-write tests without
// depending on operating-system socket buffer sizes.
type socket interface {
	Read(context.Context) (websocket.MessageType, []byte, error)
	Write(context.Context, websocket.MessageType, []byte) error
	Ping(context.Context) error
	CloseNow() error
}

type outbound struct {
	body    []byte
	ctx     context.Context
	receipt chan error
	source  *session
}

// New creates a server with one active connection per VOID-ID. A new successful
// authentication replaces the previous connection; failed attempts never do.
func New(authenticator *auth.Service, opts Options) (*Server, error) {
	if authenticator == nil || opts.Queue == nil {
		return nil, errors.New("authentication and delivery queue services are required")
	}
	if opts.MaxConnections == 0 {
		opts.MaxConnections = 256
	}
	if opts.QueueSize == 0 {
		opts.QueueSize = 16
	}
	if opts.MaxPayloadBytes == 0 {
		opts.MaxPayloadBytes = DefaultMaxPayloadBytes
	}
	if opts.MaxFrameBytes == 0 {
		opts.MaxFrameBytes = DefaultMaxFrameBytes
	}
	if opts.AuthTimeout == 0 {
		opts.AuthTimeout = 10 * time.Second
	}
	if opts.PingInterval == 0 {
		opts.PingInterval = 30 * time.Second
	}
	if opts.PongTimeout == 0 {
		opts.PongTimeout = 10 * time.Second
	}
	if opts.WriteTimeout == 0 {
		opts.WriteTimeout = 5 * time.Second
	}
	if opts.ForwardTimeout == 0 {
		opts.ForwardTimeout = 6 * time.Second
	}
	if opts.Logger == nil {
		opts.Logger = slog.Default()
	}
	if opts.MaxConnections < 1 || opts.MaxConnections > 100000 || opts.QueueSize < 1 || opts.QueueSize > 1024 ||
		opts.MaxPayloadBytes < 1 || opts.MaxPayloadBytes > DefaultMaxPayloadBytes ||
		opts.MaxFrameBytes < 1 || opts.MaxFrameBytes > DefaultMaxFrameBytes ||
		opts.AuthTimeout <= 0 || opts.AuthTimeout > 10*time.Second || opts.PingInterval <= 0 ||
		opts.PongTimeout <= 0 || opts.WriteTimeout <= 0 || opts.ForwardTimeout <= 0 {
		return nil, errors.New("invalid relay configuration")
	}
	return &Server{auth: authenticator, opts: opts, connections: make(map[*session]struct{}), active: make(map[string]*session), rates: make(map[string]commandBucket), slots: make(chan struct{}, 64)}, nil
}

func (s *Server) ServeHTTP(w http.ResponseWriter, r *http.Request) {
	w.Header().Set("Cache-Control", "no-store")
	w.Header().Set("X-Content-Type-Options", "nosniff")
	if r.Method != http.MethodGet {
		w.Header().Set("Allow", http.MethodGet)
		http.Error(w, "Method not allowed.", http.StatusMethodNotAllowed)
		return
	}
	// Admission is reserved before upgrading, including unauthenticated sockets.
	// A standalone context is required after HTTP hijacking; request contexts
	// are not a reliable lifetime for upgraded connections.
	ctx, cancel := context.WithCancel(context.Background())
	client := &session{server: s, ctx: ctx, cancel: cancel, out: make(chan outbound, s.opts.QueueSize), wake: make(chan struct{}, 1)}
	s.mu.Lock()
	if s.closed || len(s.connections) >= s.opts.MaxConnections {
		s.mu.Unlock()
		cancel()
		w.Header().Set("Retry-After", "1")
		http.Error(w, "WebSocket service unavailable.", http.StatusServiceUnavailable)
		return
	}
	s.connections[client] = struct{}{}
	s.mu.Unlock()
	defer s.remove(client)
	conn, err := websocket.Accept(w, r, &websocket.AcceptOptions{CompressionMode: websocket.CompressionDisabled})
	if err != nil {
		return
	}
	client.connMu.Lock()
	client.conn = conn
	stopped := client.ctx.Err() != nil
	client.connMu.Unlock()
	if stopped {
		_ = conn.CloseNow()
		return
	}
	// Authentication never needs a full ciphertext frame. Keep unproven
	// connections on a smaller input budget until ownership is established.
	conn.SetReadLimit(min(s.opts.MaxFrameBytes, 8*1024))
	defer client.stop()
	s.opts.Logger.Info("websocket connected")
	defer func() { s.opts.Logger.Info("websocket disconnected", "void_id", client.voidID) }()
	var workers sync.WaitGroup
	workers.Add(2)
	go func() { defer workers.Done(); client.writeLoop() }()
	go func() { defer workers.Done(); client.heartbeat() }()
	// Cancel before joining workers, including when authentication fails.
	defer func() { client.stop(); workers.Wait() }()
	if !client.authenticate() {
		return
	}
	conn.SetReadLimit(s.opts.MaxFrameBytes)
	workers.Add(1)
	go func() { defer workers.Done(); client.deliveryLoop() }()
	client.readLoop()
}

func (s *Server) remove(client *session) {
	client.cancel()
	s.mu.Lock()
	delete(s.connections, client)
	if s.active[client.voidID] == client {
		delete(s.active, client.voidID)
	}
	s.mu.Unlock()
}

func (client *session) stop() {
	client.cancel()
	client.connMu.Lock()
	conn := client.conn
	client.connMu.Unlock()
	if conn != nil {
		// No close handshake waits for an unresponsive peer. CloseRead is never
		// used, so CloseNow does not wait for its optional reader goroutine.
		_ = conn.CloseNow()
	}
}

// Close rejects future upgrades and closes every admitted socket, including
// unauthenticated connections. It is safe to call repeatedly or concurrently.
func (s *Server) Close() error {
	s.mu.Lock()
	s.closed = true
	clients := make([]*session, 0, len(s.connections))
	for client := range s.connections {
		clients = append(clients, client)
	}
	s.mu.Unlock()
	for _, client := range clients {
		client.stop()
	}
	return nil
}

// ActiveCount reports authenticated sessions, for local observation.
func (s *Server) ActiveCount() int {
	s.mu.Lock()
	defer s.mu.Unlock()
	return len(s.active)
}

func (s *Server) isCurrent(client *session) bool {
	s.mu.Lock()
	defer s.mu.Unlock()
	return !s.closed && client.ctx.Err() == nil && s.active[client.voidID] == client
}

func (s *Server) canWrite(client *session) bool {
	s.mu.Lock()
	defer s.mu.Unlock()
	return !s.closed && client.ctx.Err() == nil && (client.voidID == "" || s.active[client.voidID] == client)
}

func (client *session) authenticate() bool {
	ctx, cancel := context.WithTimeout(client.ctx, client.server.opts.AuthTimeout)
	defer cancel()
	fields, err := client.read(ctx)
	if err != nil {
		if errors.Is(err, errInvalidFrame) {
			client.sendError(ctx, "invalid_frame")
		}
		return false
	}
	if fields["type"] == "send" || fields["type"] == "prekeys_publish" || fields["type"] == "prekeys_claim" {
		client.sendError(ctx, "unauthorized")
		return false
	}
	if !exactFields(fields, "auth_begin", "void_id") || !auth.ValidVoidID(fields["void_id"]) {
		client.sendError(ctx, "invalid_frame")
		return false
	}
	challenge, err := client.server.auth.Begin(ctx, fields["void_id"])
	if err != nil {
		code := "authentication_failed"
		if errors.Is(err, auth.ErrUnavailable) {
			code = "unavailable"
		}
		client.server.opts.Logger.Info("websocket authentication failed", "code", code)
		client.sendError(ctx, code)
		return false
	}
	if err := client.transmit(ctx, struct {
		Type        string    `json:"type"`
		ChallengeID string    `json:"challenge_id"`
		Message     string    `json:"message"`
		ExpiresAt   time.Time `json:"expires_at"`
	}{"auth_challenge", challenge.ChallengeID, challenge.Message, challenge.ExpiresAt}); err != nil {
		return false
	}
	fields, err = client.read(ctx)
	if err != nil {
		if errors.Is(err, errInvalidFrame) {
			client.sendError(ctx, "invalid_frame")
		}
		return false
	}
	if !exactFields(fields, "auth_proof", "challenge_id", "signature") {
		client.sendError(ctx, "invalid_frame")
		return false
	}
	signature, err := decodeBase64(fields["signature"], ed25519.SignatureSize)
	if err != nil {
		client.sendError(ctx, "authentication_failed")
		return false
	}
	voidID, err := challenge.Verify(fields["challenge_id"], signature)
	if err != nil || ctx.Err() != nil {
		client.server.opts.Logger.Info("websocket authentication failed", "code", "authentication_failed")
		client.sendError(ctx, "authentication_failed")
		return false
	}
	// Queue auth_ok before publishing, while the writer's source check shares
	// the registry lock. Other senders can only queue after auth_ok, and a peer
	// that observes auth_ok is already an active recipient.
	body, _ := json.Marshal(map[string]string{"type": "auth_ok", "void_id": voidID})
	item := outbound{body: body, ctx: ctx, receipt: make(chan error, 1), source: client}
	s := client.server
	s.mu.Lock()
	if s.closed || client.ctx.Err() != nil || ctx.Err() != nil {
		s.mu.Unlock()
		return false
	}
	if !client.enqueue(item) {
		s.mu.Unlock()
		return false
	}
	client.voidID = voidID
	old := s.active[voidID]
	s.active[voidID] = client
	s.mu.Unlock()
	if old != nil {
		old.stop()
	}
	if waitReceipt(ctx, client.ctx, item.receipt) != nil {
		return false
	}
	s.opts.Logger.Info("websocket authenticated", "void_id", voidID)
	return true
}

func (client *session) read(ctx context.Context) (map[string]string, error) {
	kind, body, err := client.conn.Read(ctx)
	if err != nil {
		return nil, err
	}
	if kind != websocket.MessageText {
		return nil, errInvalidFrame
	}
	return decodeFrame(body)
}

func (client *session) readLoop() {
	for {
		fields, err := client.read(client.ctx)
		if err != nil {
			if errors.Is(err, errInvalidFrame) {
				client.sendError(client.ctx, "invalid_frame")
			}
			return
		}
		if !client.server.isCurrent(client) {
			return
		}
		if !client.handleCommand(fields) {
			return
		}
	}
}

func (client *session) forward(fields map[string]string) string {
	ctx, cancel := context.WithTimeout(client.ctx, client.server.opts.ForwardTimeout)
	defer cancel()
	body, _ := json.Marshal(map[string]string{"type": "online_event", "exchange_id": fields["exchange_id"], "from_void_id": client.voidID, "payload_base64": fields["payload_base64"]})
	item := outbound{body: body, ctx: ctx, receipt: make(chan error, 1), source: client}
	s := client.server
	s.mu.Lock()
	if s.closed || client.ctx.Err() != nil || s.active[client.voidID] != client {
		s.mu.Unlock()
		return "failed"
	}
	recipient := s.active[fields["to_void_id"]]
	if recipient == nil || recipient.ctx.Err() != nil {
		s.mu.Unlock()
		return "unavailable"
	}
	// Never block the shared registry on a slow recipient or a full queue.
	if !recipient.enqueue(item) {
		s.mu.Unlock()
		return "overloaded"
	}
	s.mu.Unlock()
	if waitReceipt(ctx, recipient.ctx, item.receipt) != nil {
		return "failed"
	}
	return "forwarded"
}

func waitReceipt(ctx, target context.Context, receipt <-chan error) error {
	select {
	case err := <-receipt:
		return err
	case <-ctx.Done():
	case <-target.Done():
	}
	// Prefer a completed transport write over a simultaneous cancellation.
	select {
	case err := <-receipt:
		return err
	default:
		return errStopped
	}
}

func (client *session) transmit(ctx context.Context, value any) error {
	body, err := json.Marshal(value)
	if err != nil {
		return errStopped
	}
	ctx, cancel := context.WithTimeout(ctx, client.server.opts.WriteTimeout)
	defer cancel()
	item := outbound{body: body, ctx: ctx, receipt: make(chan error, 1)}
	if !client.enqueue(item) {
		return errStopped
	}
	return waitReceipt(ctx, client.ctx, item.receipt)
}

func (client *session) sendError(ctx context.Context, code string) {
	_ = client.transmit(ctx, map[string]string{"type": "error", "code": code})
}

func (client *session) writeLoop() {
	for {
		select {
		case <-client.ctx.Done():
			return
		case item := <-client.out:
			if item.ctx.Err() != nil || !client.server.canWrite(client) || item.source != nil && !client.server.isCurrent(item.source) {
				client.queuedBytes.Add(-int64(len(item.body)))
				item.receipt <- errStopped
				continue
			}
			ctx, cancel := context.WithTimeout(item.ctx, client.server.opts.WriteTimeout)
			err := client.conn.Write(ctx, websocket.MessageText, item.body)
			cancel()
			client.queuedBytes.Add(-int64(len(item.body)))
			item.receipt <- err
			if err != nil {
				client.stop()
				return
			}
		}
	}
}

func (client *session) enqueue(item outbound) bool {
	if client.ctx.Err() != nil || item.ctx.Err() != nil {
		return false
	}
	size := int64(len(item.body))
	for {
		previous := client.queuedBytes.Load()
		if previous+size > maxQueuedBytes {
			return false
		}
		if client.queuedBytes.CompareAndSwap(previous, previous+size) {
			break
		}
	}
	select {
	case client.out <- item:
		return true
	default:
		client.queuedBytes.Add(-size)
		return false
	}
}

func (client *session) heartbeat() {
	ticker := time.NewTicker(client.server.opts.PingInterval)
	defer ticker.Stop()
	for {
		select {
		case <-client.ctx.Done():
			return
		case <-ticker.C:
			ctx, cancel := context.WithTimeout(client.ctx, client.server.opts.PongTimeout)
			err := client.conn.Ping(ctx)
			cancel()
			if err != nil {
				client.stop()
				return
			}
		}
	}
}

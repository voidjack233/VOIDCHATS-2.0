// Package messagingapi serves the durable messaging contract independently of
// registration's smaller body, rate and concurrency budgets.
package messagingapi

import (
	"context"
	"encoding/base64"
	"errors"
	"net/http"
	"net/url"
	"strconv"
	"strings"
	"time"

	"void-nothingness/internal/auth"
	"void-nothingness/internal/messaging"
	"void-nothingness/internal/session"
)

const Prefix = "/dm9pZG5vdGhpbmduZXNz"

type Sessions interface {
	Challenge(context.Context, string) (session.Challenge, error)
	Create(context.Context, string, []byte) (session.Token, error)
	Authenticate(context.Context, string) (string, error)
	Revoke(context.Context, string) error
}
type Messaging interface {
	Lookup(context.Context, string, string) (messaging.Person, error)
	Publish(context.Context, string, [][]byte) ([]messaging.PublishedPackage, error)
	Claim(context.Context, string, string, string) (messaging.Claim, error)
	CreateRequest(context.Context, string, messaging.CreateRequestInput) (messaging.Request, error)
	ListRequests(context.Context, string, string, int) (messaging.RequestPage, error)
	Accept(context.Context, string, string) (messaging.Request, error)
	Decline(context.Context, string, string) (messaging.Request, error)
	Block(context.Context, string, string) error
	ListConversations(context.Context, string, string, int) (messaging.ConversationPage, error)
	Send(context.Context, string, string, messaging.SendInput) (messaging.Message, error)
	Sync(context.Context, string, string, int) (messaging.SyncPage, error)
	Ack(context.Context, string, []messaging.Acknowledgment) error
}

type Options struct {
	RequestTimeout                                                   time.Duration
	RatePerMinute, RateBurst, MaxRateAccounts, MaxConcurrentRequests int
	AuthRatePerMinute, AuthBurst, MaxAuthClients                     int
}
type Handler struct {
	sessions         Sessions
	messaging        Messaging
	api              *http.ServeMux
	slots            chan struct{}
	accounts, unauth *limiter
	timeout          time.Duration
}

func New(sessions Sessions, service Messaging, opts Options) http.Handler {
	if opts.RequestTimeout <= 0 {
		opts.RequestTimeout = 10 * time.Second
	}
	if opts.RatePerMinute <= 0 {
		opts.RatePerMinute = 120
	}
	if opts.RateBurst <= 0 {
		opts.RateBurst = 40
	}
	if opts.MaxRateAccounts <= 0 {
		opts.MaxRateAccounts = 4096
	}
	if opts.MaxConcurrentRequests <= 0 {
		opts.MaxConcurrentRequests = 64
	}
	if opts.AuthRatePerMinute <= 0 {
		opts.AuthRatePerMinute = 30
	}
	if opts.AuthBurst <= 0 {
		opts.AuthBurst = 10
	}
	if opts.MaxAuthClients <= 0 {
		opts.MaxAuthClients = 4096
	}
	h := &Handler{sessions: sessions, messaging: service, api: http.NewServeMux(), slots: make(chan struct{}, opts.MaxConcurrentRequests), accounts: newLimiter(opts.RatePerMinute, opts.RateBurst, opts.MaxRateAccounts), unauth: newLimiter(opts.AuthRatePerMinute, opts.AuthBurst, opts.MaxAuthClients), timeout: opts.RequestTimeout}
	h.route("POST", "/auth/challenges", false, h.challenge)
	h.route("POST", "/auth/sessions", false, h.createSession)
	h.route("DELETE", "/auth/session", true, h.revokeSession)
	h.route("GET", "/people/{void_id}", true, h.person)
	h.route("POST", "/key-packages", true, h.publish)
	h.route("POST", "/people/{void_id}/key-packages/claim", true, h.claim)
	h.route("POST", "/requests", true, h.createRequest)
	h.route("GET", "/requests", true, h.requests)
	h.route("POST", "/requests/{id}/accept", true, h.accept)
	h.route("POST", "/requests/{id}/decline", true, h.decline)
	h.route("PUT", "/blocks/{void_id}", true, h.block)
	h.route("GET", "/conversations", true, h.conversations)
	h.route("POST", "/conversations/{id}/messages", true, h.send)
	h.route("GET", "/sync", true, h.sync)
	h.route("POST", "/sync/ack", true, h.ack)
	return h
}

// Mount adds only messaging namespaces. Existing registration/health routes keep
// their original handler, and unknown messaging routes cannot fall through to it.
func Mount(mux *http.ServeMux, h http.Handler) {
	for _, p := range []string{"/auth/", "/people/", "/key-packages", "/requests", "/requests/", "/blocks/", "/conversations", "/conversations/", "/sync", "/sync/"} {
		mux.Handle(Prefix+p, h)
	}
}

type actorKey struct{}

func actor(r *http.Request) string { v, _ := r.Context().Value(actorKey{}).(string); return v }

func (h *Handler) route(method, path string, authenticated bool, fn http.HandlerFunc) {
	h.api.HandleFunc(method+" "+Prefix+path, func(w http.ResponseWriter, r *http.Request) {
		if h.sessions == nil || h.messaging == nil {
			writeError(w, 503, "unavailable", "Messaging is temporarily unavailable.")
			return
		}
		if authenticated {
			token, ok := bearer(r)
			if !ok {
				writeError(w, 401, "invalid_session", "A valid bearer session is required.")
				return
			}
			id, err := h.sessions.Authenticate(r.Context(), token)
			if err != nil {
				serviceError(w, err)
				return
			}
			if !throttle(w, h.accounts, id) {
				return
			}
			r = r.WithContext(context.WithValue(r.Context(), actorKey{}, id))
		} else if !throttle(w, h.unauth, remoteIP(r.RemoteAddr)) {
			return
		}
		fn(w, r)
	})
}

func (h *Handler) ServeHTTP(w http.ResponseWriter, r *http.Request) {
	w.Header().Set("Cache-Control", "no-store")
	w.Header().Set("X-Content-Type-Options", "nosniff")
	ctx, cancel := context.WithTimeout(r.Context(), h.timeout)
	defer cancel()
	r = r.WithContext(ctx)
	select {
	case h.slots <- struct{}{}:
		defer func() { <-h.slots }()
	default:
		writeError(w, 503, "busy", "Messaging is busy. Retry later.")
		return
	}
	// ServeMux's default errors are plaintext. Determine the matched route first
	// so all contract errors retain the existing JSON envelope.
	_, pattern := h.api.Handler(r)
	if pattern == "" {
		allowed := []string{}
		for _, method := range []string{"GET", "POST", "PUT", "DELETE"} {
			clone := r.Clone(ctx)
			clone.Method = method
			if _, p := h.api.Handler(clone); p != "" {
				allowed = append(allowed, method)
			}
		}
		if len(allowed) > 0 {
			w.Header().Set("Allow", strings.Join(allowed, ", "))
			writeError(w, 405, "method_not_allowed", "Method not allowed.")
		} else {
			writeError(w, 404, "not_found", "Endpoint not found.")
		}
		return
	}
	h.api.ServeHTTP(w, r)
}

func bearer(r *http.Request) (string, bool) {
	values := r.Header.Values("Authorization")
	if len(values) != 1 {
		return "", false
	}
	parts := strings.Split(values[0], " ")
	returnToken := ""
	if len(parts) == 2 && strings.EqualFold(parts[0], "Bearer") {
		returnToken = parts[1]
	}
	if len(returnToken) != 43 {
		return "", false
	}
	decoded, err := base64.RawURLEncoding.Strict().DecodeString(returnToken)
	return returnToken, err == nil && len(decoded) == 32 && base64.RawURLEncoding.EncodeToString(decoded) == returnToken
}

func throttle(w http.ResponseWriter, l *limiter, key string) bool {
	if ok, retry := l.allow(key); !ok {
		w.Header().Set("Retry-After", strconv.Itoa(retry))
		writeError(w, 429, "rate_limited", "Too many requests.")
		return false
	}
	return true
}

func serviceError(w http.ResponseWriter, err error) {
	var e *messaging.Error
	switch {
	case errors.As(err, &e):
		writeError(w, e.Status, e.Code, e.Message)
	case errors.Is(err, session.ErrInvalidInput):
		writeError(w, 400, "invalid_input", "Invalid authentication input.")
	case errors.Is(err, session.ErrAuthentication):
		writeError(w, 401, "invalid_session", "Session or authentication proof is invalid or expired.")
	case errors.Is(err, session.ErrCapacity):
		writeError(w, 429, "authentication_capacity", "Authentication capacity reached. Retry later.")
	case errors.Is(err, context.DeadlineExceeded):
		writeError(w, 504, "deadline_exceeded", "Request deadline exceeded.")
	case errors.Is(err, context.Canceled):
		writeError(w, 408, "request_canceled", "Request was canceled.")
	default:
		writeError(w, 503, "unavailable", "Messaging is temporarily unavailable.")
	}
}

func (h *Handler) challenge(w http.ResponseWriter, r *http.Request) {
	var input struct {
		VoidID string `json:"void_id"`
	}
	if !readBody(w, r, 4096, &input) {
		return
	}
	result, err := h.sessions.Challenge(r.Context(), input.VoidID)
	if err != nil {
		serviceError(w, err)
		return
	}
	writeJSON(w, 201, result)
}
func (h *Handler) createSession(w http.ResponseWriter, r *http.Request) {
	var input struct {
		ChallengeID string `json:"challenge_id"`
		Signature   string `json:"signature"`
	}
	if !readBody(w, r, 4096, &input) {
		return
	}
	sig, err := base64.StdEncoding.Strict().DecodeString(input.Signature)
	if err != nil || base64.StdEncoding.EncodeToString(sig) != input.Signature {
		sig = nil
	}
	// Even malformed proof bytes consume a syntactically valid challenge.
	result, err := h.sessions.Create(r.Context(), input.ChallengeID, sig)
	if err != nil {
		serviceError(w, err)
		return
	}
	writeJSON(w, 201, result)
}
func (h *Handler) revokeSession(w http.ResponseWriter, r *http.Request) {
	token, _ := bearer(r)
	if err := h.sessions.Revoke(r.Context(), token); err != nil {
		serviceError(w, err)
		return
	}
	w.WriteHeader(204)
}
func (h *Handler) person(w http.ResponseWriter, r *http.Request) {
	if !auth.ValidVoidID(r.PathValue("void_id")) {
		writeError(w, 400, "invalid_input", "Invalid VOID-ID.")
		return
	}
	result, err := h.messaging.Lookup(r.Context(), actor(r), r.PathValue("void_id"))
	if err != nil {
		serviceError(w, err)
		return
	}
	writeJSON(w, 200, result)
}
func (h *Handler) publish(w http.ResponseWriter, r *http.Request) {
	var input struct {
		Packages []string `json:"packages"`
	}
	if !readBody(w, r, 2<<20, &input) {
		return
	}
	if len(input.Packages) < 1 || len(input.Packages) > messaging.MaxPackageBatch {
		writeError(w, 400, "invalid_input", "Publish between 1 and 16 public KeyPackages.")
		return
	}
	packages := make([][]byte, len(input.Packages))
	total := 0
	for i, encoded := range input.Packages {
		decoded, err := base64.StdEncoding.Strict().DecodeString(encoded)
		if err != nil || len(decoded) == 0 || base64.StdEncoding.EncodeToString(decoded) != encoded {
			writeError(w, 400, "invalid_input", "Invalid public KeyPackage encoding.")
			return
		}
		total += len(decoded)
		if total > messaging.MaxOpaqueBytes {
			writeError(w, 413, "payload_too_large", "Decoded KeyPackage batch exceeds 1 MiB.")
			return
		}
		packages[i] = decoded
	}
	result, err := h.messaging.Publish(r.Context(), actor(r), packages)
	if err != nil {
		serviceError(w, err)
		return
	}
	writeJSON(w, 201, map[string]any{"packages": result})
}
func (h *Handler) claim(w http.ResponseWriter, r *http.Request) {
	var input struct {
		ClientClaimID string `json:"client_claim_id"`
	}
	if !readBody(w, r, 4096, &input) {
		return
	}
	result, err := h.messaging.Claim(r.Context(), actor(r), r.PathValue("void_id"), input.ClientClaimID)
	if err != nil {
		serviceError(w, err)
		return
	}
	writeJSON(w, 200, result)
}
func (h *Handler) createRequest(w http.ResponseWriter, r *http.Request) {
	var input messaging.CreateRequestInput
	if !readBody(w, r, 2<<20, &input) {
		return
	}
	result, err := h.messaging.CreateRequest(r.Context(), actor(r), input)
	if err != nil {
		serviceError(w, err)
		return
	}
	writeJSON(w, 201, result)
}
func pagination(w http.ResponseWriter, r *http.Request) (string, int, bool) {
	query, err := parseQuery(r)
	if err != nil {
		writeError(w, 400, "invalid_input", "Invalid pagination query.")
		return "", 0, false
	}
	after := query["after"]
	limit := 50
	if value, ok := query["limit"]; ok {
		n, err := strconv.Atoi(value)
		if err != nil || n < 1 || n > 100 || strconv.Itoa(n) != value {
			writeError(w, 400, "invalid_input", "Limit must be an integer from 1 to 100.")
			return "", 0, false
		}
		limit = n
	}
	return after, limit, true
}
func parseQuery(r *http.Request) (map[string]string, error) {
	values, err := url.ParseQuery(r.URL.RawQuery)
	if err != nil {
		return nil, err
	}
	result := make(map[string]string, len(values))
	for key, items := range values {
		if (key != "after" && key != "limit") || len(items) != 1 || items[0] == "" {
			return nil, errors.New("invalid query")
		}
		result[key] = items[0]
	}
	return result, nil
}
func (h *Handler) requests(w http.ResponseWriter, r *http.Request) {
	after, limit, ok := pagination(w, r)
	if !ok {
		return
	}
	result, err := h.messaging.ListRequests(r.Context(), actor(r), after, limit)
	if err != nil {
		serviceError(w, err)
		return
	}
	writeJSON(w, 200, result)
}
func (h *Handler) accept(w http.ResponseWriter, r *http.Request) {
	var input struct{}
	if !readBody(w, r, 4096, &input) {
		return
	}
	result, err := h.messaging.Accept(r.Context(), actor(r), r.PathValue("id"))
	if err != nil {
		serviceError(w, err)
		return
	}
	writeJSON(w, 200, result)
}
func (h *Handler) decline(w http.ResponseWriter, r *http.Request) {
	var input struct{}
	if !readBody(w, r, 4096, &input) {
		return
	}
	result, err := h.messaging.Decline(r.Context(), actor(r), r.PathValue("id"))
	if err != nil {
		serviceError(w, err)
		return
	}
	writeJSON(w, 200, result)
}
func (h *Handler) block(w http.ResponseWriter, r *http.Request) {
	var input struct{}
	if !readBody(w, r, 4096, &input) {
		return
	}
	if err := h.messaging.Block(r.Context(), actor(r), r.PathValue("void_id")); err != nil {
		serviceError(w, err)
		return
	}
	w.WriteHeader(204)
}
func (h *Handler) conversations(w http.ResponseWriter, r *http.Request) {
	after, limit, ok := pagination(w, r)
	if !ok {
		return
	}
	result, err := h.messaging.ListConversations(r.Context(), actor(r), after, limit)
	if err != nil {
		serviceError(w, err)
		return
	}
	writeJSON(w, 200, result)
}
func (h *Handler) send(w http.ResponseWriter, r *http.Request) {
	var input messaging.SendInput
	if !readBody(w, r, 2<<20, &input) {
		return
	}
	result, err := h.messaging.Send(r.Context(), actor(r), r.PathValue("id"), input)
	if err != nil {
		serviceError(w, err)
		return
	}
	writeJSON(w, 201, result)
}
func (h *Handler) sync(w http.ResponseWriter, r *http.Request) {
	after, limit, ok := pagination(w, r)
	if !ok {
		return
	}
	if after == "" {
		after = "0"
	}
	result, err := h.messaging.Sync(r.Context(), actor(r), after, limit)
	if err != nil {
		serviceError(w, err)
		return
	}
	writeJSON(w, 200, result)
}
func (h *Handler) ack(w http.ResponseWriter, r *http.Request) {
	var input struct {
		Events []messaging.Acknowledgment `json:"events"`
	}
	if !readBody(w, r, 32768, &input) {
		return
	}
	if len(input.Events) < 1 || len(input.Events) > 100 {
		writeError(w, 400, "invalid_input", "Acknowledge between 1 and 100 events.")
		return
	}
	if err := h.messaging.Ack(r.Context(), actor(r), input.Events); err != nil {
		serviceError(w, err)
		return
	}
	w.WriteHeader(204)
}

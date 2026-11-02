// Package httpapi exposes account initialization over a small JSON HTTP API.
package httpapi

import (
	"context"
	"encoding/base64"
	"encoding/json"
	"errors"
	"io"
	"mime"
	"net"
	"net/http"
	"strconv"
	"time"
	"unicode/utf8"

	"void-nothingness/internal/account"
)

const maxBodyBytes = 4096

type registrationService interface {
	Challenge([]byte) (account.Challenge, error)
	Register(context.Context, string, []byte) (account.Account, error)
}

type Options struct {
	RequestTimeout        time.Duration
	RatePerMinute         int
	RateBurst             int
	MaxRateClients        int
	MaxConcurrentRequests int
}

type handler struct {
	service registrationService
	ready   func(context.Context) error
	timeout time.Duration
	limiter *ipLimiter
	slots   chan struct{}
}

// New returns a handler with safe defaults for omitted options. RemoteAddr is
// the sole source of client IPs; forwarding headers never bypass rate limits.
func New(service registrationService, ready func(context.Context) error, opts Options) http.Handler {
	if opts.RequestTimeout <= 0 {
		opts.RequestTimeout = 5 * time.Second
	}
	if opts.RatePerMinute <= 0 {
		opts.RatePerMinute = 30
	}
	if opts.RateBurst <= 0 {
		opts.RateBurst = 10
	}
	if opts.MaxRateClients <= 0 {
		opts.MaxRateClients = 4096
	}
	if opts.MaxConcurrentRequests <= 0 {
		opts.MaxConcurrentRequests = 64
	}
	return &handler{
		service: service,
		ready:   ready,
		timeout: opts.RequestTimeout,
		limiter: newIPLimiter(opts.RatePerMinute, opts.RateBurst, opts.MaxRateClients),
		slots:   make(chan struct{}, opts.MaxConcurrentRequests),
	}
}

func (h *handler) ServeHTTP(w http.ResponseWriter, r *http.Request) {
	w.Header().Set("Cache-Control", "no-store")
	w.Header().Set("X-Content-Type-Options", "nosniff")
	ctx, cancel := context.WithTimeout(r.Context(), h.timeout)
	defer cancel()
	r = r.WithContext(ctx)

	var method string
	switch r.URL.Path {
	case "/healthz", "/readyz":
		method = http.MethodGet
	case "/dm9pZG5vdGhpbmduZXNz/challenges", "/dm9pZG5vdGhpbmduZXNz/accounts":
		method = http.MethodPost
	default:
		writeError(w, http.StatusNotFound, "not_found", "Endpoint not found.")
		return
	}
	if r.Method != method {
		w.Header().Set("Allow", method)
		writeError(w, http.StatusMethodNotAllowed, "method_not_allowed", "Method not allowed.")
		return
	}
	if r.URL.Path == "/healthz" {
		writeJSON(w, http.StatusOK, map[string]string{"status": "ok"})
		return
	}
	if method == http.MethodPost {
		if allowed, retry := h.limiter.allow(r.RemoteAddr); !allowed {
			w.Header().Set("Retry-After", strconv.Itoa(retry))
			writeError(w, http.StatusTooManyRequests, "rate_limited", "Too many requests.")
			return
		}
	}
	// Admission never queues or launches a goroutine. Liveness remains available
	// while database readiness and registration work share this bounded budget.
	select {
	case h.slots <- struct{}{}:
		defer func() { <-h.slots }()
	default:
		w.Header().Set("Retry-After", "1")
		writeError(w, http.StatusServiceUnavailable, "busy", "Service is busy. Retry later.")
		return
	}
	if err := ctx.Err(); err != nil {
		writeServiceError(w, err)
		return
	}
	if r.URL.Path == "/readyz" {
		if h.ready == nil || h.ready(ctx) != nil || ctx.Err() != nil {
			writeError(w, http.StatusServiceUnavailable, "not_ready", "Service is not ready.")
			return
		}
		writeJSON(w, http.StatusOK, map[string]string{"status": "ready"})
		return
	}
	if h.service == nil {
		writeServiceError(w, account.ErrUnavailable)
		return
	}
	if r.URL.Path == "/dm9pZG5vdGhpbmduZXNz/challenges" {
		fields, requestErr := readFields(w, r, []string{"auth_public_key"})
		if err := ctx.Err(); err != nil && (requestErr == nil || requestErr.status != http.StatusGatewayTimeout) {
			writeServiceError(w, err)
			return
		}
		if requestErr != nil {
			writeError(w, requestErr.status, requestErr.code, requestErr.message)
			return
		}
		publicKey, err := decodeBase64(fields["auth_public_key"])
		if err != nil {
			writeServiceError(w, account.ErrInvalidInput)
			return
		}
		result, err := h.service.Challenge(publicKey)
		if err != nil {
			writeServiceError(w, err)
			return
		}
		writeJSON(w, http.StatusCreated, result)
		return
	}
	fields, requestErr := readFields(w, r, []string{"challenge_id", "signature"})
	if err := ctx.Err(); err != nil && (requestErr == nil || requestErr.status != http.StatusGatewayTimeout) {
		writeServiceError(w, err)
		return
	}
	if requestErr != nil {
		writeError(w, requestErr.status, requestErr.code, requestErr.message)
		return
	}
	signature, err := decodeBase64(fields["signature"])
	if err != nil {
		writeServiceError(w, account.ErrInvalidInput)
		return
	}
	result, serviceErr := h.service.Register(ctx, fields["challenge_id"], signature)
	if serviceErr != nil {
		writeServiceError(w, serviceErr)
		return
	}
	writeJSON(w, http.StatusCreated, result)
}

type requestError struct {
	status        int
	code, message string
}

func readFields(w http.ResponseWriter, r *http.Request, required []string) (map[string]string, *requestError) {
	mediaType, _, err := mime.ParseMediaType(r.Header.Get("Content-Type"))
	if err != nil || mediaType != "application/json" {
		return nil, &requestError{http.StatusUnsupportedMediaType, "unsupported_media_type", "Content-Type must be application/json."}
	}
	if deadline, ok := r.Context().Deadline(); ok {
		// Context cancellation alone cannot unblock a stalled network body read.
		// Recorders and wrappers without deadline support retain the server's
		// connection read timeout instead.
		err := http.NewResponseController(w).SetReadDeadline(deadline)
		if err != nil && !errors.Is(err, http.ErrNotSupported) {
			return nil, &requestError{http.StatusServiceUnavailable, "unavailable", "Registration is temporarily unavailable."}
		}
	}
	r.Body = http.MaxBytesReader(w, r.Body, maxBodyBytes)
	defer r.Body.Close()
	body, err := io.ReadAll(r.Body)
	if err != nil {
		var tooLarge *http.MaxBytesError
		if errors.As(err, &tooLarge) {
			return nil, &requestError{http.StatusRequestEntityTooLarge, "body_too_large", "Request body exceeds 4096 bytes."}
		}
		var networkError net.Error
		if errors.As(err, &networkError) && networkError.Timeout() {
			// The socket timeout can cancel the server request context before
			// its deadline timer fires. Preserve the timeout response in either case.
			return nil, &requestError{http.StatusGatewayTimeout, "request_timeout", "Request timed out."}
		}
		return nil, invalidJSON()
	}
	if !utf8.Valid(body) {
		return nil, invalidJSON()
	}
	fields, err := parseStringObject(body, required)
	if err != nil {
		return nil, invalidJSON()
	}
	return fields, nil
}

func invalidJSON() *requestError {
	return &requestError{http.StatusBadRequest, "invalid_input", "Request must be a JSON object containing only the required string fields."}
}

func decodeBase64(encoded string) ([]byte, error) {
	decoded, err := base64.StdEncoding.Strict().DecodeString(encoded)
	// DecodeString tolerates CR/LF. Require canonical padded standard base64 so
	// inputs have one unambiguous wire representation.
	if err != nil || base64.StdEncoding.EncodeToString(decoded) != encoded {
		return nil, account.ErrInvalidInput
	}
	return decoded, nil
}

func writeServiceError(w http.ResponseWriter, err error) {
	switch {
	case errors.Is(err, context.DeadlineExceeded):
		writeError(w, http.StatusGatewayTimeout, "request_timeout", "Request timed out.")
	case errors.Is(err, context.Canceled):
		writeError(w, http.StatusRequestTimeout, "request_canceled", "Request was canceled.")
	case errors.Is(err, account.ErrInvalidInput):
		writeError(w, http.StatusBadRequest, "invalid_input", "Invalid registration input.")
	case errors.Is(err, account.ErrInvalidChallenge), errors.Is(err, account.ErrInvalidSignature):
		writeError(w, http.StatusUnauthorized, "invalid_proof", "Registration proof is invalid or expired.")
	case errors.Is(err, account.ErrDuplicateKey):
		writeError(w, http.StatusConflict, "key_registered", "Authentication key is already registered.")
	case errors.Is(err, account.ErrCapacity):
		w.Header().Set("Retry-After", "1")
		writeError(w, http.StatusTooManyRequests, "capacity_reached", "Registration capacity reached. Retry later.")
	case errors.Is(err, account.ErrUnavailable), errors.Is(err, account.ErrIDCollision):
		writeError(w, http.StatusServiceUnavailable, "unavailable", "Registration is temporarily unavailable.")
	default:
		writeError(w, http.StatusInternalServerError, "internal_error", "An internal error occurred.")
	}
}

func writeError(w http.ResponseWriter, status int, code, message string) {
	writeJSON(w, status, struct {
		Error struct {
			Code    string `json:"code"`
			Message string `json:"message"`
		} `json:"error"`
	}{Error: struct {
		Code    string `json:"code"`
		Message string `json:"message"`
	}{Code: code, Message: message}})
}

func writeJSON(w http.ResponseWriter, status int, value any) {
	w.Header().Set("Content-Type", "application/json")
	w.WriteHeader(status)
	_ = json.NewEncoder(w).Encode(value)
}

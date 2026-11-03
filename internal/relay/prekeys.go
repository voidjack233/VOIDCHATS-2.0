package relay

import (
	"context"
	"errors"
	"strconv"
	"time"

	"void-nothingness/internal/auth"
	"void-nothingness/internal/prekeys"
	"void-nothingness/internal/queue"
)

// Public-prekey operations share the bounded account budget, with a smaller
// eight-per-minute, four-operation burst that also survives socket replacement.
func (s *Server) allowPrekey(actor string) bool {
	s.mu.Lock()
	defer s.mu.Unlock()
	b, found := s.rates[actor]
	if !found {
		return false
	}
	now := time.Now()
	if b.prekeySeen.IsZero() {
		b.prekeyTokens = 4
	} else {
		b.prekeyTokens = min(4, b.prekeyTokens+max(0, now.Sub(b.prekeySeen).Seconds())*(8.0/60))
	}
	b.prekeySeen = now
	allowed := b.prekeyTokens >= 1
	if allowed {
		b.prekeyTokens--
	}
	s.rates[actor] = b
	return allowed
}

func (client *session) prekeyError(requestID string, err error) bool {
	code := "prekeys_unavailable"
	switch {
	case errors.Is(err, prekeys.ErrInvalid):
		code = "prekeys_invalid"
	case errors.Is(err, prekeys.ErrConflict):
		code = "prekeys_conflict"
	case errors.Is(err, prekeys.ErrCapacity):
		code = "prekeys_capacity"
	case errors.Is(err, prekeys.ErrExpired):
		code = "prekey_claim_expired"
	}
	// A timed-out store operation still gets a bounded, correlated response on
	// the live socket rather than inheriting the already-expired store deadline.
	return client.transmit(client.ctx, map[string]string{"type": "prekeys_error", "request_id": requestID, "code": code}) == nil
}

func (client *session) handlePrekeys(fields map[string]string) bool {
	publish := fields["type"] == "prekeys_publish"
	requestID := fields["request_id"]
	if publish {
		requestID = fields["publication_id"]
	}
	// Without a canonical ID the frontend cannot correlate an error to its
	// outstanding request, so retain the relay's ordinary invalid-frame behavior.
	if !queue.ValidEventID(requestID) {
		client.sendError(client.ctx, "invalid_frame")
		return false
	}
	s := client.server
	if !s.allowCommand(client.voidID) || !s.allowPrekey(client.voidID) {
		return client.prekeyError(requestID, prekeys.ErrCapacity)
	}
	if publish {
		if !exactFields(fields, "prekeys_publish", "publication_id", "payload_base64") {
			return client.prekeyError(requestID, prekeys.ErrInvalid)
		}
	} else if !exactFields(fields, "prekeys_claim", "request_id", "void_id") || !auth.ValidVoidID(fields["void_id"]) {
		return client.prekeyError(requestID, prekeys.ErrInvalid)
	}
	select {
	case s.slots <- struct{}{}:
		defer func() { <-s.slots }()
	default:
		return client.prekeyError(requestID, prekeys.ErrCapacity)
	}
	ctx, cancel := context.WithTimeout(client.ctx, s.opts.ForwardTimeout)
	defer cancel()
	if publish {
		if _, err := decodeBase64(fields["payload_base64"], prekeys.MaxUploadBytes); err != nil {
			return client.prekeyError(requestID, prekeys.ErrInvalid)
		}
	}
	if s.opts.Prekeys == nil {
		return client.prekeyError(requestID, prekeys.ErrUnavailable)
	}
	if publish {
		counts, err := s.opts.Prekeys.Publish(ctx, client.voidID, requestID, fields["payload_base64"])
		if err != nil {
			return client.prekeyError(requestID, err)
		}
		return client.transmit(ctx, map[string]string{"type": "prekeys_published", "publication_id": requestID, "ec_remaining": strconv.Itoa(counts.ECRemaining), "kyber_remaining": strconv.Itoa(counts.KyberRemaining)}) == nil
	}
	payload, err := s.opts.Prekeys.Claim(ctx, client.voidID, requestID, fields["void_id"])
	if err != nil {
		return client.prekeyError(requestID, err)
	}
	return client.transmit(ctx, map[string]string{"type": "prekey_bundle", "request_id": requestID, "void_id": fields["void_id"], "payload_base64": payload}) == nil
}

package relay

import (
	"context"
	"crypto/ed25519"
	"errors"
	"time"

	"void-nothingness/internal/account"
	"void-nothingness/internal/auth"
)

func (s *Server) withActive(ctx context.Context, ids []string, fn func() error) error {
	if s.opts.Accounts == nil {
		return fn()
	}
	return s.opts.Accounts.WithActive(ctx, ids, fn)
}

// The account lock remains held until the bounded write finishes. A response
// containing identity/prekeys or an online event cannot race past deletion.
func (client *session) activeTransmit(ctx context.Context, ids []string, value any) bool {
	err := client.server.withActive(ctx, ids, func() error { return client.transmit(ctx, value) })
	if err != nil {
		client.queueError(client.ctx, err)
	}
	return err == nil
}

func (client *session) accountStatus(ctx context.Context, fields map[string]string) bool {
	if !exactFields(fields, "account_status", "request_id", "void_id") ||
		!validRequestID(fields["request_id"]) || !auth.ValidVoidID(fields["void_id"]) {
		client.sendError(ctx, "invalid_frame")
		return false
	}
	if client.server.opts.Accounts == nil {
		client.sendError(ctx, "unavailable")
		return true
	}
	status, err := client.server.opts.Accounts.Status(ctx, client.voidID, fields["void_id"])
	if err != nil {
		client.queueError(ctx, err)
		return true
	}
	frame := map[string]string{"type": "account_status", "request_id": fields["request_id"], "void_id": status.VoidID, "state": status.State}
	if status.DeletedAt != nil {
		frame["deleted_at"] = status.DeletedAt.Format(time.RFC3339Nano)
	}
	return client.activeTransmit(ctx, []string{client.voidID}, frame)
}

// Deletion-only authentication is also accepted as the first exchange on a new
// socket. It never grants a relay session, allowing safe recovery after deletion.
func (client *session) deleteAccount(fields map[string]string) bool {
	if !exactFields(fields, "account_delete_begin", "void_id") || !auth.ValidVoidID(fields["void_id"]) {
		client.sendError(client.ctx, "invalid_frame")
		return false
	}
	id := fields["void_id"]
	if client.voidID != "" && client.voidID != id {
		client.sendError(client.ctx, "unauthorized")
		return false
	}
	client.deleting.Store(true)
	// Join any pending delivery write before beginning the deletion exchange.
	client.deliveryMu.Lock()
	defer client.deliveryMu.Unlock()
	s := client.server
	if s.opts.Accounts == nil {
		client.sendError(client.ctx, "unavailable")
		return false
	}
	if !s.allowCommand(id) {
		client.sendError(client.ctx, "rate_limited")
		return false
	}
	select {
	case s.slots <- struct{}{}:
		defer func() { <-s.slots }()
	default:
		client.sendError(client.ctx, "overloaded")
		return false
	}
	ctx, cancel := context.WithTimeout(client.ctx, 35*time.Second)
	defer cancel()
	challenge, err := s.opts.Accounts.Begin(ctx, id)
	if err != nil {
		code := "deletion_authorization_failed"
		if !errors.Is(err, account.ErrInvalidSignature) {
			code = "unavailable"
		}
		client.sendError(ctx, code)
		return false
	}
	if client.transmit(ctx, map[string]string{"type": "account_delete_challenge", "challenge_id": challenge.ChallengeID,
		"message": challenge.Message, "expires_at": challenge.ExpiresAt.Format(time.RFC3339Nano)}) != nil {
		return false
	}
	proofCtx, stop := context.WithDeadline(ctx, challenge.ExpiresAt)
	proof, err := client.read(proofCtx)
	stop()
	if err != nil {
		return false
	}
	if !exactFields(proof, "account_delete_confirm", "challenge_id", "confirmation", "signature") {
		client.sendError(ctx, "invalid_frame")
		return false
	}
	signature, decodeErr := decodeBase64(proof["signature"], ed25519.SignatureSize)
	if decodeErr != nil {
		client.sendError(ctx, "deletion_authorization_failed")
		return false
	}
	result, err := s.opts.Accounts.Confirm(ctx, challenge, proof["challenge_id"], proof["confirmation"], signature)
	if err != nil {
		code := "unavailable"
		if errors.Is(err, account.ErrInvalidChallenge) || errors.Is(err, account.ErrInvalidSignature) {
			code = "deletion_authorization_failed"
		}
		client.sendError(client.ctx, code)
		return false
	}
	// Commit already succeeded. Revoke another local session even if this socket
	// disappears before acknowledgment. Other processes enforce durable state.
	s.mu.Lock()
	other := s.active[id]
	if other != client {
		delete(s.active, id)
	}
	s.mu.Unlock()
	if other != nil && other != client {
		other.stop()
	}
	_ = client.transmit(client.ctx, map[string]string{"type": "account_deleted", "void_id": id,
		"deleted_at": result.DeletedAt.Format(time.RFC3339Nano)})
	return false
}

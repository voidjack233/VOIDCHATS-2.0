package relay

import (
	"context"
	"encoding/base64"
	"errors"
	"time"

	"void-nothingness/internal/auth"
	"void-nothingness/internal/queue"
)

// Account budgets survive socket replacement. A full map refuses new admission
// instead of evicting active account budgets on each reconnect.
func (s *Server) allowCommand(actor string) bool {
	s.mu.Lock()
	defer s.mu.Unlock()
	now := time.Now()
	b, found := s.rates[actor]
	if !found {
		if len(s.rates) >= 4096 {
			for id, old := range s.rates {
				if now.Sub(old.seen) >= 10*time.Minute {
					delete(s.rates, id)
				}
			}
			if len(s.rates) >= 4096 {
				return false
			}
		}
		b.tokens = 40
	}
	b.tokens = min(40, b.tokens+max(0, now.Sub(b.seen).Seconds())*2)
	b.seen = now
	allowed := b.tokens >= 1
	if allowed {
		b.tokens--
	}
	s.rates[actor] = b
	return allowed
}

func (client *session) queueError(ctx context.Context, err error) {
	code := "unavailable"
	switch {
	case errors.Is(err, queue.ErrInvalid):
		code = "invalid_frame"
	case errors.Is(err, queue.ErrNotFound):
		code = "not_found"
	case errors.Is(err, queue.ErrDuplicate):
		code = "duplicate_event"
	case errors.Is(err, queue.ErrCapacity):
		code = "queue_full"
	}
	client.sendError(ctx, code)
}

func (client *session) handleCommand(fields map[string]string) bool {
	s := client.server
	if !s.allowCommand(client.voidID) {
		client.sendError(client.ctx, "rate_limited")
		return true
	}
	select {
	case s.slots <- struct{}{}:
		defer func() { <-s.slots }()
	default:
		client.sendError(client.ctx, "overloaded")
		return true
	}
	ctx, cancel := context.WithTimeout(client.ctx, s.opts.ForwardTimeout)
	defer cancel()
	switch fields["type"] {
	case "send":
		if !exactFields(fields, "send", "event_id", "to_void_id", "payload_base64") || !queue.ValidEventID(fields["event_id"]) || !auth.ValidVoidID(fields["to_void_id"]) {
			break
		}
		payload, err := decodeBase64(fields["payload_base64"], s.opts.MaxPayloadBytes)
		if err != nil {
			break
		}
		event, err := s.opts.Queue.Send(ctx, client.voidID, fields["event_id"], fields["to_void_id"], payload)
		if err != nil {
			client.queueError(ctx, err)
			return true
		}
		// Persistence already committed. Wake the recipient even if the sender
		// disconnects or its acceptance frame cannot be written.
		defer s.wakeRecipient(event.RecipientVoidID)
		return client.transmit(ctx, map[string]string{"type": "accepted", "event_id": event.EventID, "created_at": event.CreatedAt.Format(time.RFC3339Nano), "expires_at": event.ExpiresAt.Format(time.RFC3339Nano)}) == nil
	case "ack":
		if !exactFields(fields, "ack", "sender_void_id", "event_id") || !auth.ValidVoidID(fields["sender_void_id"]) || !queue.ValidEventID(fields["event_id"]) {
			break
		}
		if err := s.opts.Queue.Ack(ctx, client.voidID, fields["sender_void_id"], fields["event_id"]); err != nil {
			client.queueError(ctx, err)
			return true
		}
		client.deliveryMu.Lock()
		if client.inFlightSender == fields["sender_void_id"] && client.inFlightID == fields["event_id"] {
			client.inFlightSender, client.inFlightID = "", ""
		}
		client.deliveryMu.Unlock()
		defer s.wakeRecipient(client.voidID)
		return client.transmit(ctx, map[string]string{"type": "ack_ok", "sender_void_id": fields["sender_void_id"], "event_id": fields["event_id"]}) == nil
	case "lookup":
		if !exactFields(fields, "lookup", "void_id") {
			break
		}
		person, err := s.opts.Queue.Lookup(ctx, client.voidID, fields["void_id"])
		if err != nil {
			client.queueError(ctx, err)
			return true
		}
		return client.transmit(ctx, map[string]string{"type": "person", "void_id": person.VoidID, "auth_public_key": person.AuthPublicKey}) == nil
	case "relay_online":
		if !exactFields(fields, "relay_online", "exchange_id", "to_void_id", "payload_base64") || !validRequestID(fields["exchange_id"]) || !auth.ValidVoidID(fields["to_void_id"]) {
			break
		}
		if _, err := decodeBase64(fields["payload_base64"], s.opts.MaxPayloadBytes); err != nil {
			break
		}
		// Public MLS initialization objects use only the authenticated online
		// relay. No object inventories or initialization state are persisted.
		status := client.forward(fields)
		return client.transmit(ctx, map[string]string{"type": "online_result", "exchange_id": fields["exchange_id"], "status": status}) == nil
	}
	client.sendError(ctx, "invalid_frame")
	return false
}

func (s *Server) wakeRecipient(actor string) {
	s.mu.Lock()
	client := s.active[actor]
	if client != nil {
		select {
		case client.wake <- struct{}{}:
		default:
		}
	}
	s.mu.Unlock()
}

// One event is in flight per connection, limiting JSON payload memory. A frame
// write never removes ciphertext. The next event is released by authenticated
// ACK or expiry; reconnect starts from the remaining queue without a cursor.
// There is deliberately no resend timer or persistent retry/deduplication state.
func (client *session) deliveryLoop() {
	ticker := time.NewTicker(time.Second)
	defer ticker.Stop()
	for {
		if !client.server.isCurrent(client) || !client.deliverOne() {
			return
		}
		select {
		case <-client.ctx.Done():
			return
		case <-client.wake:
		case <-ticker.C:
		}
	}
}

func (client *session) deliverOne() bool {
	s := client.server
	select {
	case s.slots <- struct{}{}:
		defer func() { <-s.slots }()
	default:
		return true
	}
	client.deliveryMu.Lock()
	defer client.deliveryMu.Unlock()
	if client.inFlightID != "" && time.Now().Before(client.inFlightExpiry) {
		return true
	}
	ctx, cancel := context.WithTimeout(client.ctx, s.opts.ForwardTimeout)
	defer cancel()
	rows, err := s.opts.Queue.Pending(ctx, client.voidID, 1)
	if err != nil {
		client.stop()
		return false
	}
	if len(rows) == 0 {
		client.inFlightSender, client.inFlightID = "", ""
		return true
	}
	event := rows[0]
	client.inFlightSender, client.inFlightID, client.inFlightExpiry = event.SenderVoidID, event.EventID, event.ExpiresAt
	eventCtx, expireCancel := context.WithDeadline(ctx, event.ExpiresAt)
	err = client.transmit(eventCtx, map[string]string{"type": "event", "event_id": event.EventID, "from_void_id": event.SenderVoidID, "payload_base64": base64.StdEncoding.EncodeToString(event.Payload), "created_at": event.CreatedAt.Format(time.RFC3339Nano), "expires_at": event.ExpiresAt.Format(time.RFC3339Nano)})
	expireCancel()
	if err != nil {
		client.stop()
		return false
	}
	return true
}

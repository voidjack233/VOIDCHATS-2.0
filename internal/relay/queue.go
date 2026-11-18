package relay

import (
	"context"
	"encoding/base64"
	"errors"
	"strconv"
	"time"

	"void-nothingness/internal/account"
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
	case errors.Is(err, account.ErrDeleted):
		code = "account_deleted"
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
	if fields["type"] == "account_delete_begin" {
		return client.deleteAccount(fields)
	}
	if fields["type"] != "protocol" && !client.messagingAllowed() {
		client.updateRequired(client.ctx)
		return false
	}
	check, stop := context.WithTimeout(client.ctx, client.server.opts.ForwardTimeout)
	err := client.server.withActive(check, []string{client.voidID}, func() error { return nil })
	stop()
	if err != nil {
		client.queueError(client.ctx, err)
		return false
	}

	if fields["type"] == "prekeys_publish" || fields["type"] == "prekeys_claim" {
		return client.handlePrekeys(fields)
	}
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
	case "protocol":
		if !exactFields(fields, "protocol", "version") {
			break
		}
		version, ok := parseProtocolVersion(fields["version"])
		if !ok {
			break
		}
		if version < s.opts.MinProtocolVersion {
			client.updateRequired(ctx)
			return false
		}
		// Serialize negotiation with delivery so the confirmation precedes any
		// event that requires the new version. Downgrading could invalidate an
		// in-flight frame; reconnect instead, starting at version 1.
		client.deliveryMu.Lock()
		defer client.deliveryMu.Unlock()
		if version < client.protocolVersion {
			break
		}
		if !client.activeTransmit(ctx, []string{client.voidID}, map[string]string{"type": "protocol_ok", "version": fields["version"]}) {
			return false
		}
		client.protocolVersion = version
		client.protocolConfirmed.Store(true)
		s.wakeRecipient(client.voidID)
		return true
	case "account_status":
		return client.accountStatus(ctx, fields)
	case "send", "send_versioned":
		version := 1
		if fields["type"] == "send_versioned" {
			var ok bool
			version, ok = parseProtocolVersion(fields["min_protocol_version"])
			if !ok || !exactFields(fields, "send_versioned", "event_id", "to_void_id", "payload_base64", "min_protocol_version") {
				break
			}
		} else if !exactFields(fields, "send", "event_id", "to_void_id", "payload_base64") {
			break
		}
		if !queue.ValidEventID(fields["event_id"]) || !auth.ValidVoidID(fields["to_void_id"]) {
			break
		}
		payload, err := decodeBase64(fields["payload_base64"], s.opts.MaxPayloadBytes)
		if err != nil {
			break
		}
		event, err := s.opts.Queue.SendVersioned(ctx, client.voidID, fields["event_id"], fields["to_void_id"], payload, version)
		if err != nil {
			client.queueError(ctx, err)
			return true
		}
		// Persistence already committed. Wake the recipient even if the sender
		// disconnects or its acceptance frame cannot be written.
		defer s.wakeRecipient(event.RecipientVoidID)
		return client.activeTransmit(ctx, []string{client.voidID, event.RecipientVoidID}, map[string]string{"type": "accepted", "event_id": event.EventID, "created_at": event.CreatedAt.Format(time.RFC3339Nano), "expires_at": event.ExpiresAt.Format(time.RFC3339Nano)})
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
		return client.activeTransmit(ctx, []string{client.voidID}, map[string]string{"type": "ack_ok", "sender_void_id": fields["sender_void_id"], "event_id": fields["event_id"]})
	case "lookup":
		if !exactFields(fields, "lookup", "void_id") {
			break
		}
		person, err := s.opts.Queue.Lookup(ctx, client.voidID, fields["void_id"])
		if err != nil {
			client.queueError(ctx, err)
			return true
		}
		return client.activeTransmit(ctx, []string{client.voidID, person.VoidID}, map[string]string{"type": "person", "void_id": person.VoidID, "auth_public_key": person.AuthPublicKey})
	case "relay_online":
		if !exactFields(fields, "relay_online", "exchange_id", "to_void_id", "payload_base64") || !validRequestID(fields["exchange_id"]) || !auth.ValidVoidID(fields["to_void_id"]) {
			break
		}
		if _, err := decodeBase64(fields["payload_base64"], s.opts.MaxPayloadBytes); err != nil {
			break
		}
		// Public MLS initialization objects use only the authenticated online
		// relay. No object inventories or initialization state are persisted.
		err := client.server.withActive(ctx, []string{client.voidID, fields["to_void_id"]}, func() error {
			status := client.forward(fields)
			return client.transmit(ctx, map[string]string{"type": "online_result", "exchange_id": fields["exchange_id"], "status": status})
		})
		if err != nil {
			client.queueError(client.ctx, err)
		}
		return err == nil
	}
	client.sendError(ctx, "invalid_frame")
	return false
}

// The minimum is immutable for a server lifetime. Confirmation is connection-local
// and only becomes true after protocol_ok is written; it never decreases.
func (client *session) messagingAllowed() bool {
	return client.server.opts.MinProtocolVersion <= 1 || client.protocolConfirmed.Load()
}

func (client *session) updateRequired(ctx context.Context) {
	_ = client.transmit(ctx, map[string]string{"type": "error", "code": "update_required", "min_protocol_version": strconv.Itoa(client.server.opts.MinProtocolVersion)})
}

func parseProtocolVersion(value string) (int, bool) {
	version, err := strconv.ParseInt(value, 10, 32)
	if err != nil || !queue.ValidProtocolVersion(int(version)) || strconv.FormatInt(version, 10) != value {
		return 0, false
	}
	return int(version), true
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
	client.deliveryMu.Lock()
	defer client.deliveryMu.Unlock()
	if client.deleting.Load() || !client.messagingAllowed() {
		return true
	}
	select {
	case s.slots <- struct{}{}:
		defer func() { <-s.slots }()
	default:
		return true
	}
	check, stop := context.WithTimeout(client.ctx, s.opts.ForwardTimeout)
	err := s.withActive(check, []string{client.voidID}, func() error { return nil })
	stop()
	if err != nil {
		client.stop()
		return false
	}
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
	// Read only the existing queue head. Skipping incompatible ciphertext can
	// advance a Signal session past withheld messages. Leave it stored and
	// hold the entire recipient queue until support increases or it expires.
	if event.MinProtocolVersion > client.protocolVersion {
		client.inFlightSender, client.inFlightID = "", ""
		return true
	}
	client.inFlightSender, client.inFlightID, client.inFlightExpiry = event.SenderVoidID, event.EventID, event.ExpiresAt
	frame := map[string]string{"type": "event", "event_id": event.EventID, "from_void_id": event.SenderVoidID, "payload_base64": base64.StdEncoding.EncodeToString(event.Payload), "created_at": event.CreatedAt.Format(time.RFC3339Nano), "expires_at": event.ExpiresAt.Format(time.RFC3339Nano)}
	if event.MinProtocolVersion > 1 {
		frame["type"] = "event_versioned"
		frame["min_protocol_version"] = strconv.Itoa(event.MinProtocolVersion)
	}
	eventCtx, expireCancel := context.WithDeadline(ctx, event.ExpiresAt)
	err = s.withActive(eventCtx, []string{client.voidID, event.SenderVoidID}, func() error {
		return client.transmit(eventCtx, frame)
	})
	expireCancel()
	if errors.Is(err, account.ErrDeleted) {
		// Deletion can remove an event after Pending read it. Silently discard
		// that stale delivery; an unsolicited generic error could otherwise be
		// mistaken for a reply to an unrelated request on this recipient socket.
		client.inFlightSender, client.inFlightID = "", ""
		if s.withActive(ctx, []string{client.voidID}, func() error { return nil }) == nil {
			return true
		}
	}
	if err != nil {
		client.stop()
		return false
	}
	return true
}

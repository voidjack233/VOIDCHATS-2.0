package messaging

import (
	"context"
	"crypto/rand"
	"crypto/sha256"
	"encoding/base64"
	"encoding/hex"
	"encoding/json"
	"errors"
	"strconv"
	"strings"
	"time"
)

type Service struct {
	store Store
	now   func() time.Time
}

func NewService(store Store) (*Service, error) {
	if store == nil {
		return nil, errors.New("messaging store required")
	}
	return &Service{store: store, now: time.Now}, nil
}
func problem(status int, code, message string) error { return &Error{status, code, message} }
func invalid() error                                 { return problem(400, "invalid_input", "Invalid messaging input.") }
func missing() error                                 { return problem(404, "not_found", "Resource unavailable.") }
func conflict() error {
	return problem(409, "idempotency_conflict", "Stable client ID was reused with different content.")
}
func capacity() error {
	return problem(503, "messaging_capacity", "Messaging storage or pending delivery capacity reached.")
}
func (s *Service) run(ctx context.Context, fn func(Tx) error) error {
	err := s.store.Run(ctx, fn)
	if err == nil {
		return nil
	}
	var app *Error
	if errors.As(err, &app) {
		return app
	}
	if ctx.Err() != nil {
		return ctx.Err()
	}
	return problem(503, "messaging_unavailable", "Messaging temporarily unavailable.")
}
func validID(s string) bool {
	n, e := strconv.ParseInt(s, 10, 64)
	return e == nil && n > 0 && strconv.FormatInt(n, 10) == s
}
func ValidUUID(s string) bool {
	if len(s) != 36 {
		return false
	}
	for i, c := range s {
		if i == 8 || i == 13 || i == 18 || i == 23 {
			if c != '-' {
				return false
			}
		} else if !(c >= '0' && c <= '9' || c >= 'a' && c <= 'f') {
			return false
		}
	}
	return s != "00000000-0000-0000-0000-000000000000"
}
func uuid() (string, error) {
	var b [16]byte
	if _, err := rand.Read(b[:]); err != nil {
		return "", err
	}
	b[6] = b[6]&15 | 64
	b[8] = b[8]&63 | 128
	v := hex.EncodeToString(b[:])
	return v[:8] + "-" + v[8:12] + "-" + v[12:16] + "-" + v[16:20] + "-" + v[20:], nil
}
func decode(s string, max int) ([]byte, error) {
	if len(s) > base64.StdEncoding.EncodedLen(max) {
		return nil, problem(413, "payload_too_large", "Decoded opaque payload exceeds its limit.")
	}
	b, e := base64.StdEncoding.Strict().DecodeString(s)
	if e != nil || len(b) == 0 || base64.StdEncoding.EncodeToString(b) != s {
		return nil, invalid()
	}
	if len(b) > max {
		return nil, problem(413, "payload_too_large", "Decoded opaque payload exceeds its limit.")
	}
	return b, nil
}
func owned(tx Tx, actor string) error {
	_, ok, e := tx.Person(actor)
	if e != nil {
		return e
	}
	if !ok {
		return problem(401, "invalid_session", "Registered account unavailable.")
	}
	return nil
}
func available(tx Tx, actor, target string) error {
	if !validID(target) || actor == target {
		return invalid()
	}
	if err := owned(tx, actor); err != nil {
		return err
	}
	_, ok, e := tx.Person(target)
	if e != nil {
		return e
	}
	if !ok {
		return missing()
	}
	blocked, e := tx.Blocked(actor, target)
	if e != nil {
		return e
	}
	if blocked {
		return missing()
	}
	return nil
}
func (s *Service) Lookup(ctx context.Context, actor, target string) (out Person, err error) {
	if !validID(actor) || !validID(target) {
		return out, invalid()
	}
	err = s.run(ctx, func(tx Tx) error {
		if actor != target {
			if e := available(tx, actor, target); e != nil {
				return e
			}
		} else if e := owned(tx, actor); e != nil {
			return e
		}
		var ok bool
		var e error
		out, ok, e = tx.Person(target)
		if e != nil {
			return e
		}
		if !ok {
			return missing()
		}
		return nil
	})
	return
}
func (s *Service) Publish(ctx context.Context, actor string, packages [][]byte) (out []PublishedPackage, err error) {
	if !validID(actor) || len(packages) < 1 || len(packages) > MaxPackageBatch {
		return nil, invalid()
	}
	total := 0
	for _, p := range packages {
		if len(p) == 0 {
			return nil, invalid()
		}
		total += len(p)
	}
	if total > MaxOpaqueBytes {
		return nil, problem(413, "payload_too_large", "Total decoded package bytes exceed one MiB.")
	}
	err = s.run(ctx, func(tx Tx) error {
		if e := owned(tx, actor); e != nil {
			return e
		}
		out = make([]PublishedPackage, 0, len(packages))
		now := s.now().UTC()
		for _, p := range packages {
			sum := sha256.Sum256(p)
			fp := hex.EncodeToString(sum[:])
			row, ok, e := tx.PackageByFingerprint(actor, fp)
			if e != nil {
				return e
			}
			if !ok {
				count, storedBytes, e := tx.PackageCount(actor)
				if e != nil {
					return e
				}
				if count >= MaxPackagesPerAccount || storedBytes+base64.StdEncoding.EncodedLen(len(p)) > MaxPackageBytes {
					return capacity()
				}
				id, e := uuid()
				if e != nil {
					return e
				}
				row = PackageRow{ID: id, Owner: actor, Fingerprint: fp, Data: base64.StdEncoding.EncodeToString(p), ExpiresAt: now.Add(PackageTTL)}
				if e = tx.SavePackage(row); e != nil {
					return e
				}
			}
			state := "available"
			if row.ClaimedBy != "" {
				state = "claimed"
			} else if !now.Before(row.ExpiresAt) {
				state = "expired"
			}
			out = append(out, PublishedPackage{row.ID, state, row.ExpiresAt})
		}
		return nil
	})
	return
}
func (s *Service) Claim(ctx context.Context, actor, target, clientClaimID string) (out Claim, err error) {
	if !validID(actor) || !validID(target) || actor == target || !ValidUUID(clientClaimID) {
		return out, invalid()
	}
	err = s.run(ctx, func(tx Tx) error {
		if e := available(tx, actor, target); e != nil {
			return e
		}
		p, ok, e := tx.ClaimedPackage(actor, clientClaimID)
		if e != nil {
			return e
		}
		if ok && p.Owner != target {
			return conflict()
		}
		if !ok {
			p, ok, e = tx.AvailablePackage(target, s.now().UTC())
			if e != nil {
				return e
			}
			if !ok {
				return problem(409, "key_package_unavailable", "Recipient must publish a fresh public KeyPackage.")
			}
			p.ClaimedBy = actor
			p.ClaimID = clientClaimID
			if e = tx.SavePackage(p); e != nil {
				return e
			}
		}
		person, _, e := tx.Person(target)
		if e != nil {
			return e
		}
		out = Claim{clientClaimID, p.ID, p.Data, person}
		return nil
	})
	return
}
func (s *Service) emit(tx Tx, owner, kind string, payload any) error {
	total, pending, totalBytes, pendingBytes, e := tx.EventCounts(owner)
	if e != nil {
		return e
	}
	reserved, e := tx.ReceiptReservations(owner)
	if e != nil {
		return e
	}
	if pending+reserved >= MaxPendingEvents || total+reserved >= MaxRetainedEvents {
		return capacity()
	}
	id, e := uuid()
	if e != nil {
		return e
	}
	cursor, e := tx.NextCursor(owner)
	if e != nil {
		return e
	}
	b, e := json.Marshal(payload)
	if e != nil {
		return e
	}
	row := EventRow{Owner: owner, Event: Event{EventID: id, Cursor: strconv.FormatInt(cursor, 10), Type: kind, Payload: b, CreatedAt: s.now().UTC()}}
	encoded, e := json.Marshal(row.Event)
	if e != nil {
		return e
	}
	n := len(encoded)
	if totalBytes+reserved*ReceiptReserveBytes+n > MaxRetainedEventBytes || pendingBytes+reserved*ReceiptReserveBytes+n > MaxPendingEventBytes {
		return capacity()
	}
	return tx.SaveEvent(row)
}

func reserveReceipt(tx Tx, owner string) error {
	total, pending, totalBytes, pendingBytes, e := tx.EventCounts(owner)
	if e != nil {
		return e
	}
	n, e := tx.ReceiptReservations(owner)
	if e != nil {
		return e
	}
	n++
	if total+n > MaxRetainedEvents || pending+n > MaxPendingEvents || totalBytes+n*ReceiptReserveBytes > MaxRetainedEventBytes || pendingBytes+n*ReceiptReserveBytes > MaxPendingEventBytes {
		return capacity()
	}
	return tx.ReserveReceipt(owner, 1)
}
func cancelEvent(tx Tx, row EventRow) error {
	if row.Outcome != "" {
		return nil
	}
	if row.Event.Type == "message.created" {
		var m Message
		if e := json.Unmarshal(row.Event.Payload, &m); e != nil {
			return e
		}
		if e := tx.ReserveReceipt(m.SenderVoidID, -1); e != nil {
			return e
		}
	}
	row.Outcome = "cancelled"
	return tx.SaveEvent(row)
}
func (s *Service) CreateRequest(ctx context.Context, actor string, in CreateRequestInput) (out Request, err error) {
	if !validID(actor) || !validID(in.ToVoidID) || actor == in.ToVoidID || !ValidUUID(in.ClientRequestID) || !ValidUUID(in.KeyPackageID) {
		return out, invalid()
	}
	group, e := decode(in.GroupIDBase64, 256)
	if e != nil {
		return out, e
	}
	welcome, e := decode(in.WelcomeBase64, MaxOpaqueBytes)
	if e != nil {
		return out, e
	}
	if len(group)+len(welcome) > MaxOpaqueBytes {
		return out, problem(413, "payload_too_large", "Total decoded request bytes exceed one MiB.")
	}
	err = s.run(ctx, func(tx Tx) error {
		if e := owned(tx, actor); e != nil {
			return e
		}
		old, ok, e := tx.RequestByClient(actor, in.ClientRequestID)
		if e != nil {
			return e
		}
		if ok {
			if old.Recipient.VoidID != in.ToVoidID || old.KeyPackageID != in.KeyPackageID || old.GroupIDBase64 != in.GroupIDBase64 || old.WelcomeBase64 != in.WelcomeBase64 {
				return conflict()
			}
			out = old
			return nil
		}
		if e := available(tx, actor, in.ToVoidID); e != nil {
			return e
		}
		if _, exists, e := tx.ActivePair(actor, in.ToVoidID); e != nil {
			return e
		} else if exists {
			return problem(409, "conversation_exists", "An active conversation or pending request already exists for this pair.")
		}
		p, ok, e := tx.Package(in.KeyPackageID)
		if e != nil {
			return e
		}
		if !ok || p.Owner != in.ToVoidID || p.ClaimedBy != actor {
			return problem(409, "key_package_claim_required", "Claim the recipient KeyPackage before creating a request.")
		}
		if p.RequestID != "" {
			return problem(409, "key_package_used", "KeyPackage is already bound to a request; claim a fresh package.")
		}
		n, e := tx.RequestCount(actor)
		if e != nil {
			return e
		}
		m, e := tx.RequestCount(in.ToVoidID)
		if e != nil {
			return e
		}
		if n >= MaxRequestsPerAccount || m >= MaxRequestsPerAccount {
			return capacity()
		}
		sender, _, e := tx.Person(actor)
		if e != nil {
			return e
		}
		recipient, _, e := tx.Person(in.ToVoidID)
		if e != nil {
			return e
		}
		id, e := uuid()
		if e != nil {
			return e
		}
		conv, e := uuid()
		if e != nil {
			return e
		}
		now := s.now().UTC()
		out = Request{RequestID: id, ConversationID: conv, ClientRequestID: in.ClientRequestID, KeyPackageID: in.KeyPackageID, Status: "pending", Sender: sender, Recipient: recipient, GroupIDBase64: in.GroupIDBase64, WelcomeBase64: in.WelcomeBase64, CreatedAt: now, UpdatedAt: now}
		if e = tx.SaveRequest(out); e != nil {
			return e
		}
		p.RequestID = out.RequestID
		if e = tx.SavePackage(p); e != nil {
			return e
		}
		if e = s.emit(tx, actor, "request.created", out); e != nil {
			return e
		}
		return s.emit(tx, in.ToVoidID, "request.created", out)
	})
	return
}
func pageArgs(actor, after string, limit int) error {
	if !validID(actor) || after != "" && !ValidUUID(after) || limit < 1 || limit > 100 {
		return invalid()
	}
	return nil
}
func (s *Service) ListRequests(ctx context.Context, actor, after string, limit int) (out RequestPage, err error) {
	if e := pageArgs(actor, after, limit); e != nil {
		return out, e
	}
	out.Requests = []Request{}
	out.NextCursor = after
	err = s.run(ctx, func(tx Tx) error {
		if e := owned(tx, actor); e != nil {
			return e
		}
		rows, e := tx.Requests(actor, after, limit+1)
		if e != nil {
			return e
		}
		size := 0
		for i, r := range rows {
			if i >= limit {
				out.HasMore = true
				break
			}
			target := r.Sender.VoidID
			if target == actor {
				target = r.Recipient.VoidID
			}
			blocked, e := tx.Blocked(actor, target)
			if e != nil {
				return e
			}
			if blocked {
				out.NextCursor = r.RequestID
				continue
			}
			b, e := json.Marshal(r)
			if e != nil {
				return e
			}
			if size+len(b)+512 > MaxSyncBytes && i > 0 {
				out.HasMore = true
				break
			}
			size += len(b) + 1
			out.Requests = append(out.Requests, r)
			out.NextCursor = r.RequestID
		}
		return nil
	})
	return
}

type Accepted struct {
	Request      Request      `json:"request"`
	Conversation Conversation `json:"conversation"`
}

func (s *Service) transition(ctx context.Context, actor, id, want string) (out Request, err error) {
	if !validID(actor) || !ValidUUID(id) {
		return out, invalid()
	}
	err = s.run(ctx, func(tx Tx) error {
		if e := owned(tx, actor); e != nil {
			return e
		}
		r, ok, e := tx.Request(id)
		if e != nil {
			return e
		}
		if !ok {
			return missing()
		}
		if r.Recipient.VoidID != actor {
			if r.Sender.VoidID == actor {
				return problem(403, "forbidden", "Only the invitation recipient can accept or decline it.")
			}
			return missing()
		}
		if r.Status == want {
			out = r
			return nil
		}
		if r.Status != "pending" {
			return problem(409, "request_not_pending", "Request is no longer pending.")
		}
		if e := available(tx, actor, r.Sender.VoidID); e != nil {
			return e
		}
		r.Status = want
		r.UpdatedAt = s.now().UTC()
		if e = tx.SaveRequest(r); e != nil {
			return e
		}
		var payload any = r
		if want == "accepted" {
			c := Conversation{ConversationID: r.ConversationID, RequestID: r.RequestID, Participants: []Person{r.Sender, r.Recipient}, GroupIDBase64: r.GroupIDBase64, CreatedAt: r.UpdatedAt}
			if e = tx.SaveConversation(ConversationRow{Conversation: c}); e != nil {
				return e
			}
			payload = Accepted{r, c}
		}
		out = r
		if e = s.emit(tx, r.Sender.VoidID, "request."+want, payload); e != nil {
			return e
		}
		return s.emit(tx, r.Recipient.VoidID, "request."+want, payload)
	})
	return
}
func (s *Service) Accept(ctx context.Context, actor, id string) (Request, error) {
	return s.transition(ctx, actor, id, "accepted")
}
func (s *Service) Decline(ctx context.Context, actor, id string) (Request, error) {
	return s.transition(ctx, actor, id, "declined")
}
func peer(c Conversation, actor string) (string, bool) {
	if len(c.Participants) != 2 {
		return "", false
	}
	a, b := c.Participants[0].VoidID, c.Participants[1].VoidID
	if a == actor {
		return b, true
	}
	if b == actor {
		return a, true
	}
	return "", false
}
func eventPeer(row EventRow) string {
	if strings.HasPrefix(row.Event.Type, "request.") {
		var r Request
		if row.Event.Type == "request.accepted" {
			var a Accepted
			if json.Unmarshal(row.Event.Payload, &a) != nil {
				return ""
			}
			r = a.Request
		} else if json.Unmarshal(row.Event.Payload, &r) != nil {
			return ""
		}
		if r.Sender.VoidID == row.Owner {
			return r.Recipient.VoidID
		}
		if r.Recipient.VoidID == row.Owner {
			return r.Sender.VoidID
		}
	}
	if row.Event.Type == "message.created" {
		var m Message
		if json.Unmarshal(row.Event.Payload, &m) == nil {
			if m.SenderVoidID == row.Owner {
				return m.RecipientVoidID
			}
			return m.SenderVoidID
		}
	}
	if row.Event.Type == "message.delivered" || row.Event.Type == "message.rejected" {
		var r Receipt
		if json.Unmarshal(row.Event.Payload, &r) == nil {
			return r.RecipientVoidID
		}
	}
	return ""
}

type BlockState struct {
	VoidID    string    `json:"void_id"`
	Blocked   bool      `json:"blocked"`
	CreatedAt time.Time `json:"created_at"`
}

func (s *Service) Block(ctx context.Context, actor, target string) error {
	if !validID(actor) || !validID(target) || actor == target {
		return invalid()
	}
	return s.run(ctx, func(tx Tx) error {
		if e := owned(tx, actor); e != nil {
			return e
		}
		if _, ok, e := tx.Person(target); e != nil {
			return e
		} else if !ok {
			return missing()
		}
		now := s.now().UTC()
		inserted, e := tx.SaveBlock(actor, target, now)
		if e != nil {
			return e
		}
		r, ok, e := tx.ActivePair(actor, target)
		if e != nil {
			return e
		}
		if ok && r.Status == "pending" {
			r.Status = "cancelled"
			r.UpdatedAt = now
			if e = tx.SaveRequest(r); e != nil {
				return e
			}
		}
		for _, owner := range []string{actor, target} {
			rows, e := tx.Events(owner, 0, MaxPendingEvents+1)
			if e != nil {
				return e
			}
			for _, row := range rows {
				other := target
				if owner == target {
					other = actor
				}
				if eventPeer(row) == other {
					if e = cancelEvent(tx, row); e != nil {
						return e
					}
				}
			}
		}
		if !inserted {
			return nil
		}
		e = s.emit(tx, actor, "block.updated", BlockState{target, true, now})
		var app *Error
		if errors.As(e, &app) && app.Code == "messaging_capacity" {
			return nil
		}
		return e
	})
}
func (s *Service) ListConversations(ctx context.Context, actor, after string, limit int) (out ConversationPage, err error) {
	if e := pageArgs(actor, after, limit); e != nil {
		return out, e
	}
	out.Conversations = []Conversation{}
	out.NextCursor = after
	err = s.run(ctx, func(tx Tx) error {
		if e := owned(tx, actor); e != nil {
			return e
		}
		rows, e := tx.Conversations(actor, after, limit+1)
		if e != nil {
			return e
		}
		for i, r := range rows {
			if i >= limit {
				out.HasMore = true
				break
			}
			out.Conversations = append(out.Conversations, r)
			out.NextCursor = r.ConversationID
		}
		return nil
	})
	return
}
func (s *Service) Send(ctx context.Context, actor, conversation string, in SendInput) (out Message, err error) {
	if !validID(actor) || !ValidUUID(conversation) || !ValidUUID(in.ClientMessageID) || (in.Kind != "application" && in.Kind != "commit") {
		return out, invalid()
	}
	if _, e := decode(in.CiphertextBase64, MaxOpaqueBytes); e != nil {
		return out, e
	}
	err = s.run(ctx, func(tx Tx) error {
		if e := owned(tx, actor); e != nil {
			return e
		}
		old, ok, e := tx.MessageByClient(actor, in.ClientMessageID)
		if e != nil {
			return e
		}
		if ok {
			if old.ConversationID != conversation || old.Kind != in.Kind || old.CiphertextBase64 != in.CiphertextBase64 {
				return conflict()
			}
			out = old
			return nil
		}
		c, ok, e := tx.Conversation(conversation)
		if e != nil {
			return e
		}
		if !ok {
			return missing()
		}
		target, member := peer(c.Conversation, actor)
		if !member {
			return missing()
		}
		if e = available(tx, actor, target); e != nil {
			return e
		}
		count, storedBytes, e := tx.MessageCount(conversation)
		if e != nil {
			return e
		}
		if count >= MaxRetainedMessages || storedBytes+len(in.CiphertextBase64) > MaxMessageBytes {
			return capacity()
		}
		c.LastSequence++
		id, e := uuid()
		if e != nil {
			return e
		}
		out = Message{MessageID: id, ClientMessageID: in.ClientMessageID, ConversationID: conversation, SenderVoidID: actor, RecipientVoidID: target, Kind: in.Kind, Sequence: strconv.FormatInt(c.LastSequence, 10), CiphertextBase64: in.CiphertextBase64, CreatedAt: s.now().UTC()}
		if e = tx.SaveConversation(c); e != nil {
			return e
		}
		if e = tx.SaveMessage(out); e != nil {
			return e
		}
		if e = reserveReceipt(tx, actor); e != nil {
			return e
		}
		return s.emit(tx, target, "message.created", out)
	})
	return
}
func parseCursor(s string) (int64, error) {
	if s == "" {
		return 0, nil
	}
	v, e := strconv.ParseInt(s, 10, 64)
	if e != nil || v < 0 || strconv.FormatInt(v, 10) != s {
		return 0, invalid()
	}
	return v, nil
}
func (s *Service) Sync(ctx context.Context, actor, after string, limit int) (out SyncPage, err error) {
	cursor, e := parseCursor(after)
	if e != nil || !validID(actor) || limit < 1 || limit > 100 {
		return out, invalid()
	}
	out.Events = []Event{}
	out.NextCursor = strconv.FormatInt(cursor, 10)
	err = s.run(ctx, func(tx Tx) error {
		if e := owned(tx, actor); e != nil {
			return e
		}
		rows, e := tx.Events(actor, cursor, limit+1)
		if e != nil {
			return e
		}
		size := 0
		for i, row := range rows {
			if i >= limit {
				out.HasMore = true
				break
			}
			if p := eventPeer(row); p != "" {
				blocked, e := tx.Blocked(actor, p)
				if e != nil {
					return e
				}
				if blocked {
					if e = cancelEvent(tx, row); e != nil {
						return e
					}
					out.NextCursor = row.Event.Cursor
					continue
				}
			}
			b, e := json.Marshal(row.Event)
			if e != nil {
				return e
			}
			if size+len(b)+512 > MaxSyncBytes && len(out.Events) > 0 {
				out.HasMore = true
				break
			}
			size += len(b) + 1
			out.Events = append(out.Events, row.Event)
			out.NextCursor = row.Event.Cursor
		}
		return nil
	})
	return
}
func (s *Service) Ack(ctx context.Context, actor string, acks []Acknowledgment) error {
	if !validID(actor) || len(acks) < 1 || len(acks) > 100 {
		return invalid()
	}
	for _, a := range acks {
		if !ValidUUID(a.EventID) || (a.Outcome != "applied" && a.Outcome != "rejected") {
			return invalid()
		}
	}
	return s.run(ctx, func(tx Tx) error {
		if e := owned(tx, actor); e != nil {
			return e
		}
		for _, a := range acks {
			row, ok, e := tx.Event(actor, a.EventID)
			if e != nil {
				return e
			}
			if !ok {
				return missing()
			}
			if row.Outcome != "" {
				if row.Outcome == a.Outcome {
					continue
				}
				return problem(409, "ack_conflict", "Event already has a different acknowledgment outcome.")
			}
			if row.Event.Type == "message.created" {
				var m Message
				if e = json.Unmarshal(row.Event.Payload, &m); e != nil {
					return e
				}
				blocked, e := tx.Blocked(actor, m.SenderVoidID)
				if e != nil {
					return e
				}
				if blocked {
					return problem(409, "event_cancelled", "Event delivery is no longer available.")
				}
				receipt := Receipt{MessageID: m.MessageID, ConversationID: m.ConversationID, Sequence: m.Sequence, RecipientVoidID: actor, Outcome: a.Outcome, CreatedAt: s.now().UTC()}
				if e = tx.ReserveReceipt(m.SenderVoidID, -1); e != nil {
					return e
				}
				kind := "message.rejected"
				if a.Outcome == "applied" {
					if m.Kind == "application" {
						kind = "message.delivered"
					} else {
						kind = ""
					}
				}
				if kind != "" {
					if e = s.emit(tx, m.SenderVoidID, kind, receipt); e != nil {
						return e
					}
				}
			}
			row.Outcome = a.Outcome
			if e = tx.SaveEvent(row); e != nil {
				return e
			}
		}
		return nil
	})
}

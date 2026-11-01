package integration

import (
	"context"
	"crypto/ed25519"
	"crypto/rand"
	"crypto/sha256"
	"encoding/base64"
	"encoding/hex"
	"encoding/json"
	"errors"
	"os"
	"strconv"
	"sync"
	"testing"
	"time"

	"github.com/jackc/pgx/v5/pgxpool"
	"void-nothingness/internal/messaging"
	"void-nothingness/internal/postgres"
	"void-nothingness/internal/session"
)

// These tests NEVER create schema or grant privileges. An owner must explicitly
// approve and provision a disposable void_test_ database, apply both reviewed
// migrations externally, and set BOTH integration opt-ins before running them.
func messagingPools(t *testing.T) *pgxpool.Pool {
	t.Helper()
	if os.Getenv("VOID_RUN_MESSAGING_INTEGRATION") != "1" {
		t.Skip("messaging database integration disabled; owner-approved disposable database, reviewed migrations and VOID_RUN_MESSAGING_INTEGRATION=1 are required")
	}
	app, _ := testPools(t)
	ctx, cancel := context.WithTimeout(context.Background(), 10*time.Second)
	defer cancel()
	if err := postgres.New(app).Validate(ctx); err != nil {
		t.Fatal("disposable database account schema/runtime privileges are not provisioned")
	}
	if err := postgres.NewSessionStore(app).Validate(ctx); err != nil {
		t.Fatal("apply reviewed session migration and runtime grants externally before running messaging integration")
	}
	if err := postgres.NewMessagingStore(app).Validate(ctx); err != nil {
		t.Fatal("apply reviewed messaging migration and runtime grants externally before running messaging integration")
	}
	return app
}

type messagingIdentity struct {
	id  string
	key ed25519.PrivateKey
}

func newMessagingIdentity(t *testing.T, ctx context.Context, app *pgxpool.Pool, name string) messagingIdentity {
	t.Helper()
	pub, private, err := ed25519.GenerateKey(rand.Reader)
	if err != nil {
		t.Fatal("could not generate synthetic integration identity")
	}
	r, err := postgres.New(app).Insert(ctx, freshID(t), name, pub)
	if err != nil {
		t.Fatalf("insert synthetic integration account: %v", err)
	}
	return messagingIdentity{r.VoidID, private}
}

func messagingUUID(t *testing.T) string {
	t.Helper()
	var b [16]byte
	if _, err := rand.Read(b[:]); err != nil {
		t.Fatal("could not generate integration client ID")
	}
	b[6], b[8] = b[6]&15|64, b[8]&63|128
	s := hex.EncodeToString(b[:])
	return s[:8] + "-" + s[8:12] + "-" + s[12:16] + "-" + s[16:20] + "-" + s[20:]
}

func newMessagingService(t *testing.T, app *pgxpool.Pool) *messaging.Service {
	t.Helper()
	s, err := messaging.NewService(postgres.NewMessagingStore(app))
	if err != nil {
		t.Fatal(err)
	}
	return s
}

func requireMessagingError(t *testing.T, err error, status int, code string) {
	t.Helper()
	var app *messaging.Error
	if !errors.As(err, &app) || app.Status != status || code != "" && app.Code != code {
		t.Fatalf("expected messaging error %d/%s; got %v", status, code, err)
	}
}

func integrationToken(t *testing.T, ctx context.Context, s *session.Service, id messagingIdentity) session.Token {
	t.Helper()
	c, err := s.Challenge(ctx, id.id)
	if err != nil {
		t.Fatal(err)
	}
	msg, err := base64.StdEncoding.DecodeString(c.Message)
	if err != nil {
		t.Fatal(err)
	}
	token, err := s.Create(ctx, c.ChallengeID, ed25519.Sign(id.key, msg))
	if err != nil {
		t.Fatal(err)
	}
	return token
}

func TestPostgresMessagingSessionRestartAndRevocation(t *testing.T) {
	app := messagingPools(t)
	ctx, cancel := context.WithTimeout(context.Background(), 20*time.Second)
	defer cancel()
	id := newMessagingIdentity(t, ctx, app, "synthetic-session")
	repo := postgres.New(app)
	first, err := session.New(repo, postgres.NewSessionStore(app), "https://api.voidchats.xyz/dm9pZG5vdGhpbmduZXNz")
	if err != nil {
		t.Fatal(err)
	}
	token := integrationToken(t, ctx, first, id)
	hash := sha256.Sum256([]byte(token.AccessToken))
	var storedHash []byte
	var storedID int64
	if err := app.QueryRow(ctx, `SELECT token_hash, void_id FROM public.void_sessions WHERE token_hash = $1`, hash[:]).Scan(&storedHash, &storedID); err != nil || string(storedHash) != string(hash[:]) || strconv.FormatInt(storedID, 10) != id.id {
		t.Fatal("session did not persist the expected token hash/account binding")
	}
	restarted, err := session.New(postgres.New(app), postgres.NewSessionStore(app), "https://api.voidchats.xyz/dm9pZG5vdGhpbmduZXNz")
	if err != nil {
		t.Fatal(err)
	}
	if got, err := restarted.Authenticate(ctx, token.AccessToken); err != nil || got != id.id {
		t.Fatal("persisted session did not survive service/store reconstruction")
	}
	if err := restarted.Revoke(ctx, token.AccessToken); err != nil {
		t.Fatal(err)
	}
	if _, err := first.Authenticate(ctx, token.AccessToken); err != session.ErrAuthentication {
		t.Fatal("revocation was not shared by independently constructed services")
	}
	if err := first.Revoke(ctx, token.AccessToken); err != nil {
		t.Fatal("repeat revocation was not idempotent")
	}
}

func TestPostgresMessagingConcurrentClaimsAndStableRetries(t *testing.T) {
	app := messagingPools(t)
	ctx, cancel := context.WithTimeout(context.Background(), 30*time.Second)
	defer cancel()
	owner := newMessagingIdentity(t, ctx, app, "synthetic-publisher")
	claimant := newMessagingIdentity(t, ctx, app, "synthetic-claimant")
	other := newMessagingIdentity(t, ctx, app, "synthetic-other-target")
	s := newMessagingService(t, app)
	data := []byte("synthetic opaque public KeyPackage")
	published, err := s.Publish(ctx, owner.id, [][]byte{data})
	if err != nil || len(published) != 1 {
		t.Fatalf("publish synthetic package: %v", err)
	}
	const workers = 8
	ids := make([]string, workers)
	for i := range ids {
		ids[i] = messagingUUID(t)
	}
	type result struct {
		claim messaging.Claim
		err   error
	}
	results := make(chan result, workers)
	var wg sync.WaitGroup
	for _, id := range ids {
		wg.Go(func() {
			claim, err := newMessagingService(t, app).Claim(ctx, claimant.id, owner.id, id)
			results <- result{claim, err}
		})
	}
	wg.Wait()
	close(results)
	var winner messaging.Claim
	success := 0
	for r := range results {
		if r.err == nil {
			winner = r.claim
			success++
		} else {
			requireMessagingError(t, r.err, 409, "key_package_unavailable")
		}
	}
	if success != 1 || winner.KeyPackageID != published[0].KeyPackageID {
		t.Fatalf("one public package was claimed %d times", success)
	}
	retry, err := newMessagingService(t, app).Claim(ctx, claimant.id, owner.id, winner.ClientClaimID)
	if err != nil || retry != winner {
		t.Fatal("claim retry did not return original package after reconstruction")
	}
	_, err = s.Claim(ctx, claimant.id, other.id, winner.ClientClaimID)
	requireMessagingError(t, err, 409, "idempotency_conflict")
	reupload, err := s.Publish(ctx, owner.id, [][]byte{data})
	if err != nil || len(reupload) != 1 || reupload[0].KeyPackageID != winner.KeyPackageID || reupload[0].State != "claimed" {
		t.Fatal("reupload recycled or duplicated a claimed package")
	}
}

func syntheticRequest(t *testing.T, ctx context.Context, s *messaging.Service, from, to string) messaging.CreateRequestInput {
	t.Helper()
	_, err := s.Publish(ctx, to, [][]byte{[]byte("opaque-public-package:" + messagingUUID(t))})
	if err != nil {
		t.Fatal(err)
	}
	claim, err := s.Claim(ctx, from, to, messagingUUID(t))
	if err != nil {
		t.Fatal(err)
	}
	return messaging.CreateRequestInput{ClientRequestID: messagingUUID(t), ToVoidID: to, KeyPackageID: claim.KeyPackageID, GroupIDBase64: base64.StdEncoding.EncodeToString([]byte("synthetic-group")), WelcomeBase64: base64.StdEncoding.EncodeToString([]byte("opaque MLS Welcome"))}
}

func allMessagingEvents(t *testing.T, ctx context.Context, s *messaging.Service, actor string) []messaging.Event {
	t.Helper()
	var events []messaging.Event
	cursor := "0"
	last := int64(0)
	for pages := 0; pages < 100; pages++ {
		// A one-event page verifies persisted ordering and stable pagination.
		page, err := s.Sync(ctx, actor, cursor, 1)
		if err != nil {
			t.Fatal(err)
		}
		for _, event := range page.Events {
			n, err := strconv.ParseInt(event.Cursor, 10, 64)
			if err != nil || n <= last {
				t.Fatal("sync did not return strictly ascending decimal-string cursors")
			}
			last = n
			events = append(events, event)
		}
		if !page.HasMore {
			return events
		}
		if page.NextCursor == cursor {
			t.Fatal("sync pagination failed to advance")
		}
		cursor = page.NextCursor
	}
	t.Fatal("integration sync exceeded expected page count")
	return nil
}

func eventForMessage(t *testing.T, events []messaging.Event, id string) messaging.Event {
	t.Helper()
	for _, event := range events {
		if event.Type != "message.created" {
			continue
		}
		var message messaging.Message
		if err := json.Unmarshal(event.Payload, &message); err != nil {
			t.Fatal(err)
		}
		if message.MessageID == id {
			return event
		}
	}
	t.Fatalf("durable message event %s missing", id)
	return messaging.Event{}
}

func TestPostgresMessagingAcceptedConversationOfflineSyncAndOutcomes(t *testing.T) {
	app := messagingPools(t)
	ctx, cancel := context.WithTimeout(context.Background(), 45*time.Second)
	defer cancel()
	a := newMessagingIdentity(t, ctx, app, "synthetic-alice")
	b := newMessagingIdentity(t, ctx, app, "synthetic-bob")
	outsider := newMessagingIdentity(t, ctx, app, "synthetic-outsider")
	s := newMessagingService(t, app)
	forward := syntheticRequest(t, ctx, s, a.id, b.id)
	reverse := syntheticRequest(t, ctx, s, b.id, a.id)
	type requestResult struct {
		r   messaging.Request
		err error
	}
	results := make(chan requestResult, 2)
	var wg sync.WaitGroup
	wg.Go(func() { r, e := s.CreateRequest(ctx, a.id, forward); results <- requestResult{r, e} })
	wg.Go(func() {
		r, e := newMessagingService(t, app).CreateRequest(ctx, b.id, reverse)
		results <- requestResult{r, e}
	})
	wg.Wait()
	close(results)
	var request messaging.Request
	success := 0
	for r := range results {
		if r.err == nil {
			request = r.r
			success++
		} else {
			requireMessagingError(t, r.err, 409, "conversation_exists")
		}
	}
	if success != 1 || request.Status != "pending" {
		t.Fatal("opposite invitations did not create one canonical pending conversation")
	}
	input := forward
	if request.Sender.VoidID == b.id {
		input = reverse
	}
	retried, err := s.CreateRequest(ctx, request.Sender.VoidID, input)
	if err != nil || retried.RequestID != request.RequestID || retried.ConversationID != request.ConversationID {
		t.Fatal("stable invitation retry did not return original pending conversation")
	}
	_, err = s.Accept(ctx, request.Sender.VoidID, request.RequestID)
	requireMessagingError(t, err, 403, "forbidden")
	_, err = s.Accept(ctx, outsider.id, request.RequestID)
	requireMessagingError(t, err, 404, "not_found")
	_, err = s.Send(ctx, request.Sender.VoidID, request.ConversationID, messaging.SendInput{ClientMessageID: messagingUUID(t), Kind: "application", CiphertextBase64: base64.StdEncoding.EncodeToString([]byte("opaque-before-accept"))})
	requireMessagingError(t, err, 404, "not_found")
	accepted, err := s.Accept(ctx, request.Recipient.VoidID, request.RequestID)
	if err != nil || accepted.Status != "accepted" {
		t.Fatalf("recipient acceptance failed: %v", err)
	}
	if _, err := s.Accept(ctx, request.Recipient.VoidID, request.RequestID); err != nil {
		t.Fatal("repeated acceptance was not idempotent")
	}
	_, err = s.Decline(ctx, request.Recipient.VoidID, request.RequestID)
	requireMessagingError(t, err, 409, "request_not_pending")
	for _, actor := range []string{a.id, b.id} {
		list, err := s.ListConversations(ctx, actor, "", 50)
		if err != nil || len(list.Conversations) != 1 || list.Conversations[0].ConversationID != request.ConversationID {
			t.Fatal("participants did not see the same accepted conversation")
		}
	}
	_, err = s.Send(ctx, outsider.id, request.ConversationID, messaging.SendInput{ClientMessageID: messagingUUID(t), Kind: "application", CiphertextBase64: base64.StdEncoding.EncodeToString([]byte("opaque-forged-participant"))})
	requireMessagingError(t, err, 404, "not_found")
	sender, recipient := request.Sender.VoidID, request.Recipient.VoidID
	commit, err := s.Send(ctx, sender, request.ConversationID, messaging.SendInput{ClientMessageID: messagingUUID(t), Kind: "commit", CiphertextBase64: base64.StdEncoding.EncodeToString([]byte("opaque MLS commit"))})
	if err != nil || commit.Sequence != "1" {
		t.Fatal("commit sequence allocation failed")
	}
	send := messaging.SendInput{ClientMessageID: messagingUUID(t), Kind: "application", CiphertextBase64: base64.StdEncoding.EncodeToString([]byte("opaque MLS application"))}
	message, err := s.Send(ctx, sender, request.ConversationID, send)
	if err != nil || message.Sequence != "2" {
		t.Fatal("application did not follow commit sequence")
	}
	s = newMessagingService(t, app) // Reconstruct the entire delivery service/store.
	retry, err := s.Send(ctx, sender, request.ConversationID, send)
	if err != nil || retry != message {
		t.Fatal("lost-response message retry did not return original persisted result")
	}
	changed := send
	changed.CiphertextBase64 = base64.StdEncoding.EncodeToString([]byte("different encrypted bytes"))
	_, err = s.Send(ctx, sender, request.ConversationID, changed)
	requireMessagingError(t, err, 409, "idempotency_conflict")
	events := allMessagingEvents(t, ctx, s, recipient)
	commitEvent := eventForMessage(t, events, commit.MessageID)
	messageEvent := eventForMessage(t, events, message.MessageID)
	commitCursor, _ := strconv.ParseInt(commitEvent.Cursor, 10, 64)
	messageCursor, _ := strconv.ParseInt(messageEvent.Cursor, 10, 64)
	if commitCursor >= messageCursor {
		t.Fatal("application event appeared before its required commit")
	}
	if len(events) != 4 {
		t.Fatalf("expected request, acceptance, commit and one application event; got %d", len(events))
	}
	err = s.Ack(ctx, outsider.id, []messaging.Acknowledgment{{EventID: messageEvent.EventID, Outcome: "applied"}})
	requireMessagingError(t, err, 404, "not_found")
	acks := []messaging.Acknowledgment{{EventID: commitEvent.EventID, Outcome: "applied"}, {EventID: messageEvent.EventID, Outcome: "rejected"}}
	if err := s.Ack(ctx, recipient, acks); err != nil {
		t.Fatal(err)
	}
	if err := newMessagingService(t, app).Ack(ctx, recipient, acks); err != nil {
		t.Fatal("persisted ACK retry was not idempotent")
	}
	err = s.Ack(ctx, recipient, []messaging.Acknowledgment{{EventID: messageEvent.EventID, Outcome: "applied"}})
	requireMessagingError(t, err, 409, "ack_conflict")
	good, err := s.Send(ctx, sender, request.ConversationID, messaging.SendInput{ClientMessageID: messagingUUID(t), Kind: "application", CiphertextBase64: base64.StdEncoding.EncodeToString([]byte("another opaque application"))})
	if err != nil || good.Sequence != "3" {
		t.Fatal("subsequent conversation sequence was not persisted")
	}
	goodEvent := eventForMessage(t, allMessagingEvents(t, ctx, s, recipient), good.MessageID)
	if err := s.Ack(ctx, recipient, []messaging.Acknowledgment{{EventID: goodEvent.EventID, Outcome: "applied"}}); err != nil {
		t.Fatal(err)
	}
	counts := map[string]int{}
	for _, event := range allMessagingEvents(t, ctx, newMessagingService(t, app), sender) {
		if event.Type != "message.delivered" && event.Type != "message.rejected" {
			continue
		}
		var receipt messaging.Receipt
		if err := json.Unmarshal(event.Payload, &receipt); err != nil {
			t.Fatal(err)
		}
		counts[event.Type]++
		if event.Type == "message.rejected" && (receipt.MessageID != message.MessageID || receipt.Outcome != "rejected") || event.Type == "message.delivered" && (receipt.MessageID != good.MessageID || receipt.Outcome != "applied") {
			t.Fatal("receipt outcome or message binding was incorrect")
		}
	}
	if counts["message.delivered"] != 1 || counts["message.rejected"] != 1 {
		t.Fatal("ACK replay duplicated receipts or reported rejected/commit data as delivered")
	}
}

func TestPostgresMessagingBlockCancelsPendingAndSuppressesQueuedEvents(t *testing.T) {
	app := messagingPools(t)
	ctx, cancel := context.WithTimeout(context.Background(), 30*time.Second)
	defer cancel()
	a := newMessagingIdentity(t, ctx, app, "synthetic-blocker")
	b := newMessagingIdentity(t, ctx, app, "synthetic-blocked")
	s := newMessagingService(t, app)
	in := syntheticRequest(t, ctx, s, a.id, b.id)
	r, err := s.CreateRequest(ctx, a.id, in)
	if err != nil {
		t.Fatal(err)
	}
	if err := s.Block(ctx, b.id, a.id); err != nil {
		t.Fatal(err)
	}
	s = newMessagingService(t, app)
	if err := s.Block(ctx, b.id, a.id); err != nil {
		t.Fatal("block retry was not durable/idempotent")
	}
	_, err = s.Accept(ctx, b.id, r.RequestID)
	requireMessagingError(t, err, 409, "request_not_pending")
	_, err = s.Claim(ctx, a.id, b.id, messagingUUID(t))
	requireMessagingError(t, err, 404, "not_found")
	_, err = s.Claim(ctx, b.id, a.id, messagingUUID(t))
	requireMessagingError(t, err, 404, "not_found")
	if events := allMessagingEvents(t, ctx, s, a.id); len(events) != 0 {
		t.Fatal("blocked peer received queued invitation or private block notification")
	}
	events := allMessagingEvents(t, ctx, s, b.id)
	if len(events) != 1 || events[0].Type != "block.updated" {
		t.Fatal("blocking account did not receive exactly its own durable block update")
	}

	// Cancellation of a queued application event must also release the sender's
	// receipt reservation transactionally, without producing a delivery receipt.
	c := newMessagingIdentity(t, ctx, app, "synthetic-queued-sender")
	d := newMessagingIdentity(t, ctx, app, "synthetic-queued-recipient")
	queuedInput := syntheticRequest(t, ctx, s, c.id, d.id)
	queuedRequest, err := s.CreateRequest(ctx, c.id, queuedInput)
	if err != nil {
		t.Fatal(err)
	}
	if _, err := s.Accept(ctx, d.id, queuedRequest.RequestID); err != nil {
		t.Fatal(err)
	}
	queuedMessage, err := s.Send(ctx, c.id, queuedRequest.ConversationID, messaging.SendInput{ClientMessageID: messagingUUID(t), Kind: "application", CiphertextBase64: base64.StdEncoding.EncodeToString([]byte("opaque application awaiting recipient"))})
	if err != nil {
		t.Fatal(err)
	}
	beforeBlock := eventForMessage(t, allMessagingEvents(t, ctx, s, d.id), queuedMessage.MessageID)
	if err := s.Block(ctx, d.id, c.id); err != nil {
		t.Fatal("block failed to cancel pending application and receipt reservation")
	}
	s = newMessagingService(t, app)
	for _, event := range allMessagingEvents(t, ctx, s, d.id) {
		if event.Type != "block.updated" {
			t.Fatal("blocked conversation retained a pending peer event")
		}
	}
	for _, event := range allMessagingEvents(t, ctx, s, c.id) {
		if event.Type == "message.delivered" || event.Type == "message.rejected" {
			t.Fatal("cancelled queued message generated a misleading delivery receipt")
		}
	}
	err = s.Ack(ctx, d.id, []messaging.Acknowledgment{{EventID: beforeBlock.EventID, Outcome: "applied"}})
	requireMessagingError(t, err, 409, "")
	for _, actor := range []string{c.id, d.id} {
		_, err = s.Send(ctx, actor, queuedRequest.ConversationID, messaging.SendInput{ClientMessageID: messagingUUID(t), Kind: "application", CiphertextBase64: base64.StdEncoding.EncodeToString([]byte("opaque blocked application"))})
		requireMessagingError(t, err, 404, "not_found")
	}
}

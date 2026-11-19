package relay

import (
	"bytes"
	"context"
	"encoding/base64"
	"fmt"
	"testing"
)

func TestInitialV1OpaqueOperationsRecoverOfflineInOrder(t *testing.T) {
	// Labels describe fixture intent only. The relay sees synthetic opaque bytes,
	// not Signal plaintext or a feature discriminator. These are transport tests,
	// not libsignal interoperability vectors.
	kinds := []string{"text", "reply", "reaction_add", "reaction_remove", "delete_for_everyone"}
	f := newFixture(t, Options{}, 2)
	alice := f.authenticate(t, "1")
	negotiate(t, alice, "1")
	ids := make([]string, len(kinds))
	payloads := make([]string, len(kinds))
	for i, kind := range kinds {
		ids[i] = fmt.Sprintf("10000000-0000-4000-8000-%012x", i+1)
		payloads[i] = base64.StdEncoding.EncodeToString([]byte{0, 255, byte(i), 128, 0, 254})
		write(t, alice, queuedSend(ids[i], "2", payloads[i]))
		expectAccepted(t, read(t, alice), ids[i])
		t.Log("queued opaque V1 fixture:", kind)
	}
	ctx := context.Background()
	original, err := f.server.opts.Queue.Pending(ctx, "2", 100)
	if err != nil || len(original) != len(kinds) {
		t.Fatalf("offline queue: %+v %v", original, err)
	}
	for i, event := range original {
		if event.EventID != ids[i] || event.MinProtocolVersion != 1 || base64.StdEncoding.EncodeToString(event.Payload) != payloads[i] {
			t.Fatalf("V1 metadata/order/ciphertext changed for %s", kinds[i])
		}
	}
	// Acceptance loss can lead to a duplicate retry. It must not overwrite or
	// extend the pending record, including when its replacement bytes differ.
	write(t, alice, queuedSend(ids[0], "2", "AQ=="))
	assertFields(t, read(t, alice), map[string]string{"type": "error", "code": "duplicate_event"})
	// The sender is not authorized to ACK the recipient's queued ciphertext.
	ackEvent(t, alice, "1", ids[0])
	rows, err := f.server.opts.Queue.Pending(ctx, "2", 100)
	if err != nil || len(rows) != len(original) {
		t.Fatalf("duplicate/foreign ACK changed queue: %+v %v", rows, err)
	}
	for i, event := range rows {
		if !bytes.Equal(event.Payload, original[i].Payload) || !event.CreatedAt.Equal(original[i].CreatedAt) || !event.ExpiresAt.Equal(original[i].ExpiresAt) {
			t.Fatalf("retry changed persisted %s", kinds[i])
		}
	}
	bob := f.authenticate(t, "2")
	head := read(t, bob)
	expectEvent(t, head, ids[0], "1", payloads[0])
	// Lose the ACK. The reconnect must replay this head, not advance to a control.
	_ = bob.CloseNow()
	bob = f.authenticate(t, "2")
	assertFields(t, read(t, bob), head)
	negotiate(t, bob, "1")
	for i := range kinds {
		if i != 0 {
			expectEvent(t, read(t, bob), ids[i], "1", payloads[i])
		}
		ackEvent(t, bob, "1", ids[i])
	}
	ackEvent(t, bob, "1", ids[len(ids)-1]) // Repeated ACK stays harmless.
	rows, err = f.server.opts.Queue.Pending(ctx, "2", 100)
	if err != nil || len(rows) != 0 {
		t.Fatalf("ACK did not drain queue: %+v %v", rows, err)
	}
	// An encrypted receipt is a new opaque event in the reverse direction. Go
	// does not synthesize it from ACKs or give it a special frame shape.
	receiptID := "20000000-0000-4000-8000-000000000001"
	receipt := "AP+BAA=="
	write(t, bob, queuedSend(receiptID, "1", receipt))
	expectAccepted(t, read(t, bob), receiptID)
	expectEvent(t, read(t, alice), receiptID, "2", receipt)
	ackEvent(t, alice, "2", receiptID)
	rows, err = f.server.opts.Queue.Pending(ctx, "1", 100)
	if err != nil || len(rows) != 0 {
		t.Fatalf("receipt ACK: %+v %v", rows, err)
	}
}

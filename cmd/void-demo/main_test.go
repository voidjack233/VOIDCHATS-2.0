package main

import (
	"bytes"
	"context"
	"encoding/base64"
	"strings"
	"testing"
	"time"
)

func TestDemoRegistersAuthenticatesRelaysAndDisconnects(t *testing.T) {
	ctx, cancel := context.WithTimeout(context.Background(), 10*time.Second)
	defer cancel()
	var output bytes.Buffer
	report, err := runDemo(ctx, &output)
	if err != nil {
		t.Fatalf("local backend demonstration: %v", err)
	}
	if report.Alice.VoidID == "" || report.Alice.VoidID == report.Bob.VoidID ||
		report.Alice.Username != "demo-alice" || report.Bob.Username != "demo-bob" ||
		report.Alice.CreatedAt.IsZero() || report.Bob.CreatedAt.IsZero() {
		t.Fatalf("expected two distinct registered accounts, got Alice=%+v Bob=%+v", report.Alice, report.Bob)
	}
	if report.Active != 2 || report.FinalActive != 0 {
		t.Fatalf("active connections: during exchange=%d after cleanup=%d", report.Active, report.FinalActive)
	}
	for _, exchange := range []struct {
		name, sender, eventID, ciphertext string
		message, result                   relayEnvelope
	}{
		{"Alice to Bob", report.Alice.VoidID, forwardEventID, simulatedCiphertext, report.Forward, report.Forwarded},
		{"Bob to Alice", report.Bob.VoidID, replyEventID, replyCiphertext, report.Reverse, report.Replied},
	} {
		if exchange.message.Type != "event" || exchange.message.EventID != exchange.eventID || exchange.message.FromVoidID != exchange.sender {
			t.Errorf("%s: wrong authenticated sender or request metadata: %+v", exchange.name, exchange.message)
		}
		payload, err := base64.StdEncoding.Strict().DecodeString(exchange.message.Payload)
		if err != nil || string(payload) != exchange.ciphertext {
			t.Errorf("%s: opaque payload changed in transit", exchange.name)
		}
		if exchange.result.Type != "accepted" || exchange.result.EventID != exchange.eventID || exchange.result.CreatedAt.IsZero() || exchange.result.ExpiresAt.IsZero() {
			t.Errorf("%s: unexpected forwarding result: %+v", exchange.name, exchange.result)
		}
	}
	if report.Offline.Type != "accepted" || report.Offline.EventID != offlineEventID {
		t.Errorf("offline recipient result: %+v", report.Offline)
	}
	if report.Reconnected.Type != "event" || report.Reconnected.EventID != offlineEventID || report.Reconnected.FromVoidID != report.Alice.VoidID || report.FinalPending != 0 {
		t.Errorf("offline queue delivery/deletion: event=%+v remaining=%d", report.Reconnected, report.FinalPending)
	}
	// The output exposes only explicitly simulated message fixtures and public
	// routing metadata; handshake secrets and proofs must remain out of logs.
	for _, forbidden := range []string{"auth_public_key", "private_key", "signature", "challenge_id", "auth_proof", "auth_challenge"} {
		if strings.Contains(output.String(), forbidden) {
			t.Errorf("demo output exposed an authentication field: %q", forbidden)
		}
	}
}

func TestDemoRejectsUnboundAuthenticationChallengesBeforeSigning(t *testing.T) {
	id := base64.RawURLEncoding.EncodeToString(bytes.Repeat([]byte{1}, 32))
	nonce := base64.RawURLEncoding.EncodeToString(bytes.Repeat([]byte{2}, 32))
	expires := time.Now().UTC().Add(10 * time.Second)
	message := "VOID-NOTHINGNESS:authenticate:v1\n" + id + "\n42\n" + expires.Format(time.RFC3339Nano) + "\n" + nonce + "\n"
	valid := authEnvelope{Type: "auth_challenge", ChallengeID: id, ExpiresAt: expires, Message: base64.StdEncoding.EncodeToString([]byte(message))}
	if decoded, err := authenticationSigningMessage(valid, "42"); err != nil || string(decoded) != message {
		t.Fatalf("valid challenge rejected: %v", err)
	}
	for _, tc := range []struct {
		name   string
		mutate func(*authEnvelope)
	}{
		{"domain", func(c *authEnvelope) {
			c.Message = base64.StdEncoding.EncodeToString([]byte(strings.Replace(message, "authenticate", "register", 1)))
		}},
		{"identity", func(c *authEnvelope) {
			c.Message = base64.StdEncoding.EncodeToString([]byte(strings.Replace(message, "\n42\n", "\n43\n", 1)))
		}},
		{"challenge ID", func(c *authEnvelope) {
			c.ChallengeID = base64.RawURLEncoding.EncodeToString(bytes.Repeat([]byte{3}, 32))
		}},
		{"expiry binding", func(c *authEnvelope) { c.ExpiresAt = c.ExpiresAt.Add(time.Second) }},
		{"expired", func(c *authEnvelope) { c.ExpiresAt = time.Now().Add(-time.Second) }},
		{"nonce", func(c *authEnvelope) {
			c.Message = base64.StdEncoding.EncodeToString([]byte(strings.Replace(message, nonce, "short", 1)))
		}},
		{"frame type", func(c *authEnvelope) { c.Type = "auth_ok" }},
	} {
		t.Run(tc.name, func(t *testing.T) {
			challenge := valid
			tc.mutate(&challenge)
			if _, err := authenticationSigningMessage(challenge, "42"); err == nil {
				t.Fatal("client would sign an unbound or invalid authentication challenge")
			}
		})
	}
}

// Package messaging implements durable, account-scoped delivery of opaque MLS
// objects. It never parses plaintext, private keys, or native MLS state.
package messaging

import (
	"context"
	"encoding/json"
	"time"
)

const (
	MaxOpaqueBytes        = 1 << 20
	MaxPackageBatch       = 16
	MaxPackagesPerAccount = 4096
	MaxPendingEvents      = 1000
	MaxRetainedEvents     = 10000
	MaxRetainedMessages   = 10000
	MaxRequestsPerAccount = 4096
	MaxSyncBytes          = 4 << 20
	MaxPackageBytes       = 16 << 20
	MaxPendingEventBytes  = 16 << 20
	MaxRetainedEventBytes = 64 << 20
	MaxMessageBytes       = 128 << 20
	ReceiptReserveBytes   = 1024
	PackageTTL            = 7 * 24 * time.Hour
)

type Error struct {
	Status  int
	Code    string
	Message string
}

func (e *Error) Error() string { return e.Message }

type Person struct {
	VoidID        string `json:"void_id"`
	Username      string `json:"username"`
	AuthPublicKey string `json:"auth_public_key"`
}
type PublishedPackage struct {
	KeyPackageID string    `json:"key_package_id"`
	State        string    `json:"state"`
	ExpiresAt    time.Time `json:"expires_at"`
}
type Claim struct {
	ClientClaimID    string `json:"client_claim_id"`
	KeyPackageID     string `json:"key_package_id"`
	KeyPackageBase64 string `json:"key_package_base64"`
	Recipient        Person `json:"recipient"`
}
type CreateRequestInput struct {
	ClientRequestID string `json:"client_request_id"`
	ToVoidID        string `json:"to_void_id"`
	KeyPackageID    string `json:"key_package_id"`
	GroupIDBase64   string `json:"group_id_base64"`
	WelcomeBase64   string `json:"welcome_base64"`
}
type Request struct {
	RequestID       string    `json:"request_id"`
	ConversationID  string    `json:"conversation_id"`
	ClientRequestID string    `json:"client_request_id"`
	KeyPackageID    string    `json:"key_package_id"`
	Status          string    `json:"status"`
	Sender          Person    `json:"sender"`
	Recipient       Person    `json:"recipient"`
	GroupIDBase64   string    `json:"group_id_base64"`
	WelcomeBase64   string    `json:"welcome_base64"`
	CreatedAt       time.Time `json:"created_at"`
	UpdatedAt       time.Time `json:"updated_at"`
}
type RequestPage struct {
	Requests   []Request `json:"requests"`
	NextCursor string    `json:"next_cursor"`
	HasMore    bool      `json:"has_more"`
}
type Conversation struct {
	ConversationID string    `json:"conversation_id"`
	RequestID      string    `json:"request_id"`
	Participants   []Person  `json:"participants"`
	GroupIDBase64  string    `json:"group_id_base64"`
	CreatedAt      time.Time `json:"created_at"`
	// Internal sequence is persisted but hidden from list responses.
	LastSequence int64 `json:"-"`
}
type ConversationPage struct {
	Conversations []Conversation `json:"conversations"`
	NextCursor    string         `json:"next_cursor"`
	HasMore       bool           `json:"has_more"`
}
type SendInput struct {
	ClientMessageID  string `json:"client_message_id"`
	Kind             string `json:"kind"`
	CiphertextBase64 string `json:"ciphertext_base64"`
}
type Message struct {
	MessageID        string    `json:"message_id"`
	ClientMessageID  string    `json:"client_message_id"`
	ConversationID   string    `json:"conversation_id"`
	SenderVoidID     string    `json:"sender_void_id"`
	RecipientVoidID  string    `json:"recipient_void_id"`
	Kind             string    `json:"kind"`
	Sequence         string    `json:"sequence"`
	CiphertextBase64 string    `json:"ciphertext_base64"`
	CreatedAt        time.Time `json:"created_at"`
}
type Event struct {
	EventID   string          `json:"event_id"`
	Cursor    string          `json:"cursor"`
	Type      string          `json:"type"`
	Payload   json.RawMessage `json:"payload"`
	CreatedAt time.Time       `json:"created_at"`
}
type SyncPage struct {
	Events     []Event `json:"events"`
	NextCursor string  `json:"next_cursor"`
	HasMore    bool    `json:"has_more"`
}
type Acknowledgment struct {
	EventID string `json:"event_id"`
	Outcome string `json:"outcome"`
}
type Receipt struct {
	MessageID       string    `json:"message_id"`
	ConversationID  string    `json:"conversation_id"`
	Sequence        string    `json:"sequence"`
	RecipientVoidID string    `json:"recipient_void_id"`
	Outcome         string    `json:"outcome"`
	CreatedAt       time.Time `json:"created_at"`
}

// Storage rows contain only public metadata, encrypted wire bytes, and ACK state.
type PackageRow struct {
	ID, Owner, Fingerprint, Data, ClaimedBy, ClaimID, RequestID string
	ExpiresAt                                                   time.Time
}
type EventRow struct {
	Owner   string
	Event   Event
	Outcome string
}
type ConversationRow struct {
	Conversation Conversation
	LastSequence int64
}

// Run provides a serializable atomic transaction. Implementations must serialize
// mutations, including event allocation, so a later visible cursor cannot commit
// before an earlier cursor. Callbacks must not escape or retain Tx references.
type Store interface {
	Run(context.Context, func(Tx) error) error
	Validate(context.Context) error
}
type Tx interface {
	Person(string) (Person, bool, error)
	Package(string) (PackageRow, bool, error)
	PackageByFingerprint(string, string) (PackageRow, bool, error)
	AvailablePackage(string, time.Time) (PackageRow, bool, error)
	ClaimedPackage(string, string) (PackageRow, bool, error)
	PackageCount(string) (count, encodedBytes int, err error)
	SavePackage(PackageRow) error
	Request(string) (Request, bool, error)
	RequestByClient(string, string) (Request, bool, error)
	ActivePair(string, string) (Request, bool, error)
	RequestCount(string) (int, error)
	Requests(string, string, int) ([]Request, error)
	SaveRequest(Request) error
	Conversation(string) (ConversationRow, bool, error)
	Conversations(string, string, int) ([]Conversation, error)
	SaveConversation(ConversationRow) error
	Blocked(string, string) (bool, error)
	SaveBlock(string, string, time.Time) (inserted bool, err error)
	MessageByClient(string, string) (Message, bool, error)
	MessageCount(string) (count, encodedBytes int, err error)
	SaveMessage(Message) error
	Event(string, string) (EventRow, bool, error)
	Events(string, int64, int) ([]EventRow, error)
	EventCounts(string) (total, pending, totalBytes, pendingBytes int, err error)
	ReceiptReservations(string) (int, error)
	ReserveReceipt(string, int) error
	NextCursor(string) (int64, error)
	SaveEvent(EventRow) error
}

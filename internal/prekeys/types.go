// Package prekeys distributes authenticated public Signal setup material.
// Signal signatures and all private key operations remain client responsibilities.
package prekeys

import (
	"context"
	"errors"
	"time"
)

const (
	MaxUploadBytes      = 256 << 10
	MaxBundleBytes      = 32 << 10
	MaxInventoryKeys    = 64
	MaxPublications     = 64
	MaxClaimsPerAccount = 1024
	MaxClaimsGlobal     = 100000
	MaxInventories      = 10000
	MaxKeyID            = 16777214
	ClaimTTL            = 24 * time.Hour
)

var (
	ErrInvalid     = errors.New("invalid prekey input")
	ErrConflict    = errors.New("prekey identifier conflict")
	ErrCapacity    = errors.New("prekey capacity reached")
	ErrExpired     = errors.New("prekey claim expired")
	ErrUnavailable = errors.New("prekeys unavailable")
)

type PublicKey struct {
	KeyID     int    `json:"key_id"`
	PublicKey string `json:"public_key"`
}

type SignedKey struct {
	KeyID     int    `json:"key_id"`
	PublicKey string `json:"public_key"`
	Signature string `json:"signature"`
}

type Upload struct {
	V                   int         `json:"v"`
	DeviceID            int         `json:"device_id"`
	RegistrationID      int         `json:"registration_id"`
	IdentityKey         string      `json:"identity_key"`
	AccountSignature    string      `json:"account_signature"`
	SignedPrekey        SignedKey   `json:"signed_prekey"`
	KyberLastResort     SignedKey   `json:"kyber_last_resort"`
	OneTimePrekeys      []PublicKey `json:"one_time_prekeys"`
	OneTimeKyberPrekeys []SignedKey `json:"one_time_kyber_prekeys"`
}

type Counts struct {
	ECRemaining    int `json:"ec_remaining"`
	KyberRemaining int `json:"kyber_remaining"`
}

type ClaimedKyberKey struct {
	SignedKey
	LastResort bool `json:"last_resort"`
}

type Bundle struct {
	V                int             `json:"v"`
	DeviceID         int             `json:"device_id"`
	RegistrationID   int             `json:"registration_id"`
	IdentityKey      string          `json:"identity_key"`
	AccountSignature string          `json:"account_signature"`
	SignedPrekey     SignedKey       `json:"signed_prekey"`
	KyberPrekey      ClaimedKyberKey `json:"kyber_prekey"`
	OneTimePrekey    *PublicKey      `json:"one_time_prekey,omitempty"`
	ClaimExpiresAt   time.Time       `json:"claim_expires_at"`
}

// Inventory contains only public material. Publications is a bounded lifetime
// UUID-to-SHA256 history; consumed IDs are protected by persistent watermarks.
type Inventory struct {
	Upload       Upload
	MaxECID      int
	MaxKyberID   int
	Publications map[string]string
	CreatedAt    time.Time
	UpdatedAt    time.Time
}

// Store operations are atomic: retrying a claim UUID cannot consume more keys.
// Expire clears bundle and recipient data while retaining bounded UUID markers.
type Store interface {
	Publish(context.Context, string, string, Upload, [32]byte, time.Time) (Counts, error)
	Claim(context.Context, string, string, string, time.Time) (Bundle, error)
	Expire(context.Context, time.Time, int) (int64, error)
	Validate(context.Context) error
}

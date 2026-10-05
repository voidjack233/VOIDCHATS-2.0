// Package account handles account initialization and lookup.
package account

import "time"

// Account records client-provided public account identity and profile metadata.
// The client creates the VOID-ID and chat identity keypair. Identity private
// keys remain on the client and have no place in this model.
type Account struct {
	VOIDID            string    `json:"void_id"`
	DisplayName       string    `json:"display_name"`
	IdentityPublicKey string    `json:"identity_public_key"`
	CreatedAt         time.Time `json:"created_at"`
}

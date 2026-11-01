package account

// ValidPublicKey reports whether key is a canonical, nonidentity Ed25519 point
// in the prime-order subgroup. Authentication and registration share this check.
func ValidPublicKey(key []byte) bool {
	return validPublicKey(key)
}

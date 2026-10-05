package voidid

import "strings"

// ValidateSubmitted checks a client-supplied account identifier for this
// experimental milestone. Only a nonblank value is required; no generated-ID
// format is imposed, and the caller must preserve the exact submitted text.
// Future account identifier rules belong here, separate from account logic.
func ValidateSubmitted(text string) error {
	if strings.TrimSpace(text) == "" {
		return ErrInvalid
	}
	return nil
}
